//! On macOS, Quit (Cmd-Q and the app menu's item) closes the window as its close
//! button does, so the app's close guard runs: it saves the edit, and refuses while
//! an export or Sync Settings runs (see docs/shutdown.md). winit's menu sends
//! `terminate:`, which closes the window without asking it first. Quit from the Dock
//! and logging out send `terminate:` too; while work is pending, the app answers
//! them by closing the window the same way, so the guard asks first.
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether quitting now would cut off work: an export or Sync Settings running, or
/// an edit that failed to save. The editor sets it every frame; the app delegate
/// reads it when the system asks to quit, outside any frame.
static WORK_PENDING: AtomicBool = AtomicBool::new(false);

/// Records whether quitting now would cut off work (see [`WORK_PENDING`]).
pub(crate) fn set_work_pending(pending: bool) {
    WORK_PENDING.store(pending, Ordering::Relaxed);
}

/// Points the app menu's Quit item at `performClose:` on the window of `app`, the
/// app being created; the event loop has built the menu by then. The window is
/// the item's target, not the responder chain, so Quit still works while the
/// window is minimized and not key. Elsewhere it does nothing.
#[cfg(target_os = "macos")]
pub(crate) fn through_close_guard(app: &impl winit::raw_window_handle::HasWindowHandle) {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSView};
    use winit::raw_window_handle::RawWindowHandle;
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let Ok(handle) = app.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies a live NSView, and this runs on its main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let Some(window) = view.window() else {
        return;
    };
    let app = NSApplication::sharedApplication(main_thread);
    guard_termination(&app, &window);
    let Some(menu) = app.mainMenu() else {
        return;
    };
    for top in menu.itemArray() {
        let Some(submenu) = top.submenu() else {
            continue;
        };
        for item in submenu.itemArray() {
            if item.action() == Some(sel!(terminate:)) {
                // SAFETY: the window implements performClose:, outlives the menu
                // item (the app quits with it), and both are used on the main
                // thread.
                unsafe {
                    item.setTarget(Some(&window));
                    item.setAction(Some(sel!(performClose:)));
                }
            }
        }
    }
}

/// The window `terminate:` closes instead while work is pending, by its number.
#[cfg(target_os = "macos")]
static WINDOW_NUMBER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Gives winit's app delegate `applicationShouldTerminate:`, which winit leaves out,
/// so a `terminate:` from the Dock or a logout goes through the close guard while
/// work is pending. With nothing pending the app quits at once, as before.
#[cfg(target_os = "macos")]
fn guard_termination(app: &objc2_app_kit::NSApplication, window: &objc2_app_kit::NSWindow) {
    use objc2::{runtime::Imp, sel};
    WINDOW_NUMBER.store(window.windowNumber(), Ordering::Relaxed);
    let Some(delegate) = app.delegate() else {
        return;
    };
    let delegate: &objc2::runtime::AnyObject = delegate.as_ref();
    let class = delegate.class();
    type Reply = extern "C-unwind" fn(
        &objc2::runtime::AnyObject,
        objc2::runtime::Sel,
        &objc2_app_kit::NSApplication,
    ) -> objc2_app_kit::NSApplicationTerminateReply;
    let reply: Reply = should_terminate;
    // SAFETY: the method's types match its encoding ("Q@:@": an unsigned reply,
    // the receiver, the selector and the application), the class is winit's own
    // delegate class, which has no such method to replace, and this runs on the
    // main thread before the event loop asks for it.
    unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            sel!(applicationShouldTerminate:),
            std::mem::transmute::<Reply, Imp>(reply),
            c"Q@:@".as_ptr(),
        );
    }
}

/// What `applicationShouldTerminate:` answers, decided from plain values so
/// the choice is testable on every host, not just where the objc2 bindings
/// compile.
#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum TerminateChoice {
    /// Quit at once: nothing is pending, or there is no window to ask.
    Quit,
    /// Close the window so its guard asks first, and cancel this request.
    AskFirst,
}

#[cfg(any(test, target_os = "macos"))]
fn terminate_choice(work_pending: bool, has_window: bool) -> TerminateChoice {
    if work_pending && has_window {
        TerminateChoice::AskFirst
    } else {
        TerminateChoice::Quit
    }
}

/// `applicationShouldTerminate:` (see [`guard_termination`]).
#[cfg(target_os = "macos")]
extern "C-unwind" fn should_terminate(
    _delegate: &objc2::runtime::AnyObject,
    _command: objc2::runtime::Sel,
    app: &objc2_app_kit::NSApplication,
) -> objc2_app_kit::NSApplicationTerminateReply {
    use objc2_app_kit::NSApplicationTerminateReply;
    let work_pending = WORK_PENDING.load(Ordering::Relaxed);
    // Only looked up while pending, as before: no window is sought on the
    // fast path that quits at once.
    let window = if work_pending {
        app.windowWithWindowNumber(WINDOW_NUMBER.load(Ordering::Relaxed))
    } else {
        None
    };
    match terminate_choice(work_pending, window.is_some()) {
        TerminateChoice::AskFirst => {
            // The choice only asks while the window exists.
            let Some(window) = window else {
                return NSApplicationTerminateReply::TerminateNow;
            };
            // The question must be seen: the app may be hidden or behind others,
            // as when quitting from the Dock or logging out.
            app.unhide(None);
            // `activate` needs macOS 14; RAWmakase supports macOS 11.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            // The close guard asks, restoring a minimized window to show its question.
            window.performClose(None);
            NSApplicationTerminateReply::TerminateCancel
        }
        TerminateChoice::Quit => NSApplicationTerminateReply::TerminateNow,
    }
}

/// Elsewhere quitting already closes the window first.
#[cfg(not(target_os = "macos"))]
pub(crate) fn through_close_guard(_app: &impl winit::raw_window_handle::HasWindowHandle) {}

#[cfg(test)]
mod tests {
    use super::{TerminateChoice, terminate_choice};

    #[test]
    fn nothing_pending_quits_at_once() {
        assert_eq!(terminate_choice(false, true), TerminateChoice::Quit);
        assert_eq!(terminate_choice(false, false), TerminateChoice::Quit);
    }

    #[test]
    fn pending_work_asks_through_the_window() {
        assert_eq!(terminate_choice(true, true), TerminateChoice::AskFirst);
    }

    #[test]
    fn pending_work_without_a_window_still_quits() {
        assert_eq!(terminate_choice(true, false), TerminateChoice::Quit);
    }
}
