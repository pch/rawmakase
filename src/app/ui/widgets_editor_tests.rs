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
            |ui| tone_curve_ui(ui, &mut curve, &[[0; 256]; 3], 0),
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
fn up_and_down_nudge_the_hovered_slider_and_the_wheel_only_with_its_setting() {
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
    // Scrolling over it doesn't move it: the wheel moves sliders only with its
    // setting on, which is off by default (see `wheel_steps`).
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

/// Two sliders in a panel that scrolls, drawn frame by frame with the wheel
/// preferences published as the editor publishes them.
struct WheelPanel {
    ctx: egui::Context,
    time: f64,
    prefs: crate::app::ui::wheel::Prefs,
    values: [f32; 2],
    rows: [Rect; 2],
    offset: f32,
    step: crate::app::develop::step_name::StepName,
    enabled: bool,
    focus: bool,
}
impl WheelPanel {
    fn new() -> Self {
        let mut panel = Self {
            ctx: egui::Context::default(),
            time: 0.,
            prefs: crate::app::ui::wheel::Prefs {
                zoom: crate::app::ui::wheel::WheelZoom::Off,
                sliders: true,
                rule: crate::app::ui::wheel::NotchRule::LineUnits,
            },
            values: [0.; 2],
            rows: [Rect::NOTHING; 2],
            offset: 0.,
            step: Default::default(),
            enabled: true,
            focus: false,
        };
        panel.frame(vec![]);
        panel
    }
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let Self {
            ctx,
            prefs,
            values,
            rows,
            offset,
            step,
            enabled,
            focus,
            ..
        } = self;
        let mut text = String::new();
        widget_frame(ctx, self.time, events, |ui| {
            crate::app::ui::wheel::begin_frame(ui.ctx(), *prefs);
            let out = egui::ScrollArea::vertical()
                .id_salt("panel")
                .show(ui, |ui| {
                    for (i, label) in ["Contrast", "Highlights"].into_iter().enumerate() {
                        let top = ui.cursor().min;
                        ui.add_enabled_ui(*enabled, |ui| {
                            crate::app::ui::widgets::slider(
                                ui,
                                step,
                                label,
                                &mut values[i],
                                -1. ..=1.,
                                0.,
                            );
                        });
                        rows[i] = Rect::from_min_max(
                            top,
                            Pos2::new(ui.max_rect().right(), ui.cursor().top()),
                        );
                    }
                    let field = ui.text_edit_singleline(&mut text);
                    if *focus {
                        field.request_focus();
                    }
                    ui.allocate_space(Vec2::new(100., 2000.));
                });
            *offset = out.state.offset.y;
        });
    }
    /// Half a second of frames with no wheel: the gesture ends.
    fn pause(&mut self) {
        for _ in 0..10 {
            self.frame(vec![]);
        }
    }
    fn over(&self, slider: usize) -> egui::Event {
        let row = self.rows[slider];
        egui::Event::PointerMoved(Pos2::new(row.center().x, row.top() + 30.))
    }
}
fn notch(unit: egui::MouseWheelUnit, y: f32, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::MouseWheel {
        unit,
        delta: Vec2::new(0., y),
        phase: egui::TouchPhase::Move,
        modifiers,
    }
}
fn line(y: f32) -> egui::Event {
    notch(egui::MouseWheelUnit::Line, y, egui::Modifiers::NONE)
}

#[test]
fn a_notched_wheel_steps_the_slider_under_the_pointer_with_its_setting() {
    let mut p = WheelPanel::new();
    p.frame(vec![p.over(0)]);
    p.frame(vec![line(1.)]);
    assert!((p.values[0] - 0.01).abs() < 1e-6, "{:?}", p.values);
    // A macOS wheel's accelerated fractions step once they add up to a notch.
    p.frame(vec![line(0.4)]);
    p.frame(vec![line(0.4)]);
    assert!((p.values[0] - 0.01).abs() < 1e-6, "{:?}", p.values);
    p.frame(vec![line(0.4)]);
    assert!((p.values[0] - 0.02).abs() < 1e-6, "{:?}", p.values);
    // Shift: ten steps a notch, whichever way macOS turns the wheel.
    p.frame(vec![notch(
        egui::MouseWheelUnit::Line,
        -1.,
        egui::Modifiers::SHIFT,
    )]);
    assert!((p.values[0] + 0.08).abs() < 1e-6, "{:?}", p.values);
    // The panel under it never moved, nor does it while egui's smoothing runs out.
    p.pause();
    assert_eq!(p.offset, 0.);
    assert_eq!(p.values[1], 0.);
    let value = p.values[0];
    // On Windows and X11, a touchpad's small fractions of a line never step it
    // however they add up; a mouse's whole notches do.
    p.prefs.rule = crate::app::ui::wheel::NotchRule::WholeLines;
    for _ in 0..8 {
        p.frame(vec![line(0.25)]);
    }
    p.pause();
    assert_eq!(p.values[0], value);
    p.frame(vec![line(1.)]);
    assert!((p.values[0] - value - 0.01).abs() < 1e-6, "{:?}", p.values);
    // A trackpad scrolls the panel instead, on any platform.
    p.pause();
    let value = p.values[0];
    p.frame(vec![notch(
        egui::MouseWheelUnit::Point,
        -60.,
        egui::Modifiers::NONE,
    )]);
    p.pause();
    assert_eq!(p.values[0], value);
    assert!(p.offset > 0.);
}

#[test]
fn a_slider_keeps_the_gesture_it_began_and_a_panel_scroll_passes_over_sliders() {
    let mut p = WheelPanel::new();
    p.frame(vec![p.over(0)]);
    p.frame(vec![line(-1.)]);
    assert!((p.values[0] + 0.01).abs() < 1e-6);
    // The pointer leaves its row mid-gesture: the slider stops, the next slider
    // takes nothing, and the panel stays put.
    p.frame(vec![p.over(1), line(-1.)]);
    for _ in 0..4 {
        p.frame(vec![line(-1.)]);
    }
    p.pause();
    assert!((p.values[0] + 0.01).abs() < 1e-6, "{:?}", p.values);
    assert_eq!(p.values[1], 0.);
    assert_eq!(p.offset, 0.);
    // A scroll begun over the panel scrolls it, and passing over a slider never
    // steps it.
    let below = p.rows[1].bottom() + 100.;
    p.frame(vec![egui::Event::PointerMoved(Pos2::new(100., below))]);
    p.frame(vec![line(-1.)]);
    p.frame(vec![p.over(1), line(-1.)]);
    p.frame(vec![p.over(1), line(-1.)]);
    assert!(p.offset > 0.);
    assert_eq!(p.values[1], 0.);
}

#[test]
fn a_disabled_slider_or_a_field_with_the_keyboard_leaves_the_wheel_to_the_panel() {
    let mut p = WheelPanel::new();
    p.enabled = false;
    p.frame(vec![p.over(0)]);
    p.frame(vec![line(-1.)]);
    p.pause();
    assert_eq!(p.values, [0., 0.]);
    assert!(p.offset > 0.);
    let mut p = WheelPanel::new();
    p.focus = true;
    p.frame(vec![]);
    p.frame(vec![p.over(0)]);
    p.frame(vec![line(1.)]);
    p.pause();
    assert_eq!(p.values, [0., 0.]);
}

/// The editor's own frames around a slider: what the wheel does to History.
struct WheelHistory {
    editor: Editor,
    ctx: egui::Context,
    time: f64,
    /// Where each slider's row is: Exposure, Contrast, then a view-only one.
    rows: [Rect; 3],
    view_only: f32,
}
impl WheelHistory {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let mut h = Self {
            editor,
            ctx,
            time: 0.,
            rows: [Rect::NOTHING; 3],
            view_only: 0.5,
        };
        h.frame(vec![]);
        h
    }
    /// One edit frame; `between` runs after the sliders, before the frame finishes.
    fn frame_with(&mut self, events: Vec<egui::Event>, between: impl FnOnce(&mut Editor)) {
        self.time += 0.05;
        let prefs = crate::app::ui::wheel::Prefs {
            zoom: crate::app::ui::wheel::WheelZoom::Off,
            sliders: true,
            rule: crate::app::ui::wheel::NotchRule::LineUnits,
        };
        let frame = self.editor.begin_edit_frame();
        let Self {
            editor,
            ctx,
            rows,
            view_only,
            ..
        } = self;
        widget_frame(ctx, self.time, events, |ui| {
            crate::app::ui::wheel::begin_frame(ui.ctx(), prefs);
            let step = &mut editor.step_name;
            let recipe = editor.document.edit.recipe_mut();
            let mut row = |i: usize, ui: &mut egui::Ui, label, value: &mut f32| {
                let top = ui.cursor().min;
                crate::app::ui::widgets::slider(ui, step, label, value, -1. ..=1., 0.);
                rows[i] =
                    Rect::from_min_max(top, Pos2::new(ui.max_rect().right(), ui.cursor().top()));
            };
            row(0, ui, "Exposure", &mut recipe.exposure);
            row(1, ui, "Contrast", &mut recipe.contrast);
            // A brush's Size: a tool setting, no edit.
            row(2, ui, "Size", view_only);
        });
        between(&mut self.editor);
        self.editor.finish_edit_frame(frame, &self.ctx);
    }
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_with(events, |_| {});
    }
    fn over(&self, slider: usize) -> egui::Event {
        egui::Event::PointerMoved(self.rows[slider].center() + Vec2::new(0., 8.))
    }
    /// Lets the wheel's input gesture end (input time) and, with `wall`, the
    /// History step's pause (the clock) too.
    fn pause(&mut self, wall: bool) {
        for _ in 0..10 {
            self.frame(vec![]);
        }
        if wall {
            std::thread::sleep(std::time::Duration::from_millis(450));
            self.frame(vec![]);
        }
    }
    fn steps(&self) -> Vec<(String, String)> {
        let (steps, applied) = self.editor.document.edit.history().steps();
        steps[..applied]
            .iter()
            .map(|s| (s.name.clone(), s.value.clone()))
            .collect()
    }
}

#[test]
fn wheel_notches_on_a_slider_are_one_history_step_and_another_slider_starts_its_own() {
    let mut h = WheelHistory::new();
    h.frame(vec![h.over(0)]);
    for _ in 0..5 {
        h.frame(vec![line(1.)]);
    }
    assert!(h.editor.document.edit.history().in_gesture());
    assert!(h.editor.view.wheel.pending());
    // The input gesture ends, but the History step waits for its own pause: the
    // next slider's notches within it are a step of their own.
    h.pause(false);
    h.frame(vec![h.over(1)]);
    h.frame(vec![line(-1.)]);
    h.frame(vec![line(-1.)]);
    h.pause(true);
    assert_eq!(
        h.steps(),
        [
            ("Exposure".to_string(), "+5".to_string()),
            ("Contrast".to_string(), "-2".to_string())
        ]
    );
    assert!(!h.editor.view.wheel.pending());
}

#[test]
fn the_wheel_over_a_view_only_slider_records_no_step() {
    let mut h = WheelHistory::new();
    h.frame(vec![h.over(2)]);
    h.frame(vec![line(1.)]);
    assert!((h.view_only - 0.51).abs() < 1e-6, "{}", h.view_only);
    assert!(!h.editor.view.wheel.pending());
    h.pause(true);
    assert!(h.steps().is_empty());
}

#[test]
fn a_rating_after_a_slider_wheel_is_logged_after_its_step() {
    let mut h = WheelHistory::new();
    h.frame(vec![h.over(0)]);
    h.frame(vec![line(1.)]);
    h.editor.sync_undo();
    let logged = h.editor.undo_log.len().0;
    // As a rating or flag key does, before it logs itself.
    h.editor.finish_wheel_gesture();
    assert_eq!(h.editor.undo_log.len().0, logged + 1);
    assert!(!h.editor.view.wheel.pending());
}

#[test]
fn a_document_switch_mid_scroll_takes_nothing_to_the_next_photo() {
    let mut h = WheelHistory::new();
    h.frame(vec![h.over(0)]);
    h.frame(vec![line(1.)]);
    h.frame_with(vec![line(1.)], |e| {
        e.open_raw(std::path::PathBuf::from("missing-wheel-fixture.ARW"), None);
    });
    assert!(!h.editor.view.wheel.pending());
    assert!(h.editor.step_name.take_wheel().is_none());
    h.frame(vec![]);
    assert!(!h.editor.document.edit.history().can_undo());
}
