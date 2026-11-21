//! The mouse wheel as the photo views and controls read it: each event in wheel
//! notches, with the modifiers held when it happened, and the rule that leaves the
//! wheel alone while another gesture is under way.
use eframe::egui;
use std::time::Duration;

/// Points of scrolling that count as one wheel notch.
pub(in crate::app) const POINTS_PER_LINE: f32 = 40.;
/// Lines a page of scrolling counts as.
const LINES_PER_PAGE: f32 = 10.;
/// Scrolling with pauses shorter than this is one gesture, one History step.
pub(in crate::app) const GESTURE_PAUSE: Duration = Duration::from_millis(400);

/// One wheel event: how far it scrolled, in notches on each axis (positive up and
/// left, as egui reports it), how the device reported it, and the modifiers held
/// when it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Wheel {
    pub(in crate::app) notches: egui::Vec2,
    pub(in crate::app) unit: egui::MouseWheelUnit,
    pub(in crate::app) modifiers: egui::Modifiers,
}
impl Wheel {
    /// The notches along the wheel: the vertical scroll, or the sideways one with
    /// Shift, which macOS turns a Shift+wheel into. A plain sideways scroll counts
    /// for nothing.
    pub(in crate::app) fn lines(self) -> f32 {
        if self.modifiers.shift && self.notches.y == 0. {
            self.notches.x
        } else {
            self.notches.y
        }
    }
}

/// Whether the wheel is left alone this frame: a button is held (a stroke, a
/// moved spot or correction), or the frame also has a click or a key, so a scroll
/// never mixes into another gesture or History step. A key a shortcut already
/// took counts too: the frame's raw input still has it.
pub(in crate::app) fn busy(i: &egui::InputState) -> bool {
    i.pointer.any_down()
        || i.raw.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::PointerButton { .. } | egui::Event::Key { pressed: true, .. }
            )
        })
}

/// This frame's wheel events, none while [`busy`].
pub(in crate::app) fn read(i: &egui::InputState) -> Vec<Wheel> {
    if busy(i) {
        return Vec::new();
    }
    i.events
        .iter()
        .filter_map(|event| {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                return None;
            };
            let notches = match unit {
                egui::MouseWheelUnit::Point => *delta / POINTS_PER_LINE,
                egui::MouseWheelUnit::Line => *delta,
                egui::MouseWheelUnit::Page => *delta * LINES_PER_PAGE,
            };
            Some(Wheel {
                notches,
                unit: *unit,
                modifiers: *modifiers,
            })
        })
        .collect()
}

/// Preferences > Display > Mouse wheel > Zoom with: whether a mouse wheel's
/// notches over the photo zoom it, and with which keys. Off by default, as in
/// Lightroom on macOS, where the wheel pans a zoomed photo and never zooms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WheelZoom {
    #[default]
    Off,
    /// The plain wheel zooms; Shift, a sideways wheel and trackpads pan.
    Plain,
    /// Cmd + wheel on macOS, Ctrl + wheel elsewhere.
    Command,
}

/// How a notched wheel is told apart from smooth scrolling (trackpads, momentum,
/// smoothing drivers), which never steps a slider or a zoom level. winit reports
/// them differently by platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum NotchRule {
    /// macOS and Wayland: a wheel arrives in lines or pages, possibly fractional
    /// from acceleration on macOS; devices with precise deltas arrive in points,
    /// and so does macOS's momentum.
    LineUnits,
    /// Windows and X11: every scroll arrives in lines, touchpads and their momentum
    /// as small fractional ones. A notch is a whole number of lines: winit divides
    /// the wheel's 120 per notch by 120, so the nearest other value is 119/120. A
    /// high-resolution mouse sending fractions of a notch counts as smooth.
    WholeLines,
}
impl NotchRule {
    /// The rule for the platform and display server the app runs on.
    pub(in crate::app) fn current() -> Self {
        if cfg!(target_os = "macos")
            || (cfg!(all(unix, not(target_os = "macos")))
                && crate::platform::wayland_in(|name| std::env::var_os(name)))
        {
            Self::LineUnits
        } else {
            Self::WholeLines
        }
    }
    fn kind(self, wheel: &Wheel) -> Kind {
        /// How close to a whole number of lines a notch is.
        const WHOLE: f32 = 1e-3;
        let whole = |v: f32| (v - v.round()).abs() < WHOLE;
        match (wheel.unit, self) {
            (egui::MouseWheelUnit::Point, _) => Kind::Smooth,
            (egui::MouseWheelUnit::Page, _) | (egui::MouseWheelUnit::Line, Self::LineUnits) => {
                Kind::Notched
            }
            (egui::MouseWheelUnit::Line, Self::WholeLines) => {
                if whole(wheel.notches.x) && whole(wheel.notches.y) {
                    Kind::Notched
                } else {
                    Kind::Smooth
                }
            }
        }
    }
}

/// A wheel gesture's device, decided once at its first event, so a touchpad delta
/// that happens to be whole mid-gesture never steps a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Kind {
    Notched,
    Smooth,
}

/// What a wheel gesture belongs to: what was under the pointer at its first event
/// and claimed it then. A gesture no one claimed is the panels' and lists' (egui's
/// own scrolling) until it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Owner {
    /// The photo being edited, or the Library Loupe's.
    Photo,
    /// Reference View's reference photo.
    Reference,
}

/// What whole notches step: each keeps the fraction of a notch scrolled towards
/// its next step until the gesture ends or another target takes the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Target {
    Zoom,
    ReferenceZoom,
    Magnifier,
}

/// The wheel preferences, published for the frame by the editor, so widgets drawn
/// by free functions read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Prefs {
    pub(in crate::app) zoom: WheelZoom,
    pub(in crate::app) rule: NotchRule,
}
impl Default for Prefs {
    fn default() -> Self {
        Self {
            zoom: WheelZoom::Off,
            rule: NotchRule::current(),
        }
    }
}

/// Seconds with no wheel event after which a gesture ends, whatever egui's
/// smoothing says.
const STALE: f64 = 2.;

/// The wheel gesture under way.
#[derive(Clone, Copy, Debug)]
struct Gesture {
    /// The pass its first event came in: only then can it be claimed.
    opened: u64,
    /// The input time of its last event, in seconds.
    last: f64,
    kind: Kind,
    owner: Option<Owner>,
    /// What the notches below are scrolled towards.
    target: Option<Target>,
    notches: f32,
}

/// The wheel's state between frames, in the context's memory.
#[derive(Clone, Copy, Debug, Default)]
struct Latch {
    prefs: Prefs,
    /// The pass last followed.
    pass: Option<u64>,
    gesture: Option<Gesture>,
}
fn latch_id() -> egui::Id {
    egui::Id::new("rawmakase-wheel")
}

/// Publishes the wheel preferences for this frame and follows the gesture under
/// way; the editor calls it before it draws anything.
pub(in crate::app) fn begin_frame(ctx: &egui::Context, prefs: Prefs) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Latch>(latch_id()).prefs = prefs);
    follow(ctx);
}
/// The wheel preferences this frame.
pub(in crate::app) fn prefs(ctx: &egui::Context) -> Prefs {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Latch>(latch_id()).prefs)
}
/// Follows this frame's wheel events, once a pass: a gesture opens with the first
/// one and ends once egui's smoothing has run out and the wheel paused for
/// [`GESTURE_PAUSE`]. While something claimed it, the scrolling egui smooths over
/// the next frames goes to nothing else, wherever the pointer goes.
fn follow(ctx: &egui::Context) -> Latch {
    let pass = ctx.cumulative_pass_nr();
    let mut latch = ctx.data_mut(|d| *d.get_temp_mut_or_default::<Latch>(latch_id()));
    if latch.pass == Some(pass) {
        return latch;
    }
    latch.pass = Some(pass);
    let (first, now, scrolling) = ctx.input(|i| {
        let first = i.raw.events.iter().find_map(|e| match e {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } => Some(Wheel {
                notches: *delta,
                unit: *unit,
                modifiers: *modifiers,
            }),
            _ => None,
        });
        (first, i.time, i.is_scrolling())
    });
    match (&mut latch.gesture, first) {
        (Some(g), Some(_)) => g.last = now,
        (Some(g), None) => {
            // egui's smoothing holds a touch until its End; a gesture with no event
            // for long ends even without one, so a lost End never keeps the panels
            // from scrolling.
            let quiet = now - g.last;
            if quiet >= GESTURE_PAUSE.as_secs_f64() && (!scrolling || quiet >= STALE) {
                latch.gesture = None;
            }
        }
        (None, Some(first)) => {
            latch.gesture = Some(Gesture {
                opened: pass,
                last: now,
                kind: latch.prefs.rule.kind(&first),
                owner: None,
                target: None,
                notches: 0.,
            });
        }
        (None, None) => {}
    }
    if let Some(g) = latch.gesture {
        if g.owner.is_some() {
            ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
        let left = GESTURE_PAUSE.as_secs_f64() - (now - g.last);
        ctx.request_repaint_after(Duration::from_secs_f64(left.max(0.)));
    }
    store(ctx, latch);
    latch
}
fn store(ctx: &egui::Context, latch: Latch) {
    ctx.data_mut(|d| d.insert_temp(latch_id(), latch));
}
/// Whether `owner` has this frame's wheel gesture: it claimed it at its first
/// event, or claims it now, when this frame opened it and `takes` its kind.
/// What holds the gesture keeps the scrolling egui smooths from the panels.
pub(in crate::app) fn claim(
    ctx: &egui::Context,
    owner: Owner,
    takes: impl FnOnce(Kind) -> bool,
) -> Option<Kind> {
    let mut latch = follow(ctx);
    let gesture = latch.gesture.as_mut()?;
    if gesture.owner.is_none() && gesture.opened == latch.pass? && takes(gesture.kind) {
        gesture.owner = Some(owner);
        ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        store(ctx, latch);
    }
    let gesture = latch.gesture?;
    (gesture.owner == Some(owner)).then_some(gesture.kind)
}
/// Whole steps `notches` more reach on `target`, with the fraction left kept
/// towards its next step; another target, or the other way, starts from none.
pub(in crate::app) fn steps(ctx: &egui::Context, target: Target, notches: f32) -> i32 {
    let mut latch = follow(ctx);
    let Some(gesture) = latch.gesture.as_mut() else {
        return notches.trunc() as i32;
    };
    // Turning the wheel back starts from none too: the fraction kept was towards
    // the other way.
    if gesture.target != Some(target) || gesture.notches * notches < 0. {
        gesture.target = Some(target);
        gesture.notches = 0.;
    }
    gesture.notches += notches;
    let whole = gesture.notches.trunc();
    gesture.notches -= whole;
    store(ctx, latch);
    whole as i32
}
/// The zoom factor a trackpad pinch asks for this frame (macOS alone reports
/// one), none while a button is held: a stroke or drag under way.
pub(in crate::app) fn pinch(i: &egui::InputState) -> Option<f32> {
    if i.pointer.any_down() {
        return None;
    }
    let factor: f32 = i
        .raw
        .events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Zoom(f) => Some(*f),
            _ => None,
        })
        .product();
    (factor != 1.).then_some(factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: egui::MouseWheelUnit, x: f32, y: f32) -> Wheel {
        Wheel {
            notches: egui::Vec2::new(x, y),
            unit,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Each platform's rule, whatever platform the tests run on.
    #[test]
    fn a_notched_wheel_is_told_apart_from_smooth_scrolling() {
        use egui::MouseWheelUnit::{Line, Page, Point};
        // macOS and Wayland: lines are a wheel's, fractions from acceleration too.
        let rule = NotchRule::LineUnits;
        assert_eq!(rule.kind(&wheel(Line, 0., 0.3)), Kind::Notched);
        assert_eq!(rule.kind(&wheel(Point, 0., 40.)), Kind::Smooth);
        assert_eq!(rule.kind(&wheel(Page, 0., 1.)), Kind::Notched);
        // Windows and X11: a whole number of lines is a notch, a fraction smooth;
        // 119/120 is the nearest a 120-per-notch wheel comes to one.
        let rule = NotchRule::WholeLines;
        assert_eq!(rule.kind(&wheel(Line, 0., -2.)), Kind::Notched);
        assert_eq!(rule.kind(&wheel(Line, 1., 0.)), Kind::Notched);
        assert_eq!(rule.kind(&wheel(Line, 0., 0.3)), Kind::Smooth);
        assert_eq!(rule.kind(&wheel(Line, 0., 119. / 120.)), Kind::Smooth);
        assert_eq!(rule.kind(&wheel(Point, 0., 40.)), Kind::Smooth);
    }

    #[test]
    fn a_pinch_zooms_on_stops_at_100_percent_and_never_goes_below_fit() {
        use crate::app::view::navigator::pinched;
        assert!((pinched(0.5, 0.5, 1.2) - 0.6).abs() < 1e-6);
        assert_eq!(pinched(0.9, 0.5, 1.2), 1.);
        assert!((pinched(1., 0.5, 1.2) - 1.2).abs() < 1e-6);
        assert_eq!(pinched(1.1, 0.5, 0.8), 1.);
        assert_eq!(pinched(0.6, 0.5, 0.5), 0.);
        // A photo smaller than the view, shown at 100% below its Fit, stays.
        assert_eq!(pinched(1., 2., 0.5), 1.);
        // At most 1600%.
        assert_eq!(pinched(15., 0.5, 2.), 16.);
    }
}
