//! Lightroom's Snapshots panel: named states of the open photo's edit, listed
//! alphabetically. + saves the current state and starts naming it; clicking one
//! applies it as a History step; its menu updates, renames or deletes it.
use crate::app::ui::design::{Tone, radius, text};
use crate::app::{Editor, history::Step, ui::theme};
use crate::catalog::{Snapshot, SnapshotSettings};
use eframe::egui::{self, Sense, Vec2};

/// The open photo's snapshots, and the one being named.
#[derive(Default)]
pub(in crate::app) struct Snapshots {
    pub(in crate::app) list: Vec<Snapshot>,
    renaming: Option<Renaming>,
}

/// A snapshot's name as it is being typed.
struct Renaming {
    id: i64,
    name: String,
    /// The field takes the keyboard once, when renaming starts.
    focus: Focus,
}

/// Whether the rename field still has to take the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Pending,
    Taken,
}

/// What the panel asked for this frame.
#[derive(Clone, Debug, PartialEq)]
enum SnapshotAction {
    New,
    Apply(i64),
    Update(i64),
    StartRename(i64),
    Rename(i64, String),
    Delete(i64),
    /// Copy Snapshot Settings to Before.
    ToBefore(i64),
}

impl Editor {
    /// Reads the open catalog photo's snapshots.
    pub(in crate::app) fn load_snapshots(&mut self) {
        let list = match (&self.library, self.document.catalog_photo) {
            (Some(l), Some(photo)) => l.session.catalog().snapshots(photo).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.document.snapshots.list = list;
    }
    /// The Snapshots panel, above History as in Lightroom. Only catalog photos keep
    /// snapshots.
    pub(in crate::app) fn snapshots_section(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() || self.document.catalog_photo.is_none() {
            return;
        }
        // A rename committed by clicking another snapshot comes first, then the click.
        let mut actions = Vec::new();
        let panel = &mut self.document.snapshots;
        let add = crate::app::ui::widgets::section_with(
            ui,
            "Snapshots",
            crate::app::ui::widgets::HeaderButton::Add,
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                if panel.list.is_empty() {
                    ui.label(
                        egui::RichText::new("+ saves the photo as it is now")
                            .size(text::SMALL)
                            .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
                    );
                }
                for snapshot in &panel.list {
                    if let Some(renaming) = panel.renaming.as_mut().filter(|r| r.id == snapshot.id)
                    {
                        let edit = ui.add_sized(
                            Vec2::new(ui.available_width(), 24.),
                            egui::TextEdit::singleline(&mut renaming.name),
                        );
                        if renaming.focus == Focus::Pending {
                            edit.request_focus();
                            renaming.focus = Focus::Taken;
                        }
                        if edit.lost_focus() {
                            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            actions.push(if cancel {
                                SnapshotAction::Rename(snapshot.id, snapshot.name.clone())
                            } else {
                                SnapshotAction::Rename(snapshot.id, renaming.name.clone())
                            });
                        }
                        continue;
                    }
                    // A click applies it, as in Lightroom; renaming is in its menu, so
                    // a rename never applies a snapshot first.
                    let row = snapshot_row(ui, &snapshot.name);
                    // Control-click opens the menu on macOS, as a right-click does.
                    if row.clicked() && !crate::app::ui::widgets::context_clicked(&row) {
                        actions.push(SnapshotAction::Apply(snapshot.id));
                    }
                    crate::app::ui::widgets::context_menu(&row, |ui| {
                        if ui.button("Update with Current Settings").clicked() {
                            actions.push(SnapshotAction::Update(snapshot.id));
                        }
                        if ui.button("Copy Snapshot Settings to Before").clicked() {
                            actions.push(SnapshotAction::ToBefore(snapshot.id));
                        }
                        if ui.button("Rename").clicked() {
                            actions.push(SnapshotAction::StartRename(snapshot.id));
                        }
                        if ui.button("Delete").clicked() {
                            actions.push(SnapshotAction::Delete(snapshot.id));
                        }
                    });
                }
            },
        );
        if add {
            actions.push(SnapshotAction::New);
        }
        for action in actions {
            self.snapshot_action(action);
        }
    }
    fn snapshot_action(&mut self, action: SnapshotAction) {
        let (Some(library), Some(photo)) = (&mut self.library, self.document.catalog_photo) else {
            return;
        };
        let session = &mut library.session;
        let recipe = self.document.edit.recipe();
        let result = match &action {
            SnapshotAction::New => {
                let name = format!("Snapshot {}", self.document.snapshots.list.len() + 1);
                session.add_snapshot(photo, &name, recipe).map(|id| {
                    self.document.snapshots.renaming = Some(Renaming {
                        id,
                        name,
                        focus: Focus::Pending,
                    });
                })
            }
            SnapshotAction::Update(id) => session.update_snapshot(*id, recipe),
            SnapshotAction::StartRename(id) => {
                let name = self
                    .snapshot(*id)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                self.document.snapshots.renaming = Some(Renaming {
                    id: *id,
                    name,
                    focus: Focus::Pending,
                });
                Ok(())
            }
            SnapshotAction::Rename(id, name) => {
                self.document.snapshots.renaming = None;
                // An emptied name keeps the old one.
                match name.trim() {
                    "" => Ok(()),
                    name => session.rename_snapshot(*id, name),
                }
            }
            SnapshotAction::Delete(id) => session.delete_snapshot(*id),
            SnapshotAction::ToBefore(id) => {
                if let Some(recipe) = self.snapshot_settings(*id) {
                    self.set_before(recipe);
                }
                return;
            }
            SnapshotAction::Apply(id) => {
                self.apply_snapshot(*id);
                return;
            }
        };
        if let Err(e) = result {
            self.messages.fail(format!("Snapshot not saved: {e:#}"));
        }
        self.load_snapshots();
    }
    /// Saves a name still being typed, before the photo is left.
    pub(in crate::app) fn commit_snapshot_rename(&mut self) {
        if let Some(renaming) = self.document.snapshots.renaming.take() {
            self.snapshot_action(SnapshotAction::Rename(renaming.id, renaming.name));
        }
    }
    fn snapshot(&self, id: i64) -> Option<&Snapshot> {
        self.document.snapshots.list.iter().find(|s| s.id == id)
    }
    /// Applies a snapshot as one History step, as Lightroom does.
    fn apply_snapshot(&mut self, id: i64) {
        let Some(name) = self.snapshot(id).map(|s| s.name.clone()) else {
            return;
        };
        let Some(recipe) = self.snapshot_settings(id) else {
            return;
        };
        // Already the edit: nothing to record, and no label left for the next step.
        if recipe == *self.document.edit.recipe() {
            return;
        }
        self.document
            .edit
            .name_next_step(Step::new(format!("Snapshot: {name}"), ""));
        self.document.edit.replace(recipe);
        self.ensure_analysis();
    }
    /// A snapshot's settings for this photo; one from Lightroom is converted, and
    /// what it could not render is said in the top bar.
    fn snapshot_settings(&mut self, id: i64) -> Option<crate::model::recipe::Recipe> {
        let snapshot = self.snapshot(id).cloned()?;
        let recipe = match snapshot.settings {
            SnapshotSettings::Recipe(recipe) => *recipe,
            SnapshotSettings::Lightroom(text) => {
                let m = self.document.metadata.as_ref()?;
                match crate::lr_develop::convert_develop(
                    &text,
                    m,
                    &self.document.profiles,
                    self.document
                        .measures()
                        .as_ref()
                        .map(|m| m as &dyn crate::xmp::PhotoMeasures),
                ) {
                    Ok((recipe, skipped)) => {
                        if !skipped.is_empty() {
                            self.messages
                                .warn(format!("Snapshot · not rendered: {}", skipped.join(", ")));
                        }
                        recipe
                    }
                    Err(e) => {
                        self.messages.fail(format!("Snapshot not applied: {e:#}"));
                        return None;
                    }
                }
            }
        };
        Some(recipe)
    }
}

/// A snapshot in the list, as History's rows look.
fn snapshot_row(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    }
    ui.painter()
        .with_clip_rect(rect.shrink2(Vec2::new(8., 0.)))
        .text(
            rect.left_center() + Vec2::new(8., 0.),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(text::LABEL),
            palette.tone(Tone::SOFT),
        );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Click to apply · right-click to update, rename or delete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Module;
    use crate::model::recipe::Recipe;

    #[test]
    fn a_snapshot_saves_names_and_restores_the_edit_as_a_history_step() -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let photos = d.path().join("photos");
        std::fs::create_dir(&photos)?;
        std::fs::write(photos.join("a.RAF"), b"snapshot fixture")?;
        let path = d.path().join("snapshots.rawmakase");
        crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let library = crate::app::library::Library::load(&path, ctx.clone())?;
        e.document.catalog_photo = Some(library.session.photos()[0].id);
        e.library = Some(Box::new(library));
        e.document.metadata = Some(Default::default());
        e.document.edit.setup_mut().exposure = 0.8;
        // + saves the edit as it is and starts naming it.
        e.snapshot_action(SnapshotAction::New);
        let id = e.document.snapshots.list[0].id;
        assert_eq!(e.document.snapshots.list[0].name, "Snapshot 1");
        e.snapshot_action(SnapshotAction::Rename(id, "Bright".into()));
        assert_eq!(e.document.snapshots.list[0].name, "Bright");
        // An emptied name keeps the old one.
        e.snapshot_action(SnapshotAction::Rename(id, "  ".into()));
        assert_eq!(e.document.snapshots.list[0].name, "Bright");
        // Applying it is one History step, which Undo takes back.
        e.document.edit.replace(Recipe::default());
        let before = e.document.edit.recipe().clone();
        e.snapshot_action(SnapshotAction::Apply(id));
        e.commit_edit(before, None);
        assert_eq!(e.document.edit.recipe().exposure, 0.8);
        let (steps, _) = e.document.edit.history().steps();
        assert_eq!(steps.last().unwrap().name, "Snapshot: Bright");
        // Update with Current Settings, then Delete.
        e.document.edit.setup_mut().exposure = -0.5;
        e.snapshot_action(SnapshotAction::Update(id));
        e.document.edit.replace(Recipe::default());
        e.snapshot_action(SnapshotAction::Apply(id));
        assert_eq!(e.document.edit.recipe().exposure, -0.5);
        e.snapshot_action(SnapshotAction::Delete(id));
        assert!(e.document.snapshots.list.is_empty());
        // Typing a name and pressing Return in the panel names the new snapshot.
        e.module = Module::Develop;
        e.set_develop_tab(crate::app::shell::panels::DevelopTab::History);
        e.snapshot_action(SnapshotAction::New);
        let mut frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1400., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| e.draw(ui),
            );
            output.textures_delta.clear();
        };
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![]);
        frame(vec![egui::Event::Text(" warm".into())]);
        frame(vec![
            key(egui::Key::Enter, true),
            key(egui::Key::Enter, false),
        ]);
        frame(vec![]);
        assert!(e.document.snapshots.renaming.is_none());
        assert_eq!(e.document.snapshots.list[0].name, "Snapshot 1 warm");
        // A name still being typed is kept when the photo is left.
        e.snapshot_action(SnapshotAction::StartRename(e.document.snapshots.list[0].id));
        e.document.snapshots.renaming.as_mut().unwrap().name = "Kept".into();
        e.commit_snapshot_rename();
        assert_eq!(e.document.snapshots.list[0].name, "Kept");
        // One being typed goes on while a tool's drawer opens: History stays.
        e.snapshot_action(SnapshotAction::StartRename(e.document.snapshots.list[0].id));
        e.document.snapshots.renaming.as_mut().unwrap().name = "Before crop".into();
        e.view.tool = crate::app::develop::state::Tool::Crop;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1400., 900.),
                )),
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        assert_eq!(
            e.document
                .snapshots
                .renaming
                .as_ref()
                .map(|r| r.name.as_str()),
            Some("Before crop")
        );
        Ok(())
    }
}
