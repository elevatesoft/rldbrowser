use std::cell::RefCell;
use std::ffi::c_void;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationPresentationOptions, NSPasteboard, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType,
};
use objc2_foundation::ns_string;
use rustbrowser_core::{LockdownBackend, LockdownError, LockdownGuard, LockdownPolicy};

/// `NSWindowAbove` (`NSWindowOrderingMode`) value from AppKit (`NSGraphics.h`).
const NS_WINDOW_ABOVE: isize = 1;

/// `NSBitmapImageFileTypeJPEG` value from AppKit (`NSBitmapImageRep.h`).
const NS_BITMAP_IMAGE_FILE_TYPE_JPEG: usize = 3;

/// Moves a native `NSView` (such as the HUD `WKWebView`) to the top of its superview's
/// subview list on macOS without removing or re-parenting it.
///
/// # Safety
/// `native_view` must be either null (no-op) or a valid pointer to an Objective-C `NSView`
/// instance, called on the main thread.
pub unsafe fn bring_view_to_front(native_view: *mut c_void) {
    if native_view.is_null() || MainThreadMarker::new().is_none() {
        return;
    }

    let view = native_view as *mut AnyObject;
    // SAFETY: Caller guarantees `native_view` is a valid `NSView` on the main thread.
    // Calling `[superview addSubview:view positioned:NSWindowAbove relativeTo:nil]` reorders
    // an existing subview to the very front of its superview in place without detaching it.
    unsafe {
        let superview: *mut AnyObject = msg_send![view, superview];
        if !superview.is_null() {
            let nil_view: *const AnyObject = std::ptr::null();
            let _: () = msg_send![
                superview,
                addSubview: view,
                positioned: NS_WINDOW_ABOVE,
                relativeTo: nil_view
            ];
        }
    }
}

/// Converts a non-null `NSImage*` into JPEG bytes using `CGImageForProposedRect:` +
/// `NSBitmapImageRep` (`initWithCGImage:` + `representationUsingType:properties:`).
///
/// # Safety
/// `image` must be a valid non-null `NSImage*` pointer, called within an active autorelease pool.
unsafe fn nsimage_to_jpeg(image: *mut AnyObject, quality: f64) -> Option<Vec<u8>> {
    let clamped_quality = if quality.is_finite() {
        quality.clamp(0.1, 0.95)
    } else {
        0.75
    };

    let rep_cls = AnyClass::get(c"NSBitmapImageRep")?;
    let ns_number_cls = AnyClass::get(c"NSNumber")?;
    let ns_dict_cls = AnyClass::get(c"NSDictionary")?;

    // SAFETY: Caller guarantees `image` is a valid `NSImage*`. The returned `CGImageRef`
    // follows the Objective-C Get rule (not owned by caller) and stays valid for the scope
    // of the enclosing autorelease pool / `NSImage`.
    let cg_image: *mut c_void = unsafe {
        let null_rect: *mut c_void = std::ptr::null_mut();
        let nil_ctx: *const AnyObject = std::ptr::null();
        let nil_hints: *const AnyObject = std::ptr::null();
        msg_send![
            image,
            CGImageForProposedRect: null_rect,
            context: nil_ctx,
            hints: nil_hints
        ]
    };
    if cg_image.is_null() {
        return None;
    }

    // SAFETY: `rep_cls` is `NSBitmapImageRep`; `alloc`/`initWithCGImage:` returns a retained
    // instance that `Retained` releases automatically on drop.
    let rep: Option<Retained<AnyObject>> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![rep_cls, alloc];
        msg_send![alloc, initWithCGImage: cg_image]
    };
    let rep = rep?;

    // SAFETY: `NSNumber`, `NSDictionary`, and `NSData` temporaries are autoreleased into the
    // caller's `autoreleasepool`.
    unsafe {
        let factor: *mut AnyObject = msg_send![ns_number_cls, numberWithDouble: clamped_quality];
        if factor.is_null() {
            return None;
        }
        let key = ns_string!("NSImageCompressionFactor");
        let props: *mut AnyObject =
            msg_send![ns_dict_cls, dictionaryWithObject: factor, forKey: key];
        if props.is_null() {
            return None;
        }

        let data: *mut AnyObject = msg_send![
            &*rep,
            representationUsingType: NS_BITMAP_IMAGE_FILE_TYPE_JPEG,
            properties: props
        ];
        if data.is_null() {
            return None;
        }

        let length: usize = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];
        if length == 0 || bytes.is_null() {
            return None;
        }

        Some(std::slice::from_raw_parts(bytes, length).to_vec())
    }
}

/// Captures a live snapshot of a native macOS `WKWebView` into JPEG bytes via
/// `-[WKWebView takeSnapshotWithConfiguration:completionHandler:]`.
///
/// # Safety
/// `wkwebview` must be either null (invokes `on_frame(None)`) or a valid `WKWebView*`
/// pointer, called on the main thread.
pub unsafe fn snapshot_wkwebview_jpeg<F>(
    wkwebview: *mut c_void,
    max_width: f64,
    quality: f64,
    on_frame: F,
) where
    F: FnOnce(Option<Vec<u8>>) + 'static,
{
    if wkwebview.is_null() || MainThreadMarker::new().is_none() {
        on_frame(None);
        return;
    }

    let Some(config_cls) = AnyClass::get(c"WKSnapshotConfiguration") else {
        on_frame(None);
        return;
    };

    // SAFETY: `config_cls` is `WKSnapshotConfiguration`; `alloc`/`init` returns a retained instance.
    let config: Option<Retained<AnyObject>> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![config_cls, alloc];
        msg_send![alloc, init]
    };
    let Some(config) = config else {
        on_frame(None);
        return;
    };

    let wv = wkwebview as *mut AnyObject;
    let callback = RefCell::new(Some(on_frame));
    let block = RcBlock::new(move |image: *mut AnyObject, error: *mut AnyObject| {
        let Some(on_frame) = callback.borrow_mut().take() else {
            return;
        };

        let frame = if error.is_null() && !image.is_null() {
            autoreleasepool(|_| {
                // SAFETY: WebKit invokes the completion handler with a valid `NSImage*` when
                // `error` is null and `image` is non-null.
                unsafe { nsimage_to_jpeg(image, quality) }
            })
        } else {
            None
        };

        on_frame(frame);
    });

    autoreleasepool(|_| {
        // SAFETY: `config` is a valid `WKSnapshotConfiguration` and `wv` is a valid `WKWebView`
        // on the main thread. Any autoreleased temporaries (`NSNumber`) are drained immediately.
        unsafe {
            if max_width.is_finite()
                && max_width > 0.0
                && let Some(ns_number_cls) = AnyClass::get(c"NSNumber")
            {
                let width_num: *mut AnyObject =
                    msg_send![ns_number_cls, numberWithDouble: max_width];
                if !width_num.is_null() {
                    let _: () = msg_send![&*config, setSnapshotWidth: width_num];
                }
            }

            let after_updates_sel = sel!(setAfterScreenUpdates:);
            let responds: Bool = msg_send![&*config, respondsToSelector: after_updates_sel];
            if responds.as_bool() {
                let _: () = msg_send![&*config, setAfterScreenUpdates: Bool::NO];
            }

            let _: () = msg_send![
                wv,
                takeSnapshotWithConfiguration: &*config,
                completionHandler: &*block
            ];
        }
    });
}

/// Elevated window level just above the macOS menu bar (`kCGMainMenuWindowLevel + 1`).
const KIOSK_WINDOW_LEVEL: isize = 25;

pub struct MacOsKioskBackend;

impl MacOsKioskBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacOsKioskBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Temporarily removes `NSApp.mainMenu` on the main thread so AppKit menu key equivalents
/// (`Cmd+Q`, `Cmd+H`, `Cmd+W`, `Cmd+M`) cannot be triggered while kiosk lockdown is active.
/// Returns the previously installed menu (if any) so it can be restored on exit.
fn strip_main_menu(app: &NSApplication, _mtm: MainThreadMarker) -> Option<Retained<AnyObject>> {
    autoreleasepool(|_| {
        // SAFETY: Called on the main thread with a valid `NSApplication` reference.
        // `mainMenu` returns either `nil` or the current `NSMenu*`, which `objc2` retains
        // into `Option<Retained<AnyObject>>`.
        let saved: Option<Retained<AnyObject>> = unsafe { msg_send![app, mainMenu] };
        if saved.is_some() {
            let nil_menu: *const AnyObject = std::ptr::null();
            // SAFETY: Setting `mainMenu` to `nil` on the main thread detaches AppKit's
            // main menu bar and disables its standard keyboard accelerators while locked.
            unsafe {
                let _: () = msg_send![app, setMainMenu: nil_menu];
            }
        }
        saved
    })
}

/// Restores a previously saved `NSMenu` onto `NSApp.mainMenu` on the main thread.
fn restore_main_menu(app: &NSApplication, menu: Retained<AnyObject>, _mtm: MainThreadMarker) {
    autoreleasepool(|_| {
        // SAFETY: Called on the main thread with a valid `NSApplication` reference and the
        // retained `NSMenu` previously captured from `NSApp.mainMenu`.
        unsafe {
            let _: () = msg_send![app, setMainMenu: &*menu];
        }
    });
}

struct MacOsLockdownGuard {
    previous_options: NSApplicationPresentationOptions,
    saved_main_menu: Option<Retained<AnyObject>>,
    clear_clipboard_on_exit: bool,
}

impl LockdownGuard for MacOsLockdownGuard {}

impl Drop for MacOsLockdownGuard {
    fn drop(&mut self) {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.setPresentationOptions(self.previous_options);
            if let Some(menu) = self.saved_main_menu.take() {
                restore_main_menu(&app, menu, mtm);
            }
            if self.clear_clipboard_on_exit {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
            }
        }
    }
}

impl LockdownBackend for MacOsKioskBackend {
    fn name(&self) -> &'static str {
        "macos-kiosk"
    }

    fn enter(&self, policy: &LockdownPolicy) -> Result<Box<dyn LockdownGuard>, LockdownError> {
        let mtm = MainThreadMarker::new().ok_or(LockdownError::NotOnMainThread)?;
        let app = NSApplication::sharedApplication(mtm);
        let previous_options = app.presentationOptions();

        let mut saved_main_menu = None;
        if policy.kiosk_mode {
            // AppKit requires HideDock + HideMenuBar when disabling process switching / force quit.
            let kiosk_options = NSApplicationPresentationOptions::HideDock
                | NSApplicationPresentationOptions::HideMenuBar
                | NSApplicationPresentationOptions::DisableProcessSwitching
                | NSApplicationPresentationOptions::DisableForceQuit
                | NSApplicationPresentationOptions::DisableSessionTermination
                | NSApplicationPresentationOptions::DisableHideApplication
                | NSApplicationPresentationOptions::DisableAppleMenu;

            app.setPresentationOptions(kiosk_options);
            saved_main_menu = strip_main_menu(&app, mtm);
        }

        if policy.block_clipboard {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
        }

        Ok(Box::new(MacOsLockdownGuard {
            previous_options,
            saved_main_menu,
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
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn bring_view_to_front_ignores_null_pointer() {
        // SAFETY: Null pointer is explicitly documented as a safe no-op.
        unsafe {
            bring_view_to_front(std::ptr::null_mut());
        }
    }

    #[test]
    fn snapshot_wkwebview_jpeg_handles_null_pointer() {
        let received = Arc::new(Mutex::new(Some(vec![1u8, 2, 3])));
        let slot = Arc::clone(&received);
        // SAFETY: Null pointer is explicitly documented to invoke `on_frame(None)` safely.
        unsafe {
            snapshot_wkwebview_jpeg(std::ptr::null_mut(), 1280.0, 0.75, move |frame| {
                *slot.lock().unwrap() = frame;
            });
        }
        assert!(received.lock().unwrap().is_none());
    }

    #[test]
    fn kiosk_backend_enter_off_main_thread_returns_error_without_panic() {
        let result = std::thread::spawn(|| {
            let backend = MacOsKioskBackend::new();
            let policy = LockdownPolicy::default();
            matches!(backend.enter(&policy), Err(LockdownError::NotOnMainThread))
        })
        .join()
        .expect("worker thread should not panic");
        assert!(result);
    }

    #[test]
    fn lockdown_guard_drop_without_main_menu_does_not_panic() {
        std::thread::spawn(|| {
            let guard = MacOsLockdownGuard {
                previous_options: NSApplicationPresentationOptions::Default,
                saved_main_menu: None,
                clear_clipboard_on_exit: false,
            };
            drop(guard);
        })
        .join()
        .expect("guard drop should not panic");
    }
}
