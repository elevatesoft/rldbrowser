use std::ffi::c_void;

use rustbrowser_core::{LockdownBackend, LockdownError, LockdownGuard, LockdownPolicy};

pub struct StubLockdownBackend;

impl LockdownBackend for StubLockdownBackend {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn enter(&self, _policy: &LockdownPolicy) -> Result<Box<dyn LockdownGuard>, LockdownError> {
        Err(LockdownError::UnsupportedPlatform)
    }

    unsafe fn configure_window(
        &self,
        _native_window: *mut c_void,
        _policy: &LockdownPolicy,
    ) -> Result<(), LockdownError> {
        Err(LockdownError::UnsupportedPlatform)
    }

    fn clear_clipboard(&self) -> Result<(), LockdownError> {
        Err(LockdownError::UnsupportedPlatform)
    }
}
