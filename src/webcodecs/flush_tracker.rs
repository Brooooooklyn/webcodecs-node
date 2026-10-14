use crossbeam::channel::Sender;
use napi::bindgen_prelude::{Error, Result, Status};
use std::sync::{
  Arc, OnceLock,
  atomic::{AtomicBool, Ordering},
};

use super::error::DOMExceptionName;

/// Set when a pending flush is cancelled, with the error it rejects with.
#[derive(Default)]
pub(crate) struct FlushAbort {
  aborted: AtomicBool,
  /// The error that closed the codec. Unset when close() or reset() aborted
  /// the flush.
  failure: OnceLock<DOMExceptionName>,
}

impl FlushAbort {
  pub(crate) fn is_aborted(&self) -> bool {
    self.aborted.load(Ordering::SeqCst)
  }

  /// The DOMException name and message an aborted flush rejects with.
  pub(crate) fn rejection(&self) -> (DOMExceptionName, &'static str) {
    match self.failure.get() {
      Some(name) => (*name, "The codec was closed by an error"),
      None => (DOMExceptionName::AbortError, "The operation was aborted"),
    }
  }
}

struct PendingFlush {
  id: u64,
  response_sender: Sender<Result<()>>,
  abort: Arc<FlushAbort>,
  buffering_outputs: bool,
}

/// Tracks flush operations independently so overlapping flushes cannot
/// overwrite each other's abort or output-delivery state.
#[derive(Default)]
pub(crate) struct FlushTracker {
  next_id: u64,
  pending: Vec<PendingFlush>,
  buffering_count: usize,
}

impl FlushTracker {
  pub(crate) fn register(
    &mut self,
    response_sender: Sender<Result<()>>,
    abort: Arc<FlushAbort>,
  ) -> u64 {
    let id = self.next_id;
    self.next_id = self.next_id.wrapping_add(1);
    self.pending.push(PendingFlush {
      id,
      response_sender,
      abort,
      buffering_outputs: true,
    });
    self.buffering_count += 1;
    id
  }

  pub(crate) fn finish(&mut self, id: u64) {
    if let Some(position) = self.pending.iter().position(|flush| flush.id == id) {
      let flush = self.pending.swap_remove(position);
      if flush.buffering_outputs {
        self.buffering_count = self.buffering_count.saturating_sub(1);
      }
    }
  }

  /// Stop routing new worker outputs into the batch this flush is about to
  /// deliver, while keeping the flush abortable until its callbacks finish.
  pub(crate) fn begin_output_delivery(&mut self, id: u64) {
    if let Some(flush) = self
      .pending
      .iter_mut()
      .find(|flush| flush.id == id && flush.buffering_outputs)
    {
      flush.buffering_outputs = false;
      self.buffering_count = self.buffering_count.saturating_sub(1);
    }
  }

  /// O(1) worker hot-path check. Later overlapping flushes can continue to
  /// buffer while an earlier flush is delivering callbacks on the JS thread.
  pub(crate) fn should_buffer_outputs(&self) -> bool {
    self.buffering_count != 0
  }

  /// close() or reset(): every pending flush rejects with AbortError.
  pub(crate) fn abort_all(&mut self) {
    self.cancel_all(None);
  }

  /// An error closed the codec: every pending flush rejects with `name`.
  pub(crate) fn fail_all(&mut self, name: DOMExceptionName) {
    self.cancel_all(Some(name));
  }

  fn cancel_all(&mut self, failure: Option<DOMExceptionName>) {
    self.buffering_count = 0;
    for flush in self.pending.drain(..) {
      if let Some(name) = failure {
        let _ = flush.abort.failure.set(name);
      }
      flush.abort.aborted.store(true, Ordering::SeqCst);
      let (name, message) = flush.abort.rejection();
      // The worker may already have filled this bounded channel. Never block
      // the main thread; the abort flag still makes the resolver reject if
      // the success response won the race.
      let _ = flush.response_sender.try_send(Err(Error::new(
        Status::GenericFailure,
        format!("{}: {}", name.as_str(), message),
      )));
    }
  }

  pub(crate) fn clear(&mut self) {
    self.pending.clear();
    self.buffering_count = 0;
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crossbeam::channel;

  #[test]
  fn tracks_and_finishes_each_flush() {
    let mut tracker = FlushTracker::default();
    let (sender_a, _receiver_a) = channel::bounded(1);
    let (sender_b, _receiver_b) = channel::bounded(1);
    let id_a = tracker.register(sender_a, Arc::new(FlushAbort::default()));
    let _id_b = tracker.register(sender_b, Arc::new(FlushAbort::default()));
    assert!(!tracker.pending.is_empty());
    tracker.finish(id_a);
    assert_eq!(tracker.pending.len(), 1);
  }

  #[test]
  fn aborts_every_pending_flush() {
    let mut tracker = FlushTracker::default();
    let (sender_a, receiver_a) = channel::bounded(1);
    let (sender_b, receiver_b) = channel::bounded(1);
    let flag_a = Arc::new(FlushAbort::default());
    let flag_b = Arc::new(FlushAbort::default());
    tracker.register(sender_a, flag_a.clone());
    tracker.register(sender_b, flag_b.clone());

    tracker.abort_all();

    assert!(flag_a.is_aborted());
    assert!(flag_b.is_aborted());
    assert_eq!(flag_a.rejection().0.as_str(), "AbortError");
    assert!(receiver_a.recv().unwrap().is_err());
    assert!(receiver_b.recv().unwrap().is_err());
    assert!(tracker.pending.is_empty());
  }

  #[test]
  fn fails_every_pending_flush_with_the_codec_error() {
    let mut tracker = FlushTracker::default();
    let (sender, receiver) = channel::bounded(1);
    let abort = Arc::new(FlushAbort::default());
    tracker.register(sender, abort.clone());

    tracker.fail_all(DOMExceptionName::NotSupportedError);

    assert!(abort.is_aborted());
    assert_eq!(abort.rejection().0.as_str(), "NotSupportedError");
    let error = receiver.recv().unwrap().unwrap_err();
    assert!(error.reason.starts_with("NotSupportedError:"));
    assert!(tracker.pending.is_empty());
  }

  #[test]
  fn stops_buffering_before_delivery_without_finishing_flush() {
    let mut tracker = FlushTracker::default();
    let (sender, _receiver) = channel::bounded(1);
    let id = tracker.register(sender, Arc::new(FlushAbort::default()));

    assert!(tracker.should_buffer_outputs());
    tracker.begin_output_delivery(id);
    assert!(!tracker.should_buffer_outputs());
    assert!(!tracker.pending.is_empty());

    tracker.finish(id);
    assert!(tracker.pending.is_empty());
  }

  #[test]
  fn later_overlapping_flush_keeps_buffering() {
    let mut tracker = FlushTracker::default();
    let (sender_a, _receiver_a) = channel::bounded(1);
    let (sender_b, _receiver_b) = channel::bounded(1);
    let id_a = tracker.register(sender_a, Arc::new(FlushAbort::default()));
    let id_b = tracker.register(sender_b, Arc::new(FlushAbort::default()));

    tracker.begin_output_delivery(id_a);
    assert!(tracker.should_buffer_outputs());
    tracker.begin_output_delivery(id_b);
    assert!(!tracker.should_buffer_outputs());
  }
}
