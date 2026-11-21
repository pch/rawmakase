//! What the mouse wheel and a trackpad pinch do over a photo, in Develop,
//! Reference View's reference and the Library's Loupe. Zoomed in, the wheel pans,
//! as Lightroom does on macOS: vertically, sideways with Shift or a sideways wheel,
//! and both ways with a trackpad. With Preferences > Display > Mouse wheel > Zoom
//! with set, a wheel's notches zoom in steps at the pointer instead, with the plain
//! wheel or with Cmd/Ctrl held. A sized tool (brushes, Remove, Red Eye) and the
//! magnifier keep the plain wheel; with wheel zoom on, Cmd/Ctrl and the wheel zoom
//! past them. A pinch zooms at the pointer, whatever the setting.
use crate::app::ui::wheel::{self, Kind, Owner, POINTS_PER_LINE, Target, Wheel, WheelZoom};
use crate::app::view::navigator::{self, Zoom};
use eframe::egui::{self, Pos2, Rect, Vec2};

/// What keeps the plain wheel over the photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Keeper {
    Nobody,
    /// A sized tool: the events go to its brush, spot or circle.
    Tool,
    /// The magnifier: the notches step its level.
    Magnifier,
}

/// What this frame's wheel does over the photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::app) struct PhotoWheel {
    /// The events the sized tool keeps.
    pub(in crate::app) tool: Vec<Wheel>,
    /// Magnifier levels up (positive) or down.
    pub(in crate::app) magnifier: i32,
    /// Zoom steps in (positive) or out.
    pub(in crate::app) zoom: i32,
    /// How far to move the view over the photo, in points: positive goes right
    /// and down the photo.
    pub(in crate::app) pan: Vec2,
}

/// This frame's wheel over the photo `owner` names, with the pointer over it.
/// `keeper` keeps the plain wheel; `zoomed` is whether the photo is zoomed in,
/// which the wheel pans; `crop` is the Crop tool open, where it neither zooms nor
/// pans (zooming would close it, as a click on the photo would).
pub(in crate::app) fn read(
    ctx: &egui::Context,
    owner: Owner,
    keeper: Keeper,
    zoomed: bool,
    crop: bool,
) -> PhotoWheel {
    let mut out = PhotoWheel::default();
    let events = ctx.input(wheel::read);
    if events.is_empty() {
        return out;
    }
    let Some(kind) = wheel::claim(ctx, owner, |_| true) else {
        return out;
    };
    let setting = wheel::prefs(ctx).zoom;
    let (mut zoom, mut magnifier) = (0., 0.);
    for w in events {
        let command = w.modifiers.command;
        // With wheel zoom on, Cmd/Ctrl + wheel zooms past the tools.
        let passes = setting != WheelZoom::Off && command;
        match keeper {
            Keeper::Tool if !passes => {
                out.tool.push(w);
                continue;
            }
            Keeper::Magnifier if !passes => {
                magnifier += w.lines();
                continue;
            }
            _ => {}
        }
        if crop {
            continue;
        }
        let zooms = kind == Kind::Notched
            && !w.modifiers.shift
            && w.notches.y != 0.
            && match setting {
                WheelZoom::Off => false,
                WheelZoom::Plain => true,
                WheelZoom::Command => command,
            };
        if zooms {
            zoom += w.notches.y;
        } else if zoomed {
            // Shift turns a vertical wheel sideways, as macOS does itself.
            let along = if w.modifiers.shift && w.notches.x == 0. {
                Vec2::new(w.notches.y, 0.)
            } else {
                w.notches
            };
            out.pan -= along * POINTS_PER_LINE;
        }
    }
    let zoom_target = match owner {
        Owner::Reference => Target::ReferenceZoom,
        _ => Target::Zoom,
    };
    if zoom != 0. {
        out.zoom = wheel::steps(ctx, zoom_target, zoom);
    }
    if magnifier != 0. {
        out.magnifier = wheel::steps(ctx, Target::Magnifier, magnifier);
    }
    out
}

/// The level `steps` zoom steps reach from `zoom`, for a photo that fits at `fit`
/// (see [`navigator::stepped`]); 0 is Fit.
pub(in crate::app) fn stepped(zoom: &Zoom, fit: f32, steps: i32) -> f32 {
    let mut level = if zoom.on { zoom.level } else { 0. };
    for _ in 0..steps.unsigned_abs() {
        let current = if level > 0. { level } else { fit };
        level = navigator::stepped(current, fit, steps > 0);
    }
    level
}

/// Zooms `zoom` to `level` (0 fits) keeping the point of the photo under `pos`
/// there, where the layout allows: the photo is drawn at `rect` in `area`, `size`
/// image pixels, `ppp` pixels per point.
pub(in crate::app) fn zoom_at(
    zoom: &mut Zoom,
    level: f32,
    pos: Pos2,
    rect: Rect,
    area: Rect,
    size: Vec2,
    ppp: f32,
) {
    if level <= 0. {
        zoom.set(0.);
        return;
    }
    let point = (pos - rect.min) / rect.size();
    let drawn = size * (level / ppp);
    let origin = pos - point * drawn;
    let pan = (area.center() - origin) / drawn;
    zoom.set(level);
    zoom.pan = [pan.x.clamp(0., 1.), pan.y.clamp(0., 1.)];
}

/// Moves `zoom`'s view `points` over a photo drawn `drawn` in size in a view
/// `view` in size, as far as the photo's edge: a scroll past it never leaves a
/// stretch to scroll back before the photo moves.
pub(in crate::app) fn pan(zoom: &mut Zoom, points: Vec2, drawn: Vec2, view: Vec2) {
    for i in 0..2 {
        let half = view[i] / 2. / drawn[i];
        let moved = zoom.pan[i] + points[i] / drawn[i];
        zoom.pan[i] = if half >= 0.5 {
            0.5
        } else {
            moved.clamp(half, 1. - half)
        };
    }
}

#[cfg(test)]
#[path = "photo_wheel_editor_tests.rs"]
mod editor_tests;
