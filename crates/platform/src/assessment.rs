use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSPasteboard, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType};
use rustbrowser_core::{LockdownBackend, LockdownError, LockdownGuard, LockdownPolicy};

/// Elevated window level just above the macOS menu bar (`kCGMainMenuWindowLevel + 1`).
const KIOSK_WINDOW_LEVEL: isize = 25;

const ASSESSMENT_FRAMEWORK_PATH: &CStr =
    c"/System/Library/Frameworks/AutomaticAssessmentConfiguration.framework/AutomaticAssessmentConfiguration";

static FRAMEWORK_LOADED: OnceLock<bool> = OnceLock::new();

/// Dynamically loads `AutomaticAssessmentConfiguration.framework` at most once.
///
/// Using `dlopen` with `RTLD_LAZY | RTLD_LOCAL` ensures the binary never fails
/// to launch on macOS versions or environments where the framework is absent.
fn load_assessment_framework() -> bool {
    *FRAMEWORK_LOADED.get_or_init(|| {
        // SAFETY: `ASSESSMENT_FRAMEWORK_PATH` is a valid NUL-terminated C string.
        // We intentionally keep the handle open for the process lifetime so any
        // registered Objective-C classes remain valid.
        let handle = unsafe {
            libc::dlopen(
                ASSESSMENT_FRAMEWORK_PATH.as_ptr(),
                libc::RTLD_LAZY | libc::RTLD_LOCAL,
            )
        };
        !handle.is_null()
    })
}

macro_rules! set_bool_if_supported {
    ($config:expr, $setter:ident, $value:expr) => {{
        let selector = sel!($setter:);
        // SAFETY: `$config` is a valid `AEAssessmentConfiguration` instance and `$setter:`
        // is a `BOOL` property setter verified at runtime via `respondsToSelector:`.
        let responds: Bool = unsafe { msg_send![$config, respondsToSelector: selector] };
        if responds.as_bool() {
            let flag = if $value { Bool::YES } else { Bool::NO };
            let _: () = unsafe { msg_send![$config, $setter: flag] };
        }
    }};
}

/// macOS lockdown backend backed by Apple's `AEAssessmentSession`.
pub struct MacOsAssessmentBackend {
    allow_dictation: bool,
    allow_spell_check: bool,
}

impl MacOsAssessmentBackend {
    pub fn new() -> Self {
        Self::with_accommodations(false, false)
    }

    pub fn with_accommodations(allow_dictation: bool, allow_spell_check: bool) -> Self {
        Self {
            allow_dictation,
            allow_spell_check,
        }
    }

    /// Returns `true` if `AutomaticAssessmentConfiguration.framework` can be
    /// dynamically loaded and both `AEAssessmentSession` and
    /// `AEAssessmentConfiguration` Objective-C classes are available.
    pub fn is_framework_available() -> bool {
        load_assessment_framework()
            && AnyClass::get(c"AEAssessmentSession").is_some()
            && AnyClass::get(c"AEAssessmentConfiguration").is_some()
    }
}

impl Default for MacOsAssessmentBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct AssessmentLockdownGuard {
    session: Option<Retained<AnyObject>>,
    clear_clipboard_on_exit: bool,
}

impl LockdownGuard for AssessmentLockdownGuard {}

impl Drop for AssessmentLockdownGuard {
    fn drop(&mut self) {
        if MainThreadMarker::new().is_none() {
            return;
        }

        if let Some(session) = self.session.take() {
            // SAFETY: `session` is a retained `AEAssessmentSession` and we are on the main thread.
            unsafe {
                let _: () = msg_send![&*session, end];
            }
        }

        if self.clear_clipboard_on_exit {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
        }
    }
}

impl LockdownBackend for MacOsAssessmentBackend {
    fn name(&self) -> &'static str {
        "macos-assessment"
    }

    fn enter(&self, policy: &LockdownPolicy) -> Result<Box<dyn LockdownGuard>, LockdownError> {
        let _mtm = MainThreadMarker::new().ok_or(LockdownError::NotOnMainThread)?;

        if !policy.kiosk_mode {
            if policy.block_clipboard {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
            }
            return Ok(Box::new(AssessmentLockdownGuard {
                session: None,
                clear_clipboard_on_exit: policy.block_clipboard,
            }));
        }

        if !Self::is_framework_available() {
            return Err(LockdownError::PlatformFailure(
                "AutomaticAssessmentConfiguration framework is unavailable".to_string(),
            ));
        }

        let config_cls = AnyClass::get(c"AEAssessmentConfiguration").ok_or_else(|| {
            LockdownError::PlatformFailure(
                "AEAssessmentConfiguration class is unavailable".to_string(),
            )
        })?;
        let session_cls = AnyClass::get(c"AEAssessmentSession").ok_or_else(|| {
            LockdownError::PlatformFailure("AEAssessmentSession class is unavailable".to_string())
        })?;

        // SAFETY: `config_cls` is `AEAssessmentConfiguration`; `alloc`/`init` returns a retained instance.
        let config: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![config_cls, alloc];
            msg_send![alloc, init]
        };
        let config = config.ok_or_else(|| {
            LockdownError::PlatformFailure(
                "failed to initialize AEAssessmentConfiguration".to_string(),
            )
        })?;

        set_bool_if_supported!(&*config, setAllowsActivityContinuation, false);
        set_bool_if_supported!(&*config, setAllowsDictation, self.allow_dictation);
        set_bool_if_supported!(&*config, setAllowsKeyboardShortcuts, false);
        set_bool_if_supported!(&*config, setAllowsPasswordAutoFill, false);
        set_bool_if_supported!(&*config, setAllowsPredictiveKeyboard, false);
        set_bool_if_supported!(&*config, setAllowsSpellCheck, self.allow_spell_check);
        set_bool_if_supported!(&*config, setAllowsAutoCorrection, self.allow_spell_check);
        set_bool_if_supported!(&*config, setAllowsSmartPunctuation, false);

        // SAFETY: `session_cls` is `AEAssessmentSession`; `initWithConfiguration:` takes `&AnyObject`.
        let session: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![session_cls, alloc];
            msg_send![alloc, initWithConfiguration: &*config]
        };
        let session = session.ok_or_else(|| {
            LockdownError::PlatformFailure(
                "failed to initialize AEAssessmentSession".to_string(),
            )
        })?;

        // SAFETY: `session` is a valid `AEAssessmentSession` on the main thread.
        let active: Bool = unsafe {
            let _: () = msg_send![&*session, begin];
            msg_send![&*session, isActive]
        };

        if !active.as_bool() {
            return Err(LockdownError::PlatformFailure(
                "AEAssessmentSession unavailable or missing entitlement (com.apple.developer.automatic-assessment-configuration)".to_string(),
            ));
        }

        if policy.block_clipboard {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
        }

        Ok(Box::new(AssessmentLockdownGuard {
            session: Some(session),
            clear_clipboard_on_exit: policy.block_clipboard,
        }))
    }

    unsafe fn configure_window(
        &self,
        native_window: *mut c_void,
        policy: &LockdownPolicy,
    ) -> Result<(), LockdownError> {
        let _mtm = MainThreadMarker::new().ok_or(LockdownError::NotOnMainThread)?;
        if native_window.is_null() {
            return Err(LockdownError::PlatformFailure(
                "native NSWindow pointer is null".to_string(),
            ));
        }

        // SAFETY: Caller guarantees `native_window` is a valid `NSWindow*` on the main thread.
        let window = unsafe { &*(native_window as *const NSWindow) };

        if policy.block_screen_capture {
            window.setSharingType(NSWindowSharingType::None);
        }

        if policy.kiosk_mode {
            window.setLevel(KIOSK_WINDOW_LEVEL);
            let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary;
            window.setCollectionBehavior(behavior);
        }

        Ok(())
    }

    fn clear_clipboard(&self) -> Result<(), LockdownError> {
        let _mtm = MainThreadMarker::new().ok_or(LockdownError::NotOnMainThread)?;
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_macos_assessment() {
        assert_eq!(MacOsAssessmentBackend::new().name(), "macos-assessment");
    }

    #[test]
    fn framework_availability_check_does_not_panic() {
        let _available = MacOsAssessmentBackend::is_framework_available();
    }
}
