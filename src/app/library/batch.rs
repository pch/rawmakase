//! Batch edits of the Library's selection: Quick Develop's Saved Preset and Auto
//! Settings, as in Lightroom's Library, and the window every batch edit (these, Sync
//! Settings and Match Total Exposures) shows while it runs, with its Cancel. The
//! batch itself runs on the Sync path (see `sync`).
use crate::app::Editor;
use crate::app::develop::state::PresetBrowser;
use crate::app::library::sync::{BatchChange, BatchPreset, BatchStage};
use crate::app::ui::design::{self, Tone, inset, space, text};
use crate::app::ui::theme;
use eframe::egui::{self, Vec2};
use std::time::Duration;

/// How long a batch runs before its window shows: a short one is over before it
/// would flash up.
const SHOW_AFTER: Duration = Duration::from_millis(300);
/// How often the window redraws while the batch runs; the worker only wakes the
/// window when it is done.
const REDRAW: Duration = Duration::from_millis(100);

/// What Quick Develop asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum QuickRequest {
    AutoSettings,
    /// Preset `i` of the preset library.
    Preset(usize),
}

/// Quick Develop's batch controls, under Open in Develop: a Saved Preset chooser and
/// Auto Settings, for the `count` photos selected.
pub(in crate::app) fn quick_develop(
    ui: &mut egui::Ui,
    presets: &mut PresetBrowser,
    count: usize,
) -> Option<QuickRequest> {
    let mut request = None;
    let width = ui.available_width();
    let photos = crate::app::ui::widgets::plural(count, "selected photo", "selected photos");
    ui.add_space(space::XS);
    let chooser = ui
        .add_enabled(
            count > 0 && !presets.library.presets.is_empty(),
            egui::Button::new("Saved Preset…").min_size(Vec2::new(width, 24.)),
        )
        .on_hover_text(format!("Apply a Develop preset to the {photos}"))
        .on_disabled_hover_text(if count == 0 {
            "Select the photos to apply a preset to"
        } else {
            "No presets yet: import them in Develop's Presets panel"
        });
    egui::Popup::menu(&chooser)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(260.);
            if let Some(i) = preset_chooser(ui, presets) {
                request = Some(QuickRequest::Preset(i));
                ui.close();
            }
        });
    ui.add_space(space::XS);
    let keys = if cfg!(target_os = "macos") {
        "⇧⌘U"
    } else {
        "Ctrl+Shift+U"
    };
    if ui
        .add_enabled(
            count > 0,
            egui::Button::new("Auto Settings").min_size(Vec2::new(width, 24.)),
        )
        .on_hover_text(format!(
            "Set Auto's tone, Vibrance and Saturation on each of the {photos} · {keys}"
        ))
        .on_disabled_hover_text("Select the photos to apply Auto Settings to")
        .clicked()
    {
        request = Some(QuickRequest::AutoSettings);
    }
    request
}

/// The chooser's list: a search, then favorites, then every group as the Presets
/// panel orders them. Returns the preset clicked.
fn preset_chooser(ui: &mut egui::Ui, presets: &mut PresetBrowser) -> Option<usize> {
    let palette = theme::palette(ui.ctx());
    crate::app::ui::widgets::search_field(ui, &mut presets.chooser_filter, "Search presets");
    let library = presets.library.clone();
    let favorites =
        crate::app::presets::preset_groups(&library, &presets.chooser_filter, |_, p| {
            presets.favorites.contains(&p.id)
        });
    let groups = crate::app::presets::preset_groups(&library, &presets.chooser_filter, |_, _| true);
    let favorites: Vec<(usize, String)> = favorites.into_iter().flat_map(|g| g.presets).collect();
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("preset-chooser")
        .max_height(360.)
        .show(ui, |ui| {
            if groups.is_empty() {
                ui.label(
                    egui::RichText::new("No presets match")
                        .size(text::SMALL)
                        .color(palette.tone(Tone::FAINT)),
                );
            }
            let sections = (!favorites.is_empty())
                .then_some(("Favorites", &favorites))
                .into_iter()
                .chain(groups.iter().map(|g| (g.name.as_str(), &g.presets)));
            for (group, list) in sections {
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(group)
                        .size(text::SMALL)
                        .color(palette.tone(Tone::MUTED)),
                );
                for (i, name) in list {
                    if ui
                        .add(egui::Button::new(name.as_str()).frame(false))
                        .clicked()
                    {
                        clicked = Some(*i);
                    }
                }
            }
        });
    clicked
}

impl Editor {
    /// Starts what Quick Develop asked for on every selected photo.
    pub(in crate::app) fn quick_develop_request(&mut self, request: QuickRequest) {
        let change = match request {
            QuickRequest::AutoSettings => BatchChange::AutoSettings,
            QuickRequest::Preset(i) => {
                // Read now: a scan finishing while the batch runs does not change it.
                let Some(preset) = self.presets.library.presets.get(i) else {
                    return;
                };
                BatchChange::Preset(Box::new(BatchPreset {
                    name: crate::presets::display_name(&preset.name),
                    preset: preset.clone(),
                }))
            }
        };
        self.start_batch(change);
    }

    /// The running batch's progress, and its Cancel, which stays usable while the
    /// rest of the window waits for the batch. Cancel is refused once the batch is
    /// saving; the batch ends when its worker reports back either way.
    pub(in crate::app) fn batch_window(&mut self, ctx: &egui::Context) {
        let Some(progress) = self.activity.batch() else {
            return;
        };
        let elapsed = progress.elapsed();
        if elapsed < SHOW_AFTER {
            ctx.request_repaint_after(SHOW_AFTER - elapsed);
            return;
        }
        let palette = theme::palette(ctx);
        let (done, total, current) = progress.counts();
        let stage = progress.stage();
        let line = match stage {
            BatchStage::Preparing => format!("{} / {total} — {current}", (done + 1).min(total)),
            BatchStage::Cancelling => "Cancelling…".into(),
            BatchStage::Saving => "Saving…".into(),
        };
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("batch-progress"))
            .frame(
                crate::app::ui::widgets::modal_frame(&palette)
                    .inner_margin(design::margin_same(inset::DIALOG)),
            )
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.label(
                    egui::RichText::new(progress.name())
                        .size(text::LARGE)
                        .color(palette.tone(Tone::TEXT)),
                );
                ui.add_space(space::L);
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32));
                ui.add_space(space::S);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(line)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::LABEL)),
                    )
                    .truncate(),
                );
                ui.add_space(space::XL);
                cancel = ui
                    .add_enabled(stage == BatchStage::Preparing, egui::Button::new("Cancel"))
                    .on_disabled_hover_text("Saving: it can no longer be cancelled")
                    .clicked();
            });
        if cancel {
            progress.cancel();
        }
        ctx.request_repaint_after(REDRAW);
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
