use crossbeam::channel::Sender;
use napi::bindgen_prelude::{Error, Result, Status};
use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};

struct PendingFlush {
  id: u64,
  response_sender: Sender<Result<()>>,
  abort_flag: Arc<AtomicBool>,
}

/// Tracks flush operations independently so overlapping flushes cannot
/// overwrite each other's abort or output-delivery state.
#[derive(Default)]
pub(crate) struct FlushTracker {
  next_id: u64,
  pending: Vec<PendingFlush>,
}

impl FlushTracker {
  pub(crate) fn register(
    &mut self,
    response_sender: Sender<Result<()>>,
    abort_flag: Arc<AtomicBool>,
  ) -> u64 {
    let id = self.next_id;
    self.next_id = self.next_id.wrapping_add(1);
    self.pending.push(PendingFlush {
      id,
      response_sender,
      abort_flag,
    });
    id
  }

  pub(crate) fn finish(&mut self, id: u64) {
    self.pending.retain(|flush| flush.id != id);
  }

  pub(crate) fn is_active(&self) -> bool {
    !self.pending.is_empty()
  }

  pub(crate) fn abort_all(&mut self) {
    for flush in self.pending.drain(..) {
      flush.abort_flag.store(true, Ordering::SeqCst);
      // The worker may already have filled this bounded channel. Never block
      // reset() on the main thread; the abort flag still makes the resolver
      // reject if the success response won the race.
      let _ = flush.response_sender.try_send(Err(Error::new(
        Status::GenericFailure,
        "AbortError: The operation was aborted",
      )));
    }
  }

  pub(crate) fn clear(&mut self) {
    self.pending.clear();
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
    let id_a = tracker.register(sender_a, Arc::new(AtomicBool::new(false)));
    let _id_b = tracker.register(sender_b, Arc::new(AtomicBool::new(false)));
    assert!(tracker.is_active());
    tracker.finish(id_a);
    assert_eq!(tracker.pending.len(), 1);
  }

  #[test]
  fn aborts_every_pending_flush() {
    let mut tracker = FlushTracker::default();
    let (sender_a, receiver_a) = channel::bounded(1);
    let (sender_b, receiver_b) = channel::bounded(1);
    let flag_a = Arc::new(AtomicBool::new(false));
    let flag_b = Arc::new(AtomicBool::new(false));
    tracker.register(sender_a, flag_a.clone());
    tracker.register(sender_b, flag_b.clone());

    tracker.abort_all();

    assert!(flag_a.load(Ordering::SeqCst));
    assert!(flag_b.load(Ordering::SeqCst));
    assert!(receiver_a.recv().unwrap().is_err());
    assert!(receiver_b.recv().unwrap().is_err());
    assert!(!tracker.is_active());
  }
}
