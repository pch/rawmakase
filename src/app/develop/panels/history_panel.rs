//! Develop's History panel: this session's steps over the photo's imported
//! Lightroom steps, drawn under Snapshots in the History tab.
use crate::app::Editor;
use crate::app::ui::design::{Tone, radius, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::section;
use eframe::egui::{self, Sense, Vec2};

impl Editor {
    /// Lightroom's History panel: this session's steps, newest first, over
    /// the photo's imported Lightroom steps. Clicking a step of this session
    /// goes back (or forward) to it; later steps stay until the next edit.
    pub(in crate::app) fn history_section(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() {
            return;
        }
        let mut go_to = None;
        let mut to_before = None;
        let mut lightroom = None;
        let (steps, applied) = self.document.edit.history().steps();
        // A click goes to the step; its menu copies it to Before, as in Lightroom.
        let mut row = |ui: &mut egui::Ui, n: usize, name: &str, value: &str| {
            let response = history_row(ui, name, value, n == applied, n > applied);
            if response.clicked() && !crate::app::ui::widgets::context_clicked(&response) {
                go_to = Some(n);
            }
            crate::app::ui::widgets::context_menu(&response, |ui| {
                ui.set_width(270.);
                if crate::app::ui::widgets::menu_item(
                    ui,
                    "Copy History Step Settings to Before",
                    "",
                    true,
                    false,
                ) {
                    to_before = Some(n);
                    ui.close();
                }
            });
        };
        section(ui, "History", |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            for (i, step) in steps.iter().enumerate().rev() {
                row(ui, i + 1, &step.name, &step.value);
            }
            let opened = if self.document.lightroom_history.is_empty() {
                "Opened"
            } else {
                "Opened with Lightroom edit"
            };
            row(ui, 0, opened, "");
            if self.document.lightroom_history.is_empty() {
                return;
            }
            ui.add_space(space::L);
            ui.label(
                egui::RichText::new("From Lightroom")
                    .size(text::SMALL)
                    .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
            );
            ui.add_space(space::XS);
            for (i, step) in self.document.lightroom_history.iter().enumerate().rev() {
                let name = if step.name.is_empty() {
                    "Edit"
                } else {
                    step.name.as_str()
                };
                let response = history_row(ui, name, "", false, false);
                let response = match step.created {
                    // Lightroom counts seconds from 2001-01-01 UTC.
                    Some(s) => response
                        .on_hover_text(format!("{} UTC", format_unix(s as i64 + 978_307_200))),
                    None => response,
                };
                if response.clicked() && !crate::app::ui::widgets::context_clicked(&response) {
                    lightroom = Some((i, LightroomStep::Apply));
                }
                crate::app::ui::widgets::context_menu(&response, |ui| {
                    ui.set_width(270.);
                    if crate::app::ui::widgets::menu_item(
                        ui,
                        "Copy History Step Settings to Before",
                        "",
                        true,
                        false,
                    ) {
                        lightroom = Some((i, LightroomStep::ToBefore));
                        ui.close();
                    }
                });
            }
        });
        if let Some(n) = to_before {
            self.before_from_history(n);
        }
        if let Some(n) = go_to {
            // A resize still being grouped is a step before the jump, so it isn't lost.
            self.finish_wheel_gesture();
            if self.document.edit.jump(n) {
                self.ensure_analysis();
            }
        }
        if let Some((i, use_step)) = lightroom
            && let Some(m) = &self.document.metadata
        {
            let step = &self.document.lightroom_history[i];
            match crate::lr_develop::convert_develop(
                &step.text,
                m,
                &self.document.profiles,
                self.document
                    .measures()
                    .as_ref()
                    .map(|m| m as &dyn crate::xmp::PhotoMeasures),
            ) {
                Ok((recipe, skipped)) if use_step == LightroomStep::ToBefore => {
                    if !skipped.is_empty() {
                        self.messages
                            .note(format!("Before · not rendered: {}", skipped.join(", ")));
                    }
                    self.set_before(recipe);
                }
                Ok((recipe, skipped)) => {
                    let name = format!("Lightroom: {}", step.name);
                    if skipped.is_empty() {
                        self.messages.note(name.clone());
                    } else {
                        self.messages
                            .warn(format!("{name} · not rendered: {}", skipped.join(", ")));
                    }
                    self.document
                        .edit
                        .name_next_step(crate::app::history::Step::new(name, ""));
                    self.document.edit.replace(recipe);
                }
                Err(e) => self
                    .messages
                    .fail(format!("History step not applied: {e:#}")),
            }
        }
    }
}
/// What a click on a Lightroom History step asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LightroomStep {
    /// Apply it to the edit, as a step.
    Apply,
    /// Copy History Step Settings to Before.
    ToBefore,
}
/// A History row: the step on the left, its value on the right. The current
/// step is highlighted; steps after it (undone) are dimmed, as in Lightroom.
fn history_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &str,
    current: bool,
    undone: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::click());
    if current {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.accent());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    }
    let color = if current {
        palette.on_accent_text(Tone::BRIGHT)
    } else {
        palette.tone(if undone { Tone::FAINT } else { Tone::SOFT })
    };
    let value_width = if value.is_empty() {
        0.
    } else {
        let galley = ui.painter().layout_no_wrap(
            value.to_string(),
            egui::FontId::proportional(text::LABEL),
            color,
        );
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(
                rect.right() - 8. - width,
                rect.center().y - galley.size().y / 2.,
            ),
            galley,
            color,
        );
        width + 16.
    };
    let clip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - 8. - value_width, rect.bottom()),
    );
    ui.painter().with_clip_rect(clip).text(
        rect.left_center() + Vec2::new(8., 0.),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(text::LABEL),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// "2016-07-28 06:14" from Unix seconds.
fn format_unix(seconds: i64) -> String {
    let [year, month, day, hour, minute, _] = crate::time::utc(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}
#[cfg(test)]
#[test]
fn formats_lightroom_history_dates() {
    assert_eq!(format_unix(1_469_686_444), "2016-07-28 06:14");
}
