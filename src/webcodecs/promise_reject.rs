use std::ptr;

use napi::{
  bindgen_prelude::{Env, PromiseRaw, Result},
  check_status, sys,
};

/// Reject a promise with a native TypeError
///
/// Creates a proper JavaScript TypeError instance and rejects the promise with it.
/// This produces errors that pass `instanceof TypeError` checks per W3C spec.
pub(crate) fn reject_with_type_error<'env, T>(
  env: &'env Env,
  message: &str,
) -> Result<PromiseRaw<'env, T>> {
  let mut deferred = ptr::null_mut();
  let mut promise = ptr::null_mut();

  check_status!(
    unsafe { sys::napi_create_promise(env.raw(), &mut deferred, &mut promise) },
    "Failed to create promise"
  )?;

  // Create JavaScript string for the message
  let mut js_message = ptr::null_mut();
  check_status!(
    unsafe {
      sys::napi_create_string_utf8(
        env.raw(),
        message.as_ptr().cast(),
        message.len() as isize,
        &mut js_message,
      )
    },
    "Failed to create message string"
  )?;

  // Create native TypeError with the message
  let mut type_error = ptr::null_mut();
  check_status!(
    unsafe { sys::napi_create_type_error(env.raw(), ptr::null_mut(), js_message, &mut type_error) },
    "Failed to create TypeError"
  )?;

  // Reject with the TypeError
  check_status!(
    unsafe { sys::napi_reject_deferred(env.raw(), deferred, type_error) },
    "Failed to reject promise"
  )?;

  Ok(PromiseRaw::new(env.raw(), promise))
}
