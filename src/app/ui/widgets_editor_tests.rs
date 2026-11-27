//! Editor tests of `widgets`: the whole editor driven through its frames.
use crate::app::ui::widgets::tone_curve_ui;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

#[test]
fn curve_pointer_add_drag_and_remove() {
    let ctx = egui::Context::default();
    let mut curve = crate::color::curve::ToneCurve::default();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                events,
                ..Default::default()
            },
            |ui| {
                tone_curve_ui(ui, &mut curve, &[[0; 256]; 3], 0, Default::default());
            },
        );
        output.textures_delta.clear();
        curve.points.clone()
    };
    let event = |p: Pos2, button, pressed| egui::Event::PointerButton {
        pos: p,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    let p = Pos2::new(150., 160.);
    frame(vec![
        egui::Event::PointerMoved(p),
        event(p, egui::PointerButton::Primary, true),
    ]);
    assert_eq!(
        frame(vec![event(p, egui::PointerButton::Primary, false)]).len(),
        3
    );
    frame(vec![event(p, egui::PointerButton::Primary, true)]);
    let q = Pos2::new(200., 110.);
    let moved = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(moved[1][0] > 0.6 && moved[1][1] > 0.6);
    frame(vec![event(q, egui::PointerButton::Primary, false)]);
    frame(vec![event(q, egui::PointerButton::Secondary, true)]);
    assert_eq!(
        frame(vec![event(q, egui::PointerButton::Secondary, false)]).len(),
        2
    );
}

/// A point curve on its own in a 320-point window, driven a frame at a time
/// with the modifiers held through the frame.
struct CurveHarness {
    ctx: egui::Context,
    curve: crate::color::curve::ToneCurve,
    /// The input's clock, a twentieth of a second a frame, so two clicks a
    /// few frames apart make a double-click.
    time: f64,
}
impl CurveHarness {
    fn new() -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            curve: Default::default(),
            time: 0.,
        };
        harness.frame(vec![], egui::Modifiers::NONE);
        harness
    }
    fn frame(&mut self, mut events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let curve = &mut self.curve;
        self.time += 0.05;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                tone_curve_ui(ui, curve, &[[0; 256]; 3], 0, Default::default());
            },
        );
        output.textures_delta.clear();
    }
    /// The curve's area inside its well, where 0–1 map to its edges.
    fn area(&mut self) -> Rect {
        self.ctx.enable_accesskit();
        let mut events = vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)];
        let curve = &mut self.curve;
        self.time += 0.05;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                time: Some(self.time),
                events: std::mem::take(&mut events),
                ..Default::default()
            },
            |ui| {
                tone_curve_ui(ui, curve, &[[0; 256]; 3], 0, Default::default());
            },
        );
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.expect("on");
        let b = tree
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Point curve"))
            .and_then(|(_, n)| n.bounds())
            .expect("the well");
        Rect::from_min_max(
            Pos2::new(b.x0 as f32, b.y0 as f32),
            Pos2::new(b.x1 as f32, b.y1 as f32),
        )
        .shrink(5.)
    }
    /// Clicks at `at` with `modifiers` held.
    fn click(&mut self, at: Pos2, modifiers: egui::Modifiers) {
        self.button(at, true, modifiers);
        self.button(at, false, modifiers);
    }
    fn button(&mut self, at: Pos2, pressed: bool, modifiers: egui::Modifiers) {
        self.frame(
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                },
            ],
            modifiers,
        );
    }
    /// Adds a point at `at` with a click and returns its index.
    fn add(&mut self, at: Pos2) -> usize {
        self.button(at, true, egui::Modifiers::NONE);
        self.button(at, false, egui::Modifiers::NONE);
        assert_eq!(self.curve.points.len(), 3);
        1
    }
    /// Drags from `from` through each of `path` in turn, each with its
    /// modifiers, and releases at the last.
    fn drag(&mut self, from: Pos2, path: &[(Pos2, egui::Modifiers)]) {
        let first = path.first().map_or(egui::Modifiers::NONE, |(_, m)| *m);
        self.button(from, true, first);
        for (at, modifiers) in path {
            self.frame(vec![egui::Event::PointerMoved(*at)], *modifiers);
        }
        let (last, modifiers) = path.last().copied().unwrap_or((from, first));
        self.button(last, false, modifiers);
    }
}

const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;
const ALT: egui::Modifiers = egui::Modifiers::ALT;

/// How far up a plain drag of `dy` points moves a point added at `at`.
fn plain_rise(at: Pos2, dy: f32) -> f32 {
    let mut h = CurveHarness::new();
    let i = h.add(at);
    let before = h.curve.points[i];
    h.drag(at, &[(at + Vec2::new(0., -dy), egui::Modifiers::NONE)]);
    h.curve.points[i][1] - before[1]
}

#[test]
fn a_curve_point_dragged_with_shift_moves_only_up_and_down() {
    let mut h = CurveHarness::new();
    let at = Pos2::new(150., 160.);
    let i = h.add(at);
    let before = h.curve.points[i];
    h.drag(
        at,
        &[
            (at + Vec2::new(10., -10.), SHIFT),
            (at + Vec2::new(40., -40.), SHIFT),
        ],
    );
    let after = h.curve.points[i];
    assert_eq!(after[0], before[0], "the input stays");
    assert!(after[1] > before[1] + 0.1, "the output rises: {after:?}");
}

#[test]
fn a_curve_point_dragged_with_alt_moves_a_tenth_as_far() {
    let at = Pos2::new(150., 160.);
    let full = plain_rise(at, 60.);
    let mut h = CurveHarness::new();
    let i = h.add(at);
    let before = h.curve.points[i];
    h.drag(at, &[(at + Vec2::new(30., -60.), ALT)]);
    let after = h.curve.points[i];
    let step = 1. / 255.;
    assert!(
        (after[1] - before[1] - full / 10.).abs() <= 1.5 * step,
        "rose {} for a tenth of {full}",
        after[1] - before[1]
    );
    assert!((after[0] - before[0] - full / 20.).abs() <= 1.5 * step);
}

#[test]
fn a_curve_point_dragged_with_shift_and_alt_moves_slowly_up_and_down() {
    let at = Pos2::new(150., 160.);
    let full = plain_rise(at, 60.);
    let mut h = CurveHarness::new();
    let i = h.add(at);
    let before = h.curve.points[i];
    h.drag(at, &[(at + Vec2::new(40., -60.), SHIFT | ALT)]);
    let after = h.curve.points[i];
    assert_eq!(after[0], before[0]);
    assert!((after[1] - before[1] - full / 10.).abs() <= 1.5 / 255.);
}

/// Pressing or letting go of a modifier mid-drag changes how the point follows
/// from there on, without a jump to or from the pointer.
#[test]
fn a_modifier_pressed_mid_drag_changes_the_pace_without_a_jump() {
    let at = Pos2::new(150., 160.);
    let full = plain_rise(at, 60.);
    let mut h = CurveHarness::new();
    let i = h.add(at);
    let before = h.curve.points[i];
    let none = egui::Modifiers::NONE;
    h.drag(
        at,
        &[
            (at + Vec2::new(0., -60.), none),
            (at + Vec2::new(0., -60.), ALT),
            (at + Vec2::new(0., -120.), ALT),
        ],
    );
    let rise = h.curve.points[i][1] - before[1];
    assert!(
        (rise - full * 1.1).abs() <= 1.5 / 255.,
        "rose {rise} for {full} and a tenth"
    );
}

/// Grabbing a point off its centre moves it by the pointer's movement, not to
/// the pointer.
#[test]
fn a_curve_point_grabbed_off_centre_does_not_jump_to_the_pointer() {
    let mut h = CurveHarness::new();
    let at = Pos2::new(150., 160.);
    let i = h.add(at);
    let before = h.curve.points[i];
    let grab = at + Vec2::new(6., 0.);
    h.drag(grab, &[(grab + Vec2::new(0., -30.), SHIFT)]);
    assert_eq!(h.curve.points[i][0], before[0]);
}

/// Double-clicking a point removes it, as in Lightroom; the end points stay.
#[test]
fn double_clicking_a_curve_point_removes_it() {
    let mut h = CurveHarness::new();
    let at = Pos2::new(150., 160.);
    h.add(at);
    // Long enough after adding it that the next two clicks are a double-click
    // of their own.
    for _ in 0..20 {
        h.frame(vec![], egui::Modifiers::NONE);
    }
    for _ in 0..2 {
        h.button(at, true, egui::Modifiers::NONE);
        h.button(at, false, egui::Modifiers::NONE);
    }
    assert_eq!(h.curve.points.len(), 2, "{:?}", h.curve.points);
    let end = h.curve.points[1];
    let rect = h.ctx.content_rect().shrink(5.);
    let corner = Pos2::new(rect.right() - 5., rect.top() + 5.);
    for _ in 0..20 {
        h.frame(vec![], egui::Modifiers::NONE);
    }
    for _ in 0..2 {
        h.button(corner, true, egui::Modifiers::NONE);
        h.button(corner, false, egui::Modifiers::NONE);
    }
    assert_eq!(h.curve.points.len(), 2);
    assert_eq!(h.curve.points[1], end);
}

/// Where input `x` and output `y` of a curve are on screen.
fn on_screen(area: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(
        area.left() + x * area.width(),
        area.bottom() - y * area.height(),
    )
}

/// A plain click puts the point where it is clicked, even a few points off the
/// line: only Alt-click puts it on the line.
#[test]
fn a_plain_click_puts_the_point_where_it_is_clicked() {
    let mut h = CurveHarness::new();
    let area = h.area();
    let at = on_screen(area, 0.5, 0.5) + Vec2::new(0., -6.);
    h.click(at, egui::Modifiers::NONE);
    let [_, y] = h.curve.points[1];
    let clicked = (area.bottom() - at.y) / area.height();
    assert!((y - clicked).abs() <= 1. / 255., "{y} for {clicked}");
}

/// Alt-click adds a point on the line however far from it, as in Lightroom.
#[test]
fn an_alt_click_adds_a_point_on_the_curve_from_anywhere() {
    let mut h = CurveHarness::new();
    h.curve.points = vec![[0., 0.], [0.5, 0.7], [1., 1.]];
    let lut = crate::color::curve::CurveLut::new(&h.curve);
    let area = h.area();
    h.click(on_screen(area, 0.25, 0.9), ALT);
    assert_eq!(h.curve.points.len(), 4);
    let [x, y] = h.curve.points[1];
    assert!((y - lut.evaluate(x)).abs() <= 0.5 / 255., "{x} → {y}");
}

/// Secondary-button drags (and Control-drags on macOS) must never insert or
/// move curve points.
#[test]
fn a_secondary_or_control_drag_does_not_add_or_move_a_curve_point() {
    let mut h = CurveHarness::new();
    let at = Pos2::new(150., 160.);
    let i = h.add(at);
    let before = h.curve.points.clone();
    // Drag with the secondary button starting on empty space.
    let empty = Pos2::new(80., 80.);
    h.frame(
        vec![
            egui::Event::PointerMoved(empty),
            egui::Event::PointerButton {
                pos: empty,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        egui::Modifiers::NONE,
    );
    h.frame(
        vec![egui::Event::PointerMoved(empty + Vec2::new(30., -30.))],
        egui::Modifiers::NONE,
    );
    h.frame(
        vec![egui::Event::PointerButton {
            pos: empty + Vec2::new(30., -30.),
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
        egui::Modifiers::NONE,
    );
    assert_eq!(
        h.curve.points, before,
        "secondary drag on empty space must not edit the curve"
    );
    // Drag with the secondary button starting on the existing point.
    h.frame(
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        egui::Modifiers::NONE,
    );
    h.frame(
        vec![egui::Event::PointerMoved(at + Vec2::new(0., -30.))],
        egui::Modifiers::NONE,
    );
    h.frame(
        vec![egui::Event::PointerButton {
            pos: at + Vec2::new(0., -30.),
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
        egui::Modifiers::NONE,
    );
    assert_eq!(
        h.curve.points[i], before[i],
        "secondary drag on a point must not move it"
    );
    if cfg!(target_os = "macos") {
        let ctrl = egui::Modifiers::CTRL;
        h.drag(empty, &[(empty + Vec2::new(30., -30.), ctrl)]);
        assert_eq!(
            h.curve.points, before,
            "Control-drag on macOS must not add or move a point"
        );
    }
}

/// Deleting the point being dragged ends the drag rather than moving its
/// neighbour (or the white endpoint) to where the pointer is.
#[test]
fn deleting_a_curve_point_mid_drag_does_not_move_its_neighbour() {
    let mut h = CurveHarness::new();
    let at = Pos2::new(150., 160.);
    h.add(at);
    let end = h.curve.points[2];
    // Start dragging the interior point.
    h.button(at, true, egui::Modifiers::NONE);
    h.frame(
        vec![egui::Event::PointerMoved(at + Vec2::new(0., -20.))],
        egui::Modifiers::NONE,
    );
    // Press Delete while still holding the primary button, then move further.
    h.frame(
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: Some(egui::Key::Delete),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        egui::Modifiers::NONE,
    );
    h.frame(
        vec![egui::Event::PointerMoved(at + Vec2::new(0., -40.))],
        egui::Modifiers::NONE,
    );
    h.button(at + Vec2::new(0., -40.), false, egui::Modifiers::NONE);
    assert_eq!(h.curve.points.len(), 2);
    assert_eq!(
        h.curve.points[1], end,
        "white endpoint must not be hijacked after deleting the dragged point"
    );
}

/// Pressing Alt after dragging a point past the well's top lets a small
/// downward movement bring it back off 255 right away, without crossing 10×
/// the overshoot.
#[test]
fn pressing_alt_after_dragging_past_the_top_responds_immediately() {
    let mut h = CurveHarness::new();
    let area = h.area();
    let at = on_screen(area, 0.5, 0.8);
    let i = h.add(at);
    let far_above = Pos2::new(at.x, area.top() - 100.);
    h.drag(
        at,
        &[
            (far_above, egui::Modifiers::NONE),
            (far_above, ALT),
            (far_above + Vec2::new(0., 30.), ALT),
        ],
    );
    assert!(
        h.curve.points[i][1] < 1.,
        "expected Alt-drag back down to leave 1.0, stayed at {}",
        h.curve.points[i][1]
    );
}

/// The Input and Output captions share one vertical middle with their fields,
/// both before and after a point is selected.
#[test]
fn point_curve_input_and_output_labels_align_with_their_fields() {
    let mut h = CurveHarness::new();
    let area = h.area();
    for select in [false, true] {
        if select {
            h.click(on_screen(area, 0.5, 0.5), egui::Modifiers::NONE);
        }
        let curve = &mut h.curve;
        h.time += 0.05;
        let mut output = h.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                time: Some(h.time),
                ..Default::default()
            },
            |ui| {
                tone_curve_ui(ui, curve, &[[0; 256]; 3], 0, Default::default());
            },
        );
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.expect("on");
        let mid_y = |role: egui::accesskit::Role, label: &str| {
            let b = tree
                .nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| {
                    n.role() == role && (n.label() == Some(label) || n.value() == Some(label))
                })
                .and_then(|n| n.bounds())
                .unwrap_or_else(|| panic!("no {role:?} {label:?} (selected={select})"));
            (b.y0 + b.y1) / 2.
        };
        let input_label = mid_y(egui::accesskit::Role::Label, "Input");
        let output_label = mid_y(egui::accesskit::Role::Label, "Output");
        assert!(
            (input_label - output_label).abs() <= 0.5,
            "Input ({input_label}) and Output ({output_label}) labels misaligned (selected={select})"
        );
        if select {
            let field_mid_y = |label: &str| {
                let b = tree
                    .nodes
                    .iter()
                    .map(|(_, n)| n)
                    .find(|n| n.role() != egui::accesskit::Role::Label && n.label() == Some(label))
                    .and_then(|n| n.bounds())
                    .unwrap_or_else(|| panic!("no field {label:?}"));
                (b.y0 + b.y1) / 2.
            };
            let input_field = field_mid_y("Input");
            let output_field = field_mid_y("Output");
            assert!(
                (input_label - input_field).abs() <= 1.0,
                "Input label ({input_label}) off field middle ({input_field})"
            );
            assert!(
                (output_label - output_field).abs() <= 1.0,
                "Output label ({output_label}) off field middle ({output_field})"
            );
        } else {
            let dashes: Vec<f64> = tree
                .nodes
                .iter()
                .map(|(_, n)| n)
                .filter(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("–"))
                .filter_map(|n| n.bounds())
                .map(|b| (b.y0 + b.y1) / 2.)
                .collect();
            assert_eq!(dashes.len(), 2, "two blank fields");
            for field in dashes {
                assert!(
                    (input_label - field).abs() <= 1.0,
                    "Input label ({input_label}) off blank field middle ({field})"
                );
            }
        }
    }
}

/// Runs one frame of `add` in a 360-point-wide window at `time`.
fn widget_frame(
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    add: impl FnMut(&mut egui::Ui),
) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360., 600.))),
            time: Some(time),
            events,
            ..Default::default()
        },
        add,
    );
    output.textures_delta.clear();
}

fn click_at(at: Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), press(true), press(false)]
}

#[test]
fn a_panel_header_switch_turns_the_panel_off_and_on_without_opening_it() {
    use crate::model::panels::PanelState;
    let ctx = egui::Context::default();
    let mut state = PanelState::On;
    let area = std::cell::Cell::new(Rect::NOTHING);
    let draw = |state: &mut PanelState, events, time| {
        widget_frame(&ctx, time, events, |ui| {
            area.set(ui.max_rect());
            crate::app::ui::widgets::switched_section(
                ui,
                &mut Default::default(),
                "Tone Curve",
                state,
                |ui, _| {
                    ui.label("contents");
                },
            );
        })
    };
    draw(&mut state, vec![], 0.);
    // The switch sits left of the reset button, which sits left of the
    // chevron, in the 28-point header 14 points down.
    let area = area.get();
    let switch = Pos2::new(area.right() - 26. - 26. - 18., area.top() + 14. + 14.);
    draw(
        &mut state,
        click_at(switch, egui::PointerButton::Primary),
        1.,
    );
    assert_eq!(state, PanelState::Off);
    draw(
        &mut state,
        click_at(switch, egui::PointerButton::Primary),
        2.,
    );
    assert_eq!(state, PanelState::On);
    let collapsed: Option<std::collections::BTreeSet<String>> =
        ctx.data(|d| d.get_temp(crate::app::ui::widgets::collapsed_sections_id()));
    assert!(collapsed.is_none_or(|set| set.is_empty()));
}

#[test]
fn solo_mode_opens_one_panel_per_side_and_is_set_from_the_header_menu() {
    use crate::app::ui::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    let titles = ["Basic", "Tone Curve", "Detail"];
    let headers = std::cell::RefCell::new(Vec::new());
    let draw = |events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            headers.borrow_mut().clear();
            for title in titles {
                headers.borrow_mut().push(ui.cursor().top() + 8. + 14.);
                crate::app::ui::widgets::section(ui, title, |ui| {
                    ui.label(title);
                });
            }
        })
    };
    let collapsed = || -> std::collections::BTreeSet<String> {
        ctx.data(|d| d.get_temp(crate::app::ui::widgets::collapsed_sections_id()))
            .unwrap_or_default()
    };
    draw(vec![], 0.);
    // Right-click Tone Curve's header, then Solo Mode in its menu.
    let header = Pos2::new(60., headers.borrow()[1]);
    draw(click_at(header, egui::PointerButton::Secondary), 1.);
    draw(vec![], 1.1);
    let item = Pos2::new(header.x + 30., header.y + 14.);
    draw(click_at(item, egui::PointerButton::Primary), 2.);
    let solo: Option<std::collections::BTreeSet<String>> =
        ctx.data(|d| d.get_temp(crate::app::ui::widgets::solo_sections_id()));
    assert_eq!(solo, Some(["develop-right".to_string()].into()));
    assert_eq!(
        collapsed(),
        ["Basic".to_string(), "Detail".to_string()].into()
    );
    // Opening Detail closes Tone Curve.
    draw(vec![], 3.);
    let detail = Pos2::new(60., headers.borrow()[2]);
    draw(click_at(detail, egui::PointerButton::Primary), 4.);
    assert_eq!(
        collapsed(),
        ["Basic".to_string(), "Tone Curve".to_string()].into()
    );
}

#[test]
fn the_panel_header_menu_fits_its_item_not_the_header() {
    use crate::app::ui::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    let header = std::cell::Cell::new(0.);
    let draw = |events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            header.set(ui.cursor().top() + 14. + 14.);
            crate::app::ui::widgets::section(ui, "Basic", |ui| {
                ui.label("Basic");
            });
        })
    };
    draw(vec![], 0.);
    draw(
        click_at(Pos2::new(60., header.get()), egui::PointerButton::Secondary),
        1.,
    );
    draw(vec![], 1.1);
    let menus: Vec<Rect> = ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|layer| layer.order == egui::Order::Foreground)
            .filter_map(|layer| m.area_rect(layer.id))
            .collect()
    });
    assert_eq!(menus.len(), 1, "{menus:?}");
    // The header spans the 360 pt panel; Solo Mode needs far less.
    assert!(menus[0].width() < 200., "{menus:?}");
}

#[test]
fn up_and_down_nudge_the_hovered_slider_but_never_while_typing_or_scrolling() {
    let ctx = egui::Context::default();
    let mut value = 0.;
    let mut text = String::new();
    let row = std::cell::Cell::new(Rect::NOTHING);
    let mut draw = |value: &mut f32, focus: bool, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            crate::app::ui::widgets::slider(
                ui,
                &mut Default::default(),
                "Contrast",
                value,
                -1. ..=1.,
                0.,
            );
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
            let field = ui.text_edit_singleline(&mut text);
            if focus {
                field.request_focus();
            }
        })
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    };
    draw(&mut value, false, vec![], 0.);
    let row = row.get();
    let over = egui::Event::PointerMoved(Pos2::new(row.center().x, row.top() + 12.));
    draw(&mut value, false, vec![over], 1.);
    draw(
        &mut value,
        false,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        2.,
    );
    assert!((value - 0.01).abs() < 1e-6, "{value}");
    draw(
        &mut value,
        false,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::SHIFT)],
        3.,
    );
    assert!((value + 0.09).abs() < 1e-6, "{value}");
    // Scrolling over it never moves it.
    let scroll = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0., -3.),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    };
    draw(&mut value, false, vec![scroll], 4.);
    assert!((value + 0.09).abs() < 1e-6, "{value}");
    // While a text field has the keyboard, the keys are the field's.
    draw(&mut value, true, vec![], 5.);
    draw(
        &mut value,
        true,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        6.,
    );
    assert!((value + 0.09).abs() < 1e-6, "{value}");
}

#[test]
fn a_disabled_slider_ignores_up_and_down() {
    let ctx = egui::Context::default();
    let mut value = 0.;
    let row = std::cell::Cell::new(Rect::NOTHING);
    let draw = |value: &mut f32, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            ui.add_enabled_ui(false, |ui| {
                crate::app::ui::widgets::slider(
                    ui,
                    &mut Default::default(),
                    "Contrast",
                    value,
                    -1. ..=1.,
                    0.,
                );
            });
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
        })
    };
    draw(&mut value, vec![], 0.);
    let over = egui::Event::PointerMoved(row.get().center());
    draw(&mut value, vec![over], 1.);
    let up = egui::Event::Key {
        key: egui::Key::ArrowUp,
        physical_key: Some(egui::Key::ArrowUp),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    draw(&mut value, vec![up], 2.);
    assert_eq!(value, 0.);
}

#[test]
fn b_and_w_opens_and_closes_with_the_color_mixer_in_solo_mode() {
    use crate::app::ui::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    ctx.data_mut(|d| {
        d.insert_temp(
            crate::app::ui::widgets::solo_sections_id(),
            std::collections::BTreeSet::from(["develop-right".to_string()]),
        )
    });
    let headers = std::cell::RefCell::new(Vec::new());
    let draw = |mixer: &str, events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            headers.borrow_mut().clear();
            for title in ["Tone Curve", mixer] {
                headers.borrow_mut().push(ui.cursor().top() + 8. + 14.);
                crate::app::ui::widgets::section(ui, title, |ui| {
                    ui.label(title);
                });
            }
        })
    };
    let collapsed = || -> std::collections::BTreeSet<String> {
        ctx.data(|d| d.get_temp(crate::app::ui::widgets::collapsed_sections_id()))
            .unwrap_or_default()
    };
    draw("Color Mixer", vec![], 0.);
    // Open Tone Curve alone, then convert to black & white: B&W stays closed.
    let mixer = Pos2::new(60., headers.borrow()[1]);
    draw(
        "Color Mixer",
        click_at(mixer, egui::PointerButton::Primary),
        1.,
    );
    assert_eq!(collapsed(), ["Color Mixer".to_string()].into());
    draw("B&W", vec![], 3.);
    assert_eq!(collapsed(), ["Color Mixer".to_string()].into());
    // Opening B&W closes Tone Curve, and the Color Mixer comes back open.
    draw("B&W", click_at(mixer, egui::PointerButton::Primary), 4.);
    assert_eq!(collapsed(), ["Tone Curve".to_string()].into());
}

#[test]
fn an_imported_value_outside_the_slider_survives_being_shown_and_nudged() {
    let ctx = egui::Context::default();
    let row = std::cell::Cell::new(Rect::NOTHING);
    let draw = |value: &mut f32, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            crate::app::ui::widgets::setting_slider(
                ui,
                &mut Default::default(),
                crate::model::params::ParameterId::Exposure,
                value,
                0.,
            );
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
        })
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // An imported +6 EV, beyond the slider's ±5.
    let mut value = 6.;
    draw(&mut value, vec![], 0.);
    assert_eq!(value, 6., "showing the slider changed the value");
    let over = egui::Event::PointerMoved(row.get().center());
    draw(&mut value, vec![over.clone()], 1.);
    assert_eq!(value, 6.);
    // Up moves it no further out; Down moves it one step toward the range,
    // not to its end.
    draw(&mut value, vec![over.clone(), key(egui::Key::ArrowUp)], 2.);
    assert_eq!(value, 6.);
    draw(&mut value, vec![over, key(egui::Key::ArrowDown)], 3.);
    assert!(5. < value && value < 6., "{value}");
}

/// At either end of its range, a slider's knob stays within the rail's ends, even
/// while it is hovered and drawn larger.
#[test]
fn a_hovered_knob_at_either_end_stays_on_the_rail() {
    for end in [-1f32, 1.] {
        let ctx = egui::Context::default();
        let mut value = end;
        let row = std::cell::Cell::new(Rect::NOTHING);
        let mut frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360., 600.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().min;
                    crate::app::ui::widgets::slider(
                        ui,
                        &mut Default::default(),
                        "Contrast",
                        &mut value,
                        -1. ..=1.,
                        0.,
                    );
                    row.set(Rect::from_min_max(
                        top,
                        Pos2::new(ui.max_rect().right(), ui.cursor().top()),
                    ));
                },
            );
            output.textures_delta.clear();
            output.shapes
        };
        frame(vec![]);
        let row = row.get();
        // Over the rail at the knob's end of it.
        let x = if end < 0. {
            row.left() + 4.
        } else {
            row.right() - 4.
        };
        let over = egui::Event::PointerMoved(Pos2::new(x, row.bottom() - 8.));
        frame(vec![over.clone()]);
        let shapes = frame(vec![over]);
        let circles: Vec<_> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Circle(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(!circles.is_empty());
        for c in circles {
            let reach = c.radius + c.stroke.width / 2.;
            assert!(
                c.center.x - reach >= row.left() - 0.01 && c.center.x + reach <= row.right() + 0.01,
                "a knob of radius {reach} at {} passes the rail {:?}",
                c.center.x,
                row.x_range()
            );
        }
    }
}
