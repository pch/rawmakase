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
