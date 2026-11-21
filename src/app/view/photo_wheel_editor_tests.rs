//! Editor tests of `photo_wheel`: Develop's viewport driven through its frames with
//! the wheel and pinch events winit sends.
use crate::app::develop::state::Tool;
use crate::app::ui::wheel::{NotchRule, Prefs, WheelZoom};
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

/// A 400 × 400 photo in a 200 × 200 view: it fits at 0.5.
fn editor(ctx: &egui::Context) -> Editor {
    let mut editor = Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.1; 3]; 160_000].into(),
        metadata: Metadata {
            width: 400,
            height: 400,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor
}

/// Frames of Develop's viewport, each a tenth of a second after the last.
struct Frames {
    ctx: egui::Context,
    editor: Editor,
    prefs: Prefs,
    time: f64,
}
impl Frames {
    fn new(zoom: WheelZoom, rule: NotchRule) -> Self {
        let ctx = egui::Context::default();
        let editor = editor(&ctx);
        let mut frames = Self {
            ctx,
            editor,
            prefs: Prefs { zoom, rule },
            time: 0.,
        };
        frames.frame(vec![egui::Event::PointerMoved(Pos2::new(100., 100.))]);
        frames
    }
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.1;
        let (editor, prefs) = (&mut self.editor, self.prefs);
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                crate::app::ui::wheel::begin_frame(ui.ctx(), prefs);
                editor.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
    }
    /// The gesture ends: half a second of frames with no wheel.
    fn pause(&mut self) {
        for _ in 0..6 {
            self.frame(vec![]);
        }
    }
    /// Where the point of the photo under `pos` lies on it, as fractions of it.
    fn under(&self, pos: Pos2) -> Vec2 {
        let rect = self.editor.view.zoom.photo_rect(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.)),
            Vec2::splat(400.),
            1.,
        );
        (pos - rect.min) / rect.size()
    }
}
fn wheel(unit: egui::MouseWheelUnit, x: f32, y: f32, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::MouseWheel {
        unit,
        delta: Vec2::new(x, y),
        phase: egui::TouchPhase::Move,
        modifiers,
    }
}
fn line(y: f32) -> egui::Event {
    wheel(egui::MouseWheelUnit::Line, 0., y, egui::Modifiers::NONE)
}
/// How closely the point under the pointer stays there, as a fraction of the
/// photo: about a pixel, as a zoomed-in photo is drawn on whole pixels.
const ANCHOR: f32 = 3e-3;
const LINE: egui::MouseWheelUnit = egui::MouseWheelUnit::Line;
const POINT: egui::MouseWheelUnit = egui::MouseWheelUnit::Point;

#[test]
fn the_wheel_pans_a_zoomed_photo_and_leaves_a_fitted_one() {
    let mut f = Frames::new(WheelZoom::Off, NotchRule::LineUnits);
    let recipe = f.editor.document.edit.recipe().clone();
    // At Fit the plain wheel does nothing, as in Lightroom.
    f.frame(vec![line(-1.)]);
    assert!(!f.editor.view.zoom.on);
    f.pause();
    f.editor.view.zoom.set(1.);
    f.frame(vec![]);
    let start = f.editor.view.zoom.pan;
    // Rolling down moves down the photo: 40 points a notch, of 400 drawn.
    f.frame(vec![line(-1.)]);
    let pan = f.editor.view.zoom.pan;
    assert!((pan[1] - start[1] - 0.1).abs() < 1e-4, "{pan:?}");
    assert_eq!(pan[0], start[0]);
    // Shift turns it sideways (Windows keeps it vertical; macOS sends it sideways),
    // and so does a sideways wheel.
    f.frame(vec![wheel(LINE, 0., -0.5, egui::Modifiers::SHIFT)]);
    f.frame(vec![wheel(LINE, -0.5, 0., egui::Modifiers::SHIFT)]);
    f.frame(vec![wheel(LINE, -0.5, 0., egui::Modifiers::NONE)]);
    let sideways = f.editor.view.zoom.pan;
    assert!((sideways[0] - start[0] - 0.15).abs() < 1e-4, "{sideways:?}");
    assert_eq!(sideways[1], pan[1]);
    // A trackpad pans both ways, by the points it scrolled, momentum and all.
    f.pause();
    for phase in [
        egui::TouchPhase::Start,
        egui::TouchPhase::Move,
        egui::TouchPhase::End,
    ] {
        f.frame(vec![egui::Event::MouseWheel {
            unit: POINT,
            delta: Vec2::new(10., 20.),
            phase,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    let trackpad = f.editor.view.zoom.pan;
    assert!(
        (sideways[0] - trackpad[0] - 0.075).abs() < 1e-4,
        "{trackpad:?}"
    );
    assert!(
        (sideways[1] - trackpad[1] - 0.15).abs() < 1e-4,
        "{trackpad:?}"
    );
    // And a Windows touchpad's small fractions of a line pan as far as they add up.
    f.pause();
    f.prefs.rule = NotchRule::WholeLines;
    for _ in 0..4 {
        f.frame(vec![line(-0.25)]);
    }
    assert!((f.editor.view.zoom.pan[1] - trackpad[1] - 0.1).abs() < 1e-4);
    // It stops where the photo's edge meets the view's: the view is half as wide
    // as the photo drawn, so its centre stays within a quarter of either edge.
    for _ in 0..10 {
        f.frame(vec![line(-1.)]);
    }
    assert_eq!(f.editor.view.zoom.pan[1], 0.75);
    f.frame(vec![line(1.)]);
    assert!((f.editor.view.zoom.pan[1] - 0.65).abs() < 1e-4);
    // Panning is a view change, never an edit.
    assert_eq!(*f.editor.document.edit.recipe(), recipe);
    assert!(f.editor.view.zoom.on);
}

#[test]
fn wheel_zoom_steps_at_the_pointer_and_stops_at_100_percent_and_fit() {
    let mut f = Frames::new(WheelZoom::Plain, NotchRule::LineUnits);
    let at = Pos2::new(150., 50.);
    f.frame(vec![egui::Event::PointerMoved(at)]);
    let point = f.under(at);
    // One notch up zooms in a step from Fit, at once, keeping the point under the
    // pointer; the render waits for a pause.
    f.frame(vec![line(1.)]);
    let zoom = f.editor.view.zoom;
    assert!(zoom.on && (zoom.level - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-4);
    assert!(f.editor.view.zoom_anim.is_none());
    assert!(f.editor.view.zoom_render_at.is_some());
    let under = f.under(at);
    assert!((under - point).length() < ANCHOR, "{under:?} for {point:?}");
    // A macOS wheel's accelerated fractions of a notch step once they add up to one;
    // the step across 100% stops there.
    for _ in 0..2 {
        f.frame(vec![line(0.45)]);
    }
    assert!((f.editor.view.zoom.level - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-4);
    f.frame(vec![line(0.45)]);
    assert!((f.editor.view.zoom.level - 1.).abs() < 1e-4);
    let under = f.under(at);
    assert!((under - point).length() < ANCHOR, "{under:?} for {point:?}");
    // Down again it stops at Fit, never below.
    f.frame(vec![line(-5.)]);
    assert!(!f.editor.view.zoom.on);
    f.pause();
    assert!(f.editor.view.zoom_render_at.is_none());
    // A trackpad never zooms, nor a Windows touchpad's fractions of a line.
    f.frame(vec![wheel(POINT, 0., 120., egui::Modifiers::NONE)]);
    assert!(!f.editor.view.zoom.on);
    f.pause();
    f.prefs.rule = NotchRule::WholeLines;
    for _ in 0..6 {
        f.frame(vec![line(0.3)]);
    }
    assert!(!f.editor.view.zoom.on);
    f.pause();
    // A Windows wheel's whole notches do.
    f.frame(vec![line(1.)]);
    assert!(f.editor.view.zoom.on);
}

#[test]
fn cmd_wheel_zooms_and_the_plain_wheel_pans_with_the_command_choice() {
    let mut f = Frames::new(WheelZoom::Command, NotchRule::LineUnits);
    f.frame(vec![line(1.)]);
    assert!(!f.editor.view.zoom.on);
    f.frame(vec![wheel(LINE, 0., 1., egui::Modifiers::COMMAND)]);
    assert!(f.editor.view.zoom.on);
    let pan = f.editor.view.zoom.pan;
    f.pause();
    f.frame(vec![line(-1.)]);
    assert!(f.editor.view.zoom.pan[1] > pan[1]);
    // With Off, Cmd + wheel does nothing to the photo.
    f.pause();
    f.prefs.zoom = WheelZoom::Off;
    let zoom = f.editor.view.zoom;
    f.frame(vec![wheel(LINE, 0., 1., egui::Modifiers::COMMAND)]);
    assert_eq!(f.editor.view.zoom.level, zoom.level);
}

#[test]
fn sized_tools_and_the_magnifier_keep_the_wheel_and_cmd_wheel_zooms_past_them() {
    let mut f = Frames::new(WheelZoom::Plain, NotchRule::LineUnits);
    f.editor.view.set_tool(Tool::Remove);
    let size = f.editor.view.retouch.size;
    f.frame(vec![line(1.)]);
    assert!(f.editor.view.retouch.size > size);
    assert!(!f.editor.view.zoom.on);
    // Cmd + wheel zooms rather than sizing the brush, with wheel zoom on.
    let size = f.editor.view.retouch.size;
    f.pause();
    f.frame(vec![wheel(LINE, 0., 1., egui::Modifiers::COMMAND)]);
    assert_eq!(f.editor.view.retouch.size, size);
    assert!(f.editor.view.zoom.on);
    // With it off, Cmd + wheel sizes the brush as before.
    f.pause();
    f.editor.view.zoom.set(0.);
    f.prefs.zoom = WheelZoom::Off;
    f.frame(vec![wheel(LINE, 0., 1., egui::Modifiers::COMMAND)]);
    assert!(f.editor.view.retouch.size > size);
    assert!(!f.editor.view.zoom.on);
    // While Space holds the Hand tool the brush leaves the wheel alone.
    f.pause();
    f.prefs.zoom = WheelZoom::Plain;
    let size = f.editor.view.retouch.size;
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    f.frame(vec![space(true)]);
    f.frame(vec![line(1.)]);
    f.frame(vec![space(false)]);
    assert_eq!(f.editor.view.retouch.size, size);
    assert!(f.editor.view.zoom.on);

    // The magnifier steps a level a whole notch, keeping the rest of one, and as
    // many levels as the notches of a frame reach.
    f.pause();
    f.editor.view.set_tool(Tool::None);
    f.editor.view.zoom.set(0.);
    f.editor.view.magnifier.on = true;
    // Once the zoom out has eased to Fit.
    f.pause();
    f.frame(vec![line(0.6)]);
    assert_eq!(f.editor.view.magnifier.level(), 1.);
    f.frame(vec![line(0.6)]);
    assert_eq!(f.editor.view.magnifier.level(), 2.);
    f.frame(vec![line(-2.)]);
    assert_eq!(f.editor.view.magnifier.level(), 1.);
    assert!(!f.editor.view.zoom.on);
    // The rest of a notch goes with the gesture.
    f.pause();
    f.frame(vec![line(0.6)]);
    assert_eq!(f.editor.view.magnifier.level(), 1.);
    // Cmd + wheel zooms past it.
    f.pause();
    f.frame(vec![wheel(LINE, 0., 1., egui::Modifiers::COMMAND)]);
    assert!(f.editor.view.zoom.on);
    assert_eq!(f.editor.view.magnifier.level(), 1.);
}

#[test]
fn the_wheel_and_a_pinch_neither_zoom_nor_pan_in_crop() {
    let mut f = Frames::new(WheelZoom::Plain, NotchRule::LineUnits);
    f.editor.view.set_tool(Tool::Crop);
    f.frame(vec![]);
    f.frame(vec![line(1.)]);
    f.frame(vec![egui::Event::Zoom(1.5)]);
    assert!(!f.editor.view.zoom.on);
    assert!(f.editor.view.is(Tool::Crop));
    // Zoomed in with Crop open, the wheel doesn't pan either.
    f.pause();
    f.prefs.zoom = WheelZoom::Off;
    f.editor.view.zoom.set(1.);
    let pan = f.editor.view.zoom.pan;
    f.frame(vec![line(-1.)]);
    assert_eq!(f.editor.view.zoom.pan, pan);
}

#[test]
fn a_pinch_zooms_at_the_pointer_but_not_while_a_button_is_held() {
    let mut f = Frames::new(WheelZoom::Off, NotchRule::LineUnits);
    let at = Pos2::new(60., 140.);
    f.frame(vec![egui::Event::PointerMoved(at)]);
    let point = f.under(at);
    f.frame(vec![egui::Event::Zoom(1.5)]);
    let zoom = f.editor.view.zoom;
    assert!(zoom.on && (zoom.level - 0.75).abs() < 1e-4, "{zoom:?}");
    let under = f.under(at);
    assert!((under - point).length() < ANCHOR, "{under:?} for {point:?}");
    // Across 100% it stops there.
    f.frame(vec![egui::Event::Zoom(1.5)]);
    assert_eq!(f.editor.view.zoom.level, 1.);
    // Out, it stops at Fit.
    f.frame(vec![egui::Event::Zoom(0.25)]);
    assert!(!f.editor.view.zoom.on);
    // A button held (a stroke, a drag) keeps the zoom.
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    f.frame(vec![press(true)]);
    f.frame(vec![egui::Event::Zoom(2.)]);
    assert!(!f.editor.view.zoom.on);
    f.frame(vec![press(false)]);
}

#[test]
fn in_two_up_the_wheel_over_before_zooms_at_the_same_point_of_the_edit() {
    use crate::app::develop::before_after::{Axis, Compare};
    let mut f = Frames::new(WheelZoom::Plain, NotchRule::LineUnits);
    f.editor.view.compare = Compare::SideBySide(Axis::LeftRight);
    f.frame(vec![]);
    let panes = crate::app::develop::before_after::panes(
        f.editor.view.compare,
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.)),
    );
    let before = panes.before.unwrap();
    // A point of Before's photo, and the same point of the edit.
    let at = before.area.center() + Vec2::new(10., 15.);
    f.frame(vec![egui::Event::PointerMoved(at)]);
    let after = panes.after.area;
    let fitted = |area: Rect| {
        let k = (area.width() / 400.).min(area.height() / 400.);
        Rect::from_center_size(area.center(), Vec2::splat(400. * k))
    };
    let point = (at - fitted(before.area).min) / fitted(before.area).size();
    let mapped = fitted(after).min + point * fitted(after).size();
    // Three steps, so the photo covers the pane both ways and nothing holds it.
    f.frame(vec![line(3.)]);
    assert!(f.editor.view.zoom.on);
    let rect = f.editor.view.zoom.photo_rect(after, Vec2::splat(400.), 1.);
    let under = (mapped - rect.min) / rect.size();
    assert!((under - point).length() < ANCHOR, "{under:?} for {point:?}");
}

#[test]
fn a_wheel_gesture_stays_with_what_it_started_over() {
    // The photo beside a panel that scrolls, as Develop lays them out.
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    let prefs = Prefs {
        zoom: WheelZoom::Off,
        rule: NotchRule::LineUnits,
    };
    let panel = std::cell::Cell::new(0.);
    let mut time = 0.;
    let mut frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        time += 0.05;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 200.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                crate::app::ui::wheel::begin_frame(ui.ctx(), prefs);
                egui::Panel::left("panel").exact_size(200.).show(ui, |ui| {
                    let out = egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.allocate_space(Vec2::new(150., 2000.));
                    });
                    panel.set(out.state.offset.y);
                });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        editor.viewport_ui(ui);
                    });
            },
        );
        output.textures_delta.clear();
    };
    let photo = Pos2::new(300., 100.);
    let side = Pos2::new(100., 100.);
    editor.view.zoom.set(1.);
    frame(&mut editor, vec![egui::Event::PointerMoved(photo)]);
    frame(&mut editor, vec![]);
    // Scrolling over the photo pans it; drifting over the panel mid-gesture, the
    // panel stays put and the photo too.
    frame(&mut editor, vec![line(-1.)]);
    let pan = editor.view.zoom.pan;
    frame(&mut editor, vec![egui::Event::PointerMoved(side)]);
    for _ in 0..4 {
        frame(&mut editor, vec![line(-1.)]);
    }
    assert_eq!(panel.get(), 0.);
    assert_eq!(editor.view.zoom.pan, pan);
    // Once it pauses, the panel takes the next one, and keeps it over the photo.
    for _ in 0..30 {
        frame(&mut editor, vec![]);
    }
    frame(&mut editor, vec![line(-1.)]);
    for _ in 0..4 {
        frame(&mut editor, vec![]);
    }
    assert!(panel.get() > 0.);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(photo), line(-1.)],
    );
    assert_eq!(editor.view.zoom.pan, pan);
    // A trackpad touch whose End never comes ends once the wheel is quiet for
    // long: the panel scrolls again.
    for _ in 0..30 {
        frame(&mut editor, vec![]);
    }
    let start = |pos| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: POINT,
                delta: Vec2::new(0., -20.),
                phase: egui::TouchPhase::Start,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    frame(&mut editor, start(photo));
    for _ in 0..50 {
        frame(&mut editor, vec![]);
    }
    let before = panel.get();
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(side), line(-1.)],
    );
    for _ in 0..4 {
        frame(&mut editor, vec![]);
    }
    assert!(panel.get() > before);
}
