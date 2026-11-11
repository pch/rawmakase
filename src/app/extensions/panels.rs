//! Host-drawn panels: a program's control tree, drawn with the app's own
//! widgets on its extension's page in Preferences.
//!
//! The program returns the whole tree when a panel opens and after each
//! change, and may push one with `panel-update`. A value the user is still
//! editing (a slider being dragged, text being typed) is kept as a draft and
//! sent once committed: a slider when it is let go, text on Enter or when the
//! field loses focus, a choice or toggle at once. What a frame commits is sent
//! before the clicks of that frame. A new tree from the program
//! replaces every draft. A validation error comes back as the control's
//! `help`, drawn under it.
use super::PanelView;
use crate::app::preferences::hint;
use crate::app::ui::theme;
use crate::app::ui::widgets::form_row;
use eframe::egui;
use rawmakase_extensions::protocol::controls::{Control, ControlKind, validate};
use serde_json::Value;
use std::collections::HashMap;

/// What the user did to a panel, for its program.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Action {
    Changed {
        control: String,
        value: Value,
    },
    Clicked {
        control: String,
    },
    /// A file or folder control's Choose… button: the system's dialog picks it.
    Choose {
        control: String,
        folder: bool,
        extensions: Vec<String>,
    },
}

/// Takes a tree the program sent, when it validates.
pub(super) fn take(view: &mut PanelView, controls: Vec<Control>) {
    view.waiting = false;
    match validate(&controls) {
        Ok(()) => {
            view.controls = controls;
            view.error = None;
            view.drafts.clear();
        }
        Err(why) => {
            view.error = Some(format!(
                "The extension sent a panel that can't be drawn: {why}"
            ))
        }
    }
}

/// Every control's value by id, for `visible_when`.
fn values(controls: &[Control], out: &mut HashMap<String, Value>) {
    for c in controls {
        if let ControlKind::Section { controls, .. } = &c.kind {
            values(controls, out);
        }
        if let (Some(id), Some(value)) = (&c.id, value_of(&c.kind)) {
            out.insert(id.clone(), value);
        }
    }
}

fn value_of(kind: &ControlKind) -> Option<Value> {
    Some(match kind {
        ControlKind::Slider { value, .. }
        | ControlKind::Angle { value }
        | ControlKind::Number { value, .. } => Value::from(*value),
        ControlKind::Toggle { value } => Value::from(*value),
        ControlKind::Choice { value, .. } | ControlKind::Segmented { value, .. } => {
            Value::from(value.clone())
        }
        ControlKind::Text { value, .. } => Value::from(value.clone()),
        ControlKind::Color { value, .. } => Value::from(value.clone()),
        ControlKind::File { value, .. } | ControlKind::Folder { value } => {
            value.clone().map_or(Value::Null, Value::from)
        }
        _ => return None,
    })
}

/// Draws `view`'s controls and returns what the user committed.
pub(in crate::app) fn draw(ui: &mut egui::Ui, salt: &str, view: &mut PanelView) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut all = HashMap::new();
    values(&view.controls, &mut all);
    let controls = view.controls.clone();
    draw_list(ui, salt, &controls, &all, &mut view.drafts, &mut actions);
    // A field loses focus to the click that ends its editing, so what it
    // commits goes first, whichever is drawn first: the click then acts on
    // the value the user left, and a Reset isn't undone by text sent after it.
    actions.sort_by_key(|action| !matches!(action, Action::Changed { .. }));
    actions
}

fn draw_list(
    ui: &mut egui::Ui,
    salt: &str,
    controls: &[Control],
    all: &HashMap<String, Value>,
    drafts: &mut HashMap<String, Value>,
    actions: &mut Vec<Action>,
) {
    for control in controls {
        if let Some(when) = &control.visible_when
            && all.get(&when.control) != Some(&when.equals)
        {
            continue;
        }
        ui.add_enabled_ui(!control.disabled, |ui| {
            draw_one(ui, salt, control, all, drafts, actions);
        });
        if let Some(help) = &control.help {
            form_row(ui, "", |ui| hint(ui, help));
        }
    }
}

fn draw_one(
    ui: &mut egui::Ui,
    salt: &str,
    control: &Control,
    all: &HashMap<String, Value>,
    drafts: &mut HashMap<String, Value>,
    actions: &mut Vec<Action>,
) {
    let id = control.id.clone().unwrap_or_default();
    let label = control.label.as_deref().unwrap_or("");
    let changed = |actions: &mut Vec<Action>, value: Value| {
        actions.push(Action::Changed {
            control: id.clone(),
            value,
        });
    };
    match &control.kind {
        ControlKind::Section {
            title,
            controls,
            collapsed,
        } => {
            egui::CollapsingHeader::new(title)
                .id_salt((salt, title, id.as_str()))
                .default_open(!collapsed)
                .show(ui, |ui| draw_list(ui, salt, controls, all, drafts, actions));
        }
        ControlKind::Slider {
            value,
            min,
            max,
            step,
            unit,
            ..
        } => form_row(ui, label, |ui| {
            let mut v = drafts.get(&id).and_then(Value::as_f64).unwrap_or(*value);
            let mut slider = egui::Slider::new(&mut v, *min..=*max);
            if let Some(step) = step {
                slider = slider.step_by(*step);
            }
            if let Some(unit) = unit {
                slider = slider.suffix(format!(" {unit}"));
            }
            let r = ui.add(slider);
            if r.changed() {
                drafts.insert(id.clone(), Value::from(v));
            }
            if (r.drag_stopped() || (r.changed() && !r.dragged())) && v != *value {
                changed(actions, Value::from(v));
            }
        }),
        ControlKind::Angle { value } => form_row(ui, label, |ui| {
            let mut v = drafts.get(&id).and_then(Value::as_f64).unwrap_or(*value);
            let r = ui.add(egui::DragValue::new(&mut v).suffix("°").speed(0.1));
            if r.changed() {
                drafts.insert(id.clone(), Value::from(v));
            }
            if (r.drag_stopped() || r.lost_focus()) && v != *value {
                changed(actions, Value::from(v));
            }
        }),
        ControlKind::Number { value, min, max } => form_row(ui, label, |ui| {
            let mut v = drafts.get(&id).and_then(Value::as_f64).unwrap_or(*value);
            let mut drag = egui::DragValue::new(&mut v);
            if let (Some(min), Some(max)) = (min, max) {
                drag = drag.range(*min..=*max);
            }
            let r = ui.add(drag);
            if r.changed() {
                drafts.insert(id.clone(), Value::from(v));
            }
            if (r.drag_stopped() || r.lost_focus()) && v != *value {
                changed(actions, Value::from(v));
            }
        }),
        ControlKind::Toggle { value } => form_row(ui, label, |ui| {
            let mut v = *value;
            if ui.checkbox(&mut v, "").changed() {
                changed(actions, Value::from(v));
            }
        }),
        ControlKind::Choice { options, value } => form_row(ui, label, |ui| {
            let shown = options
                .iter()
                .find(|o| &o.id == value)
                .map_or(value.as_str(), |o| o.title.as_str());
            egui::ComboBox::from_id_salt((salt, id.as_str()))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for option in options {
                        if ui
                            .selectable_label(&option.id == value, &option.title)
                            .clicked()
                            && &option.id != value
                        {
                            changed(actions, Value::from(option.id.clone()));
                        }
                    }
                });
        }),
        ControlKind::Segmented { options, value } => form_row(ui, label, |ui| {
            for option in options {
                if ui
                    .selectable_label(&option.id == value, &option.title)
                    .clicked()
                    && &option.id != value
                {
                    changed(actions, Value::from(option.id.clone()));
                }
            }
        }),
        ControlKind::Color { value, alpha } => form_row(ui, label, |ui| {
            let draft: Vec<f32> = drafts
                .get(&id)
                .and_then(|v| serde_json::from_value::<Vec<f32>>(v.clone()).ok())
                .unwrap_or_else(|| value.iter().map(|c| *c as f32).collect());
            let mut color = [
                draft.first().copied().unwrap_or(0.),
                draft.get(1).copied().unwrap_or(0.),
                draft.get(2).copied().unwrap_or(0.),
                draft.get(3).copied().unwrap_or(1.),
            ];
            let r = if *alpha {
                ui.color_edit_button_rgba_unmultiplied(&mut color)
            } else {
                let mut rgb = [color[0], color[1], color[2]];
                let r = ui.color_edit_button_rgb(&mut rgb);
                color[..3].copy_from_slice(&rgb);
                r
            };
            let n = if *alpha { 4 } else { 3 };
            let now: Vec<f64> = color[..n].iter().map(|c| f64::from(*c)).collect();
            if r.changed() {
                drafts.insert(id.clone(), Value::from(now.clone()));
            }
            // Sent once the picker lets go, not for every step of a drag.
            let released = !ui.input(|i| i.pointer.any_down());
            if released && drafts.contains_key(&id) && now != *value {
                drafts.remove(&id);
                changed(actions, Value::from(now));
            }
        }),
        ControlKind::Curve { points } => form_row(ui, label, |ui| {
            hint(
                ui,
                &format!(
                    "A curve through {} points (shown, not edited, here)",
                    points.len()
                ),
            );
        }),
        ControlKind::Text { value, multiline } => form_row(ui, label, |ui| {
            let mut text = drafts
                .get(&id)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| value.clone());
            let edit = if *multiline {
                egui::TextEdit::multiline(&mut text)
            } else {
                egui::TextEdit::singleline(&mut text)
            };
            let r = ui.add(edit.desired_width(f32::INFINITY));
            if r.changed() {
                drafts.insert(id.clone(), Value::from(text.clone()));
            }
            // Enter ends a one-line field's focus too.
            if r.lost_focus() && text != *value {
                changed(actions, Value::from(text));
            }
        }),
        ControlKind::File { value, extensions } => form_row(ui, label, |ui| {
            path_value(ui, value.as_deref());
            if ui.button("Choose…").clicked() {
                actions.push(Action::Choose {
                    control: id.clone(),
                    folder: false,
                    extensions: extensions.clone(),
                });
            }
        }),
        ControlKind::Folder { value } => form_row(ui, label, |ui| {
            path_value(ui, value.as_deref());
            if ui.button("Choose…").clicked() {
                actions.push(Action::Choose {
                    control: id.clone(),
                    folder: true,
                    extensions: Vec::new(),
                });
            }
        }),
        ControlKind::Button { title } => form_row(ui, label, |ui| {
            if ui.button(title).clicked() {
                actions.push(Action::Clicked {
                    control: id.clone(),
                });
            }
        }),
        ControlKind::Label { text } => form_row(ui, label, |ui| {
            ui.add(egui::Label::new(text).wrap());
        }),
        ControlKind::Help { text } => form_row(ui, label, |ui| hint(ui, text)),
        ControlKind::Progress { value } => form_row(ui, label, |ui| {
            let bar = match value {
                Some(v) => egui::ProgressBar::new(*v as f32).show_percentage(),
                None => egui::ProgressBar::new(0.).animate(true),
            };
            ui.add(bar.desired_width(ui.available_width().min(240.)));
        }),
        // The thumbnail's file, until panels load images.
        ControlKind::Image { path } => form_row(ui, label, |ui| {
            ui.add(egui::Label::new(path).truncate());
        }),
    }
}

fn path_value(ui: &mut egui::Ui, path: Option<&str>) {
    let text = path.unwrap_or("None chosen");
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .color(theme::palette(ui.ctx()).tone(crate::app::ui::design::Tone::TEXT)),
        )
        .truncate(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::{Node, TreeUpdate};
    use serde_json::json;

    /// A frame of `view` given `events`: what it committed, and where its
    /// controls are.
    fn frame(
        ctx: &egui::Context,
        view: &mut PanelView,
        events: Vec<egui::Event>,
    ) -> (Vec<Action>, TreeUpdate) {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                events,
                ..Default::default()
            },
            |ui| actions = draw(ui, "test", view),
        );
        output.textures_delta.clear();
        let tree = output
            .platform_output
            .accesskit_update
            .expect("accessibility is on");
        (actions, tree)
    }

    fn centre(tree: &TreeUpdate, what: &str, is: impl Fn(&Node) -> bool) -> egui::Pos2 {
        let bounds = tree
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| is(n))
            .and_then(|n| n.bounds())
            .unwrap_or_else(|| panic!("{what} is drawn"));
        egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.) as f32,
            ((bounds.y0 + bounds.y1) / 2.) as f32,
        )
    }

    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    /// Text typed and not yet committed is sent before a click that takes
    /// the focus from it, whichever is drawn first: the click acts on the
    /// value the user left, so a Reset clicked after typing isn't undone by
    /// the text arriving after it.
    #[test]
    fn a_draft_is_committed_before_the_click_that_ends_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        let controls: Vec<Control> = serde_json::from_value(json!([
            {"id": "reset", "type": "button", "title": "Reset"},
            {"id": "keys", "type": "text", "label": "Action", "value": ""},
        ]))
        .unwrap();
        let mut view = PanelView {
            controls,
            ..Default::default()
        };
        let (_, tree) = frame(&ctx, &mut view, Vec::new());
        let field = centre(&tree, "the text field", |n| {
            n.role() == egui::accesskit::Role::TextInput
        });
        let reset = centre(&tree, "Reset", |n| n.label() == Some("Reset"));
        frame(&ctx, &mut view, click(field));
        let (typed, _) = frame(&ctx, &mut view, vec![egui::Event::Text("undo".into())]);
        assert!(typed.is_empty(), "{typed:?}");
        let (actions, _) = frame(&ctx, &mut view, click(reset));
        assert_eq!(
            actions,
            [
                Action::Changed {
                    control: "keys".into(),
                    value: json!("undo"),
                },
                Action::Clicked {
                    control: "reset".into(),
                },
            ]
        );
    }
}
