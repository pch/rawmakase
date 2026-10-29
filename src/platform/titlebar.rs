//! The macOS header handles each captured mouse-down once (#616).
//!
//! egui handles input after AppKit dispatches it. `currentEvent` can already
//! be a Force Touch pressure, drag or release event at that point. Capture
//! the original mouse-down for both its click count and native window dragging.
//! Zoom/Minimize have one explicit owner across the whole accepted header;
//! there is no native-strip split or change to NSWindow's movable state. Where
//! AppKit answers the double click itself (some macOS releases do), the header
//! does not act again: its action waits `NATIVE_GRACE` and is done only if the
//! window has not started changing.

/// Register this viewport's native window before drawing its header.
#[cfg(target_os = "macos")]
pub(crate) fn configure(
    window: &impl winit::raw_window_handle::HasWindowHandle,
    ctx: &eframe::egui::Context,
) {
    appkit::configure(window, ctx);
}

/// The header's drag area. Off macOS this is fastframe's own widget.
pub(crate) fn drag(ui: &mut eframe::egui::Ui, rect: eframe::egui::Rect) {
    #[cfg(not(target_os = "macos"))]
    fastframe_titlebar::drag(ui, rect);
    #[cfg(target_os = "macos")]
    {
        use eframe::egui;

        if ui.input(|input| input.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let response = ui.interact(
            rect,
            ui.id().with("titlebar-drag"),
            egui::Sense::click_and_drag(),
        );
        // Preserve egui's hit testing: controls and modals keep their input.
        // A quick click may be pressed and released within the same pass.
        if ui.input(|input| input.pointer.primary_pressed())
            && (response.is_pointer_button_down_on() || response.clicked())
        {
            appkit::press(ui.ctx(), ui.input(|input| input.pointer.primary_down()));
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Drag,
    Zoom,
    Minimize,
    /// Leave Fill to AppKit, as fastframe does; unknown settings do nothing.
    NativeOrNothing,
}

/// How long a double click's action waits for AppKit to answer it first.
#[cfg(target_os = "macos")]
const NATIVE_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

/// Whether AppKit answered a double click itself: the window's size, or whether
/// it is zoomed, changed since the press. On some macOS releases AppKit zooms a
/// window whose title bar the header covers, animating the zoom after the press;
/// the header's own Zoom would undo it.
#[cfg(any(target_os = "macos", test))]
fn answered_natively(at_press: ([f64; 2], bool), now: ([f64; 2], bool)) -> bool {
    at_press != now
}

/// `AppleActionOnDoubleClick` stores Zoom as `Maximize`. Without it, the
/// older `AppleMiniaturizeOnDoubleClick` switch decides, as in fastframe.
#[cfg(any(target_os = "macos", test))]
fn press(click_count: isize, action: Option<&str>, legacy_minimize: bool) -> Press {
    if click_count != 2 {
        return Press::Drag;
    }
    match action {
        Some("Maximize") => Press::Zoom,
        Some("Minimize") => Press::Minimize,
        Some(_) => Press::NativeOrNothing,
        None if legacy_minimize => Press::Minimize,
        None => Press::Zoom,
    }
}

#[cfg(target_os = "macos")]
mod appkit {
    use std::{cell::RefCell, collections::HashMap, ptr::NonNull};

    use block2::RcBlock;
    use objc2::{MainThreadMarker, Message, rc::Retained, sel};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventMask, NSEventType, NSView, NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectNSDelayedPerforming, NSString, NSUserDefaults};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::Press;

    /// Only registered header windows retain input. A newer press replaces an
    /// unhandled one; taking it prevents replay in another widget or egui pass.
    #[derive(Default)]
    struct MouseDowns {
        windows: HashMap<isize, Option<Retained<NSEvent>>>,
    }

    impl MouseDowns {
        fn record(&mut self, event: &NSEvent) {
            if event.r#type() == NSEventType::LeftMouseDown
                && let Some(pending) = self.windows.get_mut(&event.windowNumber())
            {
                *pending = Some(event.retain());
            }
        }

        fn take(&mut self, window: isize) -> Option<Retained<NSEvent>> {
            self.windows.get_mut(&window)?.take()
        }
    }

    thread_local! {
        static MONITOR_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static PRESSES: RefCell<MouseDowns> = RefCell::default();
        /// A double click's action waiting for AppKit to answer first: the window,
        /// the action, its size and zoom at the press, and when to act.
        static WAITING: std::cell::Cell<Option<Waiting>> = const { std::cell::Cell::new(None) };
    }

    #[derive(Clone, Copy)]
    struct Waiting {
        window: isize,
        zoom: bool,
        at_press: ([f64; 2], bool),
        due: std::time::Instant,
    }

    fn state(window: &objc2_app_kit::NSWindow) -> ([f64; 2], bool) {
        let size = window.frame().size;
        ([size.width, size.height], window.isZoomed())
    }

    /// Does what a double click waited for, unless AppKit answered it meanwhile.
    fn settle(ctx: &eframe::egui::Context, app: &NSApplication) {
        let Some(waiting) = WAITING.get() else {
            return;
        };
        let now = std::time::Instant::now();
        if now < waiting.due {
            ctx.request_repaint_after(waiting.due - now);
            return;
        }
        WAITING.set(None);
        let Some(window) = app.windowWithWindowNumber(waiting.window) else {
            return;
        };
        if super::answered_natively(waiting.at_press, state(&window)) || window.isMiniaturized() {
            return;
        }
        let selector = if waiting.zoom {
            sel!(performZoom:)
        } else {
            sel!(performMiniaturize:)
        };
        // SAFETY: these NSWindow selectors take one optional sender argument.
        unsafe { window.performSelector_withObject_afterDelay(selector, None, 0.0) };
    }

    fn window_id(ctx: &eframe::egui::Context) -> eframe::egui::Id {
        eframe::egui::Id::new("rawmakase-titlebar-window").with(ctx.viewport_id())
    }

    pub(super) fn configure(window: &impl HasWindowHandle, ctx: &eframe::egui::Context) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: eframe supplies a live NSView on its main-thread UI callback.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let Some(window) = view.window() else {
            return;
        };
        let number = window.windowNumber();
        let id = window_id(ctx);
        ctx.data_mut(|data| data.insert_temp(id, number));
        let app = NSApplication::sharedApplication(MainThreadMarker::from(&*window));
        PRESSES.with_borrow_mut(|presses| {
            // Do not retain a closed viewport's pending event indefinitely.
            presses
                .windows
                .retain(|number, _| app.windowWithWindowNumber(*number).is_some());
            presses.windows.entry(number).or_default();
        });
        settle(ctx, &app);
        if MONITOR_INSTALLED.replace(true) {
            return;
        }
        let block = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: AppKit passes a live event to the local monitor.
            PRESSES.with_borrow_mut(|presses| presses.record(unsafe { event.as_ref() }));
            event.as_ptr()
        });
        // SAFETY: return the original event unchanged. This monitor runs on
        // AppKit's main thread and never consumes input or starts window actions.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown,
                &block,
            )
        };
        // One application-lifetime monitor; AppKit retains its handler.
        std::mem::forget(monitor);
    }

    pub(super) fn press(ctx: &eframe::egui::Context, down: bool) {
        let id = window_id(ctx);
        let Some(number) = ctx.data(|data| data.get_temp::<isize>(id)) else {
            return;
        };
        let Some(event) = PRESSES.with_borrow_mut(|presses| presses.take(number)) else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = NSApplication::sharedApplication(mtm).windowWithWindowNumber(number)
        else {
            return;
        };
        if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return;
        }
        let defaults = NSUserDefaults::standardUserDefaults();
        let action = defaults
            .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
            .map(|value| value.to_string());
        let legacy_minimize =
            defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick"));
        let press = super::press(event.clickCount(), action.as_deref(), legacy_minimize);
        let zoom = match press {
            Press::Zoom => true,
            Press::Minimize => false,
            Press::Drag => {
                if down {
                    // AppKit requires the original mouse-down, not the current
                    // pressure/drag event winit's StartDrag would pass instead.
                    window.performWindowDragWithEvent(&event);
                }
                return;
            }
            Press::NativeOrNothing => return,
        };
        // AppKit may answer the double click itself (it does on some macOS
        // releases); acting too would undo its zoom. The action waits a moment and
        // is done only if the window has not started changing (`settle`).
        WAITING.set(Some(Waiting {
            window: number,
            zoom,
            at_press: state(&window),
            due: std::time::Instant::now() + super::NATIVE_GRACE,
        }));
        ctx.request_repaint_after(super::NATIVE_GRACE);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_app_kit::NSEventModifierFlags;
        use objc2_foundation::NSPoint;

        // Construct native values only: never post events or manipulate windows.
        fn event(kind: NSEventType, window: isize, clicks: isize) -> Retained<NSEvent> {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind, NSPoint::new(100., 10.), NSEventModifierFlags::empty(),
                1., window, None, 1, clicks, 0.,
            ).expect("mouse event")
        }

        #[test]
        fn later_events_do_not_replace_the_mouse_down_and_it_is_consumed_once() {
            let mut presses = MouseDowns::default();
            presses.windows.insert(7, None);
            presses.record(&event(NSEventType::LeftMouseDown, 7, 2));
            for kind in [
                NSEventType::LeftMouseDragged,
                NSEventType::MouseMoved,
                NSEventType::LeftMouseUp,
            ] {
                presses.record(&event(kind, 7, 0));
            }
            let saved = presses.take(7).unwrap();
            assert_eq!(saved.r#type(), NSEventType::LeftMouseDown);
            assert_eq!(
                super::super::press(saved.clickCount(), Some("Maximize"), false),
                Press::Zoom
            );
            assert!(
                presses.take(7).is_none(),
                "another egui pass must not repeat Zoom or drag"
            );
        }

        #[test]
        fn presses_are_scoped_to_registered_windows() {
            let mut presses = MouseDowns::default();
            presses.windows.insert(7, None);
            presses.windows.insert(8, None);
            presses.record(&event(NSEventType::LeftMouseDown, 7, 2));
            presses.record(&event(NSEventType::LeftMouseDown, 8, 1));
            presses.record(&event(NSEventType::LeftMouseDown, 9, 2));
            assert_eq!(presses.take(8).unwrap().clickCount(), 1);
            assert_eq!(presses.take(7).unwrap().clickCount(), 2);
            assert!(presses.take(9).is_none());
            assert_eq!(presses.windows.len(), 2);
        }

        #[test]
        fn a_new_press_replaces_unhandled_input() {
            let mut presses = MouseDowns::default();
            presses.windows.insert(7, None);
            presses.record(&event(NSEventType::LeftMouseDown, 7, 2));
            presses.record(&event(NSEventType::LeftMouseDown, 7, 1));
            let saved = presses.take(7).unwrap();
            assert_eq!(
                super::super::press(saved.clickCount(), Some("Maximize"), false),
                Press::Drag
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_second_press_is_a_double_click() {
        for clicks in [0, 1, 3, 4] {
            assert_eq!(press(clicks, Some("Maximize"), false), Press::Drag);
        }
        assert_eq!(press(2, Some("Maximize"), false), Press::Zoom);
    }

    #[test]
    fn a_double_click_appkit_already_zoomed_is_not_zoomed_again() {
        // Resizing, or zoomed, since the press: AppKit answered; ours would undo it.
        assert!(answered_natively(
            ([1440., 960.], false),
            ([1444., 963.], false)
        ));
        assert!(answered_natively(
            ([1440., 960.], false),
            ([1440., 960.], true)
        ));
        // Unchanged: the header zooms.
        assert!(!answered_natively(
            ([1440., 960.], false),
            ([1440., 960.], false)
        ));
    }

    #[test]
    fn the_system_setting_picks_the_double_clicks_action() {
        for (action, legacy, expected) in [
            (Some("Maximize"), true, Press::Zoom),
            (Some("Minimize"), false, Press::Minimize),
            (Some("Fill"), false, Press::NativeOrNothing),
            (Some("None"), false, Press::NativeOrNothing),
            (Some("unknown"), false, Press::NativeOrNothing),
            (None, false, Press::Zoom),
            (None, true, Press::Minimize),
        ] {
            assert_eq!(press(2, action, legacy), expected, "{action:?} {legacy}");
        }
    }
}
