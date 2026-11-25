use std::ffi::c_void;

use rustbrowser_core::{LockdownBackend, LockdownError, LockdownGuard, LockdownPolicy};

#[cfg(target_os = "macos")]
pub mod assessment;
#[cfg(target_os = "macos")]
mod macos;
pub mod probes;

#[cfg(not(target_os = "macos"))]
mod stub;

pub use probes::{default_probes, deny_debugger_attach};

/// Moves a native child view (e.g., the HUD `WKWebView` on macOS) to the top of its
/// parent view's z-order so overlays/popovers always render above dynamically added tabs.
///
/// # Safety
/// On macOS, `native_view` must be either null (no-op) or a valid `NSView` pointer
/// called on the main thread. On non-macOS targets, this is a no-op.
pub unsafe fn bring_view_to_front(native_view: *mut c_void) {
    #[cfg(target_os = "macos")]
    {
        unsafe { macos::bring_view_to_front(native_view) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = native_view;
    }
}

/// Captures a live snapshot of a native macOS `WKWebView` and encodes it as JPEG bytes.
///
/// On non-macOS targets, this immediately invokes `on_frame(None)`.
///
/// # Safety
/// On macOS, `wkwebview` must be either null (invokes `on_frame(None)`) or a valid
/// `WKWebView` pointer, and this function must be called on the main thread.
pub unsafe fn snapshot_wkwebview_jpeg<F>(
    wkwebview: *mut std::ffi::c_void,
    max_width: f64,
    quality: f64,
    on_frame: F,
) where
    F: FnOnce(Option<Vec<u8>>) + 'static,
{
    #[cfg(target_os = "macos")]
    {
        unsafe { macos::snapshot_wkwebview_jpeg(wkwebview, max_width, quality, on_frame) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (wkwebview, max_width, quality);
        on_frame(None);
    }
}

/// Returns the default OS lockdown backend for the current platform (preferring
/// Apple's `AEAssessmentSession` on macOS with automatic fallback to Kiosk mode).
pub fn default_backend() -> Box<dyn LockdownBackend> {
    select_backend(true)
}

/// Returns the appropriate lockdown backend for the current platform.
/// On macOS, when `prefer_assessment` is true, tries `AEAssessmentSession` first
/// and falls back cleanly to `NSApplicationPresentationOptions` Kiosk mode if the
/// Apple entitlement is unavailable.
pub fn select_backend(prefer_assessment: bool) -> Box<dyn LockdownBackend> {
    select_backend_with_accommodations(prefer_assessment, false, false)
}

/// Returns the appropriate lockdown backend for the current platform with optional
/// OS-level accessibility accommodations (dictation & spell check for `AEAssessmentSession`).
pub fn select_backend_with_accommodations(
    prefer_assessment: bool,
    allow_dictation: bool,
    allow_spell_check: bool,
) -> Box<dyn LockdownBackend> {
    #[cfg(target_os = "macos")]
    {
        if prefer_assessment {
            Box::new(MacOsAutoBackend::with_accommodations(
                allow_dictation,
                allow_spell_check,
            ))
        } else {
            Box::new(macos::MacOsKioskBackend::new())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (prefer_assessment, allow_dictation, allow_spell_check);
        Box::new(stub::StubLockdownBackend)
    }
}

#[cfg(target_os = "macos")]
pub struct MacOsAutoBackend {
    assessment: assessment::MacOsAssessmentBackend,
    kiosk: macos::MacOsKioskBackend,
}

#[cfg(target_os = "macos")]
impl MacOsAutoBackend {
    pub fn new() -> Self {
        Self::with_accommodations(false, false)
    }

    pub fn with_accommodations(allow_dictation: bool, allow_spell_check: bool) -> Self {
        Self {
            assessment: assessment::MacOsAssessmentBackend::with_accommodations(
                allow_dictation,
                allow_spell_check,
            ),
            kiosk: macos::MacOsKioskBackend::new(),
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for MacOsAutoBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl LockdownBackend for MacOsAutoBackend {
    fn name(&self) -> &'static str {
        "macos-auto(assessment->kiosk)"
    }

    fn enter(&self, policy: &LockdownPolicy) -> Result<Box<dyn LockdownGuard>, LockdownError> {
        if policy.kiosk_mode && assessment::MacOsAssessmentBackend::is_framework_available() {
            match self.assessment.enter(policy) {
                Ok(guard) => return Ok(guard),
                Err(_err) => {
                    // Missing entitlement or unsupported environment: fall back cleanly to Kiosk mode.
                }
            }
        }
        self.kiosk.enter(policy)
    }

    unsafe fn configure_window(
        &self,
        native_window: *mut c_void,
        policy: &LockdownPolicy,
    ) -> Result<(), LockdownError> {
        unsafe { self.kiosk.configure_window(native_window, policy) }
    }

    fn clear_clipboard(&self) -> Result<(), LockdownError> {
        self.kiosk.clear_clipboard()
    }
}
