use std::ffi::c_void;

use thiserror::Error;

use crate::config::LockdownPolicy;

#[derive(Debug, Error)]
pub enum LockdownError {
    #[error("lockdown backend is not supported on this platform yet")]
    UnsupportedPlatform,
    #[error("lockdown must be entered from the application main thread")]
    NotOnMainThread,
    #[error("platform lockdown call failed: {0}")]
    PlatformFailure(String),
}

/// RAII guard representing an active OS lockdown session.
/// Dropping this guard MUST restore the system back to its prior non-locked state.
pub trait LockdownGuard {}

/// OS-specific lockdown implementation.
pub trait LockdownBackend {
    fn name(&self) -> &'static str;

    /// Engages OS-level kiosk/lockdown presentation and returns an RAII guard that
    /// restores the OS state on drop.
    fn enter(&self, policy: &LockdownPolicy) -> Result<Box<dyn LockdownGuard>, LockdownError>;

    /// Applies platform-specific window hardening (e.g., macOS NSWindowSharingNone,
    /// elevated window level, space behavior) to the native window handle.
    ///
    /// # Safety
    /// `native_window` must be a valid platform window pointer (e.g. `NSWindow*` on macOS)
    /// called on the UI main thread.
    unsafe fn configure_window(
        &self,
        native_window: *mut c_void,
        policy: &LockdownPolicy,
    ) -> Result<(), LockdownError>;

    /// Clears the system clipboard to prevent pre-copied exam material or exfiltration.
    fn clear_clipboard(&self) -> Result<(), LockdownError>;
}

/// Phase 2 seam: environment and integrity probes (VM, debugger, dylib injection, displays).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityFinding {
    pub probe: &'static str,
    pub severity: Severity,
    pub message: String,
}

pub trait SecurityProbe {
    fn name(&self) -> &'static str;
    fn check(&self) -> Vec<SecurityFinding>;
}
