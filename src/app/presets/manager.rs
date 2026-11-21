//! Manage Presets, opened by the Presets panel's +: the groups in the order the
//! panel shows them, dragged to reorder; any preset or whole group hidden from the
//! panel, its files left where they are; presets made here or imported deleted,
//! renamed as a group, or dragged onto another group, as in Lightroom; any preset
//! or group exported (a group as a .zip); and Import.
use crate::app::Editor;
use crate::app::presets::bulk_import::ImportKind;
use crate::app::presets::{PresetGroup, preset_groups};
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::design::{self, Tone, button, inset, radius, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{destructive_choice, modal_frame, plural, primary_button};
use crate::app::worker::Event;
use crate::presets::PresetLayout;
use crate::presets::user::UserPresets;
use anyhow::Context as _;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The window's state: which groups are open, and a deletion waiting for its
/// confirmation, shown in the window's place.
#[derive(Default)]
pub(in crate::app) struct PresetManager {
    open: BTreeSet<String>,
    confirm: Option<Removal>,
    /// The group or preset (`preset:` and its id) being renamed in place: its key
    /// and the name being typed.
    rename: Option<(String, String)>,
}

/// A preset being dragged onto a group, by its index in the library.
struct PresetDrag(usize);

/// Presets to delete, by file, and how the confirmation names them.
struct Removal {
    title: String,
    paths: Vec<PathBuf>,
}

/// What was done in the window this frame.
enum Change {
    /// Group `from` dropped before the group now at `to` (`to` may be the end).
    Move {
        from: usize,
        to: usize,
    },
    /// The presets `ids` hidden, or shown again.
    Hide {
        ids: Vec<String>,
        hide: bool,
    },
    Toggle(String),
    Delete(Removal),
    /// A group's or preset's name to be typed in place, by its key.
    StartRename(String, String),
    CancelRename,
    RenamePreset {
        path: PathBuf,
        name: String,
    },
    /// The presets `paths` moved to `group`; `from` is the group renamed, if any.
    MoveTo {
        paths: Vec<PathBuf>,
        group: String,
        from: Option<String>,
    },
    /// The presets `paths` written to a file the user chooses, as a .zip for a group.
    Export {
        paths: Vec<PathBuf>,
        name: String,
        zip: bool,
    },
    Import(FileDialog),
    ResetOrder,
    Done,
}

const ROW: f32 = 28.;

impl Editor {
    /// The Manage Presets window, while it is open.
    pub(in crate::app) fn preset_manager_window(&mut self, ctx: &egui::Context) {
        if !matches!(self.modal, Some(crate::app::Modal::ManagePresets(_))) {
            return;
        }
        let Some(crate::app::Modal::ManagePresets(mut manager)) = self.modal.take() else {
            unreachable!("matched above");
        };
        if self.manage_presets(&mut manager, ctx) {
            self.modal = Some(crate::app::Modal::ManagePresets(manager));
        }
    }
    /// Shows the window, or the confirmation in its place; returns whether it
    /// stays open.
    fn manage_presets(&mut self, manager: &mut PresetManager, ctx: &egui::Context) -> bool {
        if let Some(removal) = &manager.confirm {
            let lines = [
                format!(
                    "{} will be deleted from disk.",
                    plural(removal.paths.len(), "preset file", "preset files")
                ),
                "This can't be undone.".to_string(),
            ];
            if let Some(choice) = destructive_choice(
                ctx,
                "delete-presets",
                &removal.title,
                &lines,
                "Cancel",
                &["Delete"],
            ) {
                let removal = manager.confirm.take();
                if let (Some(_), Some(removal)) = (choice, removal) {
                    self.delete_presets(&removal.paths, ctx);
                }
            }
            return true;
        }
        let library = self.presets.library.clone();
        let layout = &self.presets.layout;
        let groups = preset_groups(&library, "", layout, |_, _| true);
        let user = UserPresets::default();
        let palette = theme::palette(ctx);
        let mut change = None;
        let response = egui::Modal::new(egui::Id::new("manage-presets"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                crate::app::ui::widgets::name_dialog(ui, "Manage Presets");
                ui.set_width(480.);
                ui.label(
                    egui::RichText::new("Manage Presets")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::XS);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Drag groups to set their order in the Presets panel, and your \
                             presets onto your groups to move them. Hidden presets stay on disk. \
                             Right-click a row or use \u{22ef} to rename, export or delete.",
                        )
                        .size(text::SMALL)
                        .color(palette.tone(Tone::SECONDARY)),
                    )
                    .wrap(),
                );
                ui.add_space(space::L);
                egui::Frame::new()
                    .fill(palette.tone(Tone::WELL))
                    .corner_radius(radius::CARD)
                    .inner_margin(design::margin_same(space::XS))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height((ctx.content_rect().height() - 260.).clamp(160., 460.))
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 0.;
                                change = group_list(ui, &groups, &library, layout, &user, manager);
                            });
                    });
                ui.add_space(space::XL);
                // One height for all four, so they line up.
                let size = Vec2::new(0., 30.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui
                        .add(egui::Button::new("Import Presets…").min_size(size))
                        .clicked()
                    {
                        change = Some(Change::Import(FileDialog::ImportXmp));
                    }
                    if ui
                        .add(egui::Button::new("Import Folder…").min_size(size))
                        .on_hover_text("Import every preset in a folder and its subfolders")
                        .clicked()
                    {
                        change = Some(Change::Import(FileDialog::ImportFolder(
                            ImportKind::Presets,
                        )));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Done").clicked() {
                            change = Some(Change::Done);
                        }
                        if ui
                            .add_enabled(
                                !layout.order.is_empty(),
                                egui::Button::new("Reset Order").min_size(size),
                            )
                            .on_hover_text("Built-in groups first, then the others by name")
                            .clicked()
                        {
                            change = Some(Change::ResetOrder);
                        }
                    });
                });
            });
        if response.should_close() && change.is_none() {
            change = Some(Change::Done);
        }
        let Some(change) = change else {
            return true;
        };
        let mut layout = self.presets.layout.clone();
        match change {
            Change::Toggle(key) => {
                if !manager.open.remove(&key) {
                    manager.open.insert(key);
                }
                return true;
            }
            Change::Delete(removal) => {
                manager.confirm = Some(removal);
                return true;
            }
            Change::StartRename(key, name) => {
                manager.rename = Some((key, name));
                return true;
            }
            Change::CancelRename => {
                manager.rename = None;
                return true;
            }
            Change::RenamePreset { path, name } => {
                manager.rename = None;
                self.rename_preset(&path, &name, ctx);
                return true;
            }
            Change::MoveTo { paths, group, from } => {
                manager.rename = None;
                self.move_presets(&paths, &group, from, ctx);
                return true;
            }
            Change::Export { paths, name, zip } => {
                self.export_presets(paths, name, zip, ctx);
                return true;
            }
            Change::Done => return false,
            Change::Import(dialog) => {
                self.dialog(dialog, ctx);
                return false;
            }
            Change::Move { from, to } => {
                let mut keys: Vec<String> = groups.iter().map(PresetGroup::key).collect();
                let key = keys.remove(from);
                keys.insert(if to > from { to - 1 } else { to }, key);
                layout.order = keys;
            }
            Change::Hide { ids, hide } => {
                for id in ids {
                    if hide {
                        layout.hidden.insert(id);
                    } else {
                        layout.hidden.remove(&id);
                    }
                }
            }
            Change::ResetOrder => layout.order.clear(),
        }
        self.set_preset_layout(layout);
        true
    }
    fn set_preset_layout(&mut self, layout: PresetLayout) {
        if let Err(e) = crate::presets::save_layout(&layout) {
            self.messages.fail(format!("Preset order not saved: {e:#}"));
        }
        self.presets.layout = layout;
        self.presets.revision += 1;
    }
    /// Moves the presets at `paths` to `group`, as dropping one on a group or renaming
    /// the group `from` does; favorites, hidden presets, raw defaults and the
    /// group's place follow them.
    fn move_presets(
        &mut self,
        paths: &[PathBuf],
        group: &str,
        from: Option<String>,
        ctx: &egui::Context,
    ) {
        let library = self.presets.library.clone();
        let user = UserPresets::default();
        let mut layout = self.presets.layout.clone();
        let mut defaults = self.raw_defaults.settings().clone();
        let (mut moved, mut failed) = (0, Vec::new());
        for p in library.presets.iter().filter(|p| paths.contains(&p.path)) {
            match user.move_to_group(p, group) {
                Ok(path) => {
                    moved += 1;
                    let id = path.to_string_lossy().to_string();
                    if self.presets.favorites.remove(&p.id) {
                        self.presets.favorites.insert(id.clone());
                    }
                    if layout.hidden.remove(&p.id) {
                        layout.hidden.insert(id.clone());
                    }
                    defaults.rename_preset(&p.id, &id, &p.name);
                }
                Err(e) => failed.push(format!("{e:#}")),
            }
        }
        // A renamed group keeps its place, unless it joined one already there.
        if let Some(from) = from {
            let to = PresetLayout::group_key(true, group.trim());
            if let Some(at) = layout.order.iter().position(|k| *k == from)
                && failed.is_empty()
            {
                if layout.order.contains(&to) {
                    layout.order.remove(at);
                } else {
                    layout.order[at] = to;
                }
            }
        }
        let _ = crate::presets::save_favorites(&self.presets.favorites);
        self.set_preset_layout(layout);
        if let Err(e) = self.set_raw_defaults(defaults) {
            failed.push(format!("raw defaults not saved: {e:#}"));
        }
        let what = plural(moved, "preset", "presets");
        if failed.is_empty() {
            self.messages
                .done(format!("{what} moved to {}", group.trim()));
        } else {
            self.messages
                .fail(format!("{what} moved; not moved: {}", failed.join("; ")));
        }
        self.reload_presets(ctx);
    }
    /// Asks where to save the presets at `paths`, then copies them there: one as its
    /// .xmp, a group as a .zip of them, as Lightroom's Export Group does.
    fn export_presets(
        &mut self,
        paths: Vec<PathBuf>,
        name: String,
        zip: bool,
        ctx: &egui::Context,
    ) {
        if !self.activity.begin_dialog() {
            return;
        }
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let (kind, extension) = if zip {
                    ("Zip archive", "zip")
                } else {
                    ("Lightroom preset", "xmp")
                };
                let event = match rfd::FileDialog::new()
                    .set_file_name(&name)
                    .add_filter(kind, &[extension])
                    .save_file()
                {
                    None => Event::DialogClosed,
                    Some(to) => Event::PresetsExported(
                        write_export(&paths, &to, zip)
                            .map(|()| {
                                format!(
                                    "{} exported to {}",
                                    plural(paths.len(), "preset", "presets"),
                                    to.display()
                                )
                            })
                            .map_err(|e| format!("Presets not exported: {e:#}")),
                    ),
                };
                let _ = tx.send(event);
            },
            |tx, _| {
                let _ = tx.send(Event::DialogClosed);
            },
        );
    }
    /// Deletes the presets at `paths` that may be deleted here, and forgets them as
    /// favorites and as hidden.
    fn delete_presets(&mut self, paths: &[PathBuf], ctx: &egui::Context) {
        let library = self.presets.library.clone();
        let user = UserPresets::default();
        let mut layout = self.presets.layout.clone();
        let (mut deleted, mut failed) = (0, Vec::new());
        for p in library.presets.iter().filter(|p| paths.contains(&p.path)) {
            match user.delete(p) {
                Ok(()) => {
                    deleted += 1;
                    self.presets.favorites.remove(&p.id);
                    layout.hidden.remove(&p.id);
                }
                Err(e) => failed.push(format!("{}: {e:#}", p.name)),
            }
        }
        let _ = crate::presets::save_favorites(&self.presets.favorites);
        self.set_preset_layout(layout);
        if failed.is_empty() {
            self.messages
                .done(format!("{} deleted", plural(deleted, "preset", "presets")));
        } else {
            self.messages.fail(format!(
                "{} deleted; not deleted: {}",
                plural(deleted, "preset", "presets"),
                failed.join("; ")
            ));
        }
        self.reload_presets(ctx);
    }
}

/// Writes the presets at `paths` to `to`: a copy of the one, or a .zip of them all,
/// each by its file name (numbered when two share one).
fn write_export(paths: &[PathBuf], to: &std::path::Path, zip: bool) -> anyhow::Result<()> {
    use std::io::Write;
    if !zip {
        let from = paths.first().context("No preset to export")?;
        std::fs::copy(from, to)?;
        return Ok(());
    }
    let mut archive = zip::ZipWriter::new(std::fs::File::create(to)?);
    let mut names = BTreeSet::new();
    for path in paths {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let mut name = format!("{stem}.xmp");
        let mut n = 2;
        while !names.insert(name.clone()) {
            name = format!("{stem} {n}.xmp");
            n += 1;
        }
        archive.start_file(name, zip::write::SimpleFileOptions::default())?;
        archive.write_all(&std::fs::read(path)?)?;
    }
    archive.finish()?;
    Ok(())
}

/// Every group, draggable, with its presets under it when open; returns what was
/// done.
fn group_list(
    ui: &mut egui::Ui,
    groups: &[PresetGroup],
    library: &crate::presets::Library,
    layout: &PresetLayout,
    user: &UserPresets,
    manager: &mut PresetManager,
) -> Option<Change> {
    let palette = theme::palette(ui.ctx());
    let mut change = None;
    let mut drop_line: Option<(Rect, f32)> = None;
    // Only a press that has moved drags; a click leaves rows as they are.
    let dragging = ui.input(|i| i.pointer.is_decidedly_dragging());
    let paths_of = |group: &PresetGroup| -> Vec<PathBuf> {
        group
            .presets
            .iter()
            .map(|(i, _)| library.presets[*i].path.clone())
            .collect()
    };
    for (gi, group) in groups.iter().enumerate() {
        let key = group.key();
        let ids: Vec<String> = group
            .presets
            .iter()
            .map(|(i, _)| library.presets[*i].id.clone())
            .collect();
        let hidden = ids.iter().filter(|id| layout.hidden.contains(*id)).count();
        let all_hidden = hidden == ids.len();
        // The user's own: made here or imported, so renamed, moved and deleted here.
        let own = group
            .presets
            .iter()
            .all(|(i, _)| user.manages(&library.presets[*i]));
        let open = manager.open.contains(&key);
        let renaming = manager.rename.as_ref().is_some_and(|(k, _)| *k == key);
        let drag_id = ui.id().with(("preset-group", &key));
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
        let row = ui
            .interact(rect, drag_id, Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        row.dnd_set_drag_payload(gi);
        let dragged = dragging && ui.ctx().is_being_dragged(drag_id);
        // A preset dragged over one of the user's groups it is not in yet.
        let target = own
            && group.imported
            && dragging
            && row
                .dnd_hover_payload::<PresetDrag>()
                .is_some_and(|d| !group.presets.iter().any(|(i, _)| *i == d.0));
        let count = if hidden == 0 {
            group.presets.len().to_string()
        } else {
            format!(
                "{} of {}",
                group.presets.len() - hidden,
                group.presets.len()
            )
        };
        paint_row(
            ui,
            ui.painter(),
            rect,
            &RowLook {
                name: (!renaming).then_some(group.name.as_str()),
                count: Some(&count),
                open: Some(open),
                grip: true,
                dim: all_hidden,
                marked: dragged || target,
            },
        );
        if dragged {
            ghost(ui, drag_id, rect, &group.name, Some(&count));
        }
        if target && let Some(d) = row.dnd_release_payload::<PresetDrag>() {
            change = Some(Change::MoveTo {
                paths: vec![library.presets[d.0].path.clone()],
                group: group.name.clone(),
                from: None,
            });
        }
        // Dropped on this row: before it in its upper half, after it in its lower.
        if dragging && let Some(from) = row.dnd_hover_payload::<usize>() {
            let below = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| p.y > rect.center().y);
            let to = gi + usize::from(below);
            // Its own place: nothing to show or do.
            if to != *from && to != *from + 1 {
                drop_line = Some((rect, if below { rect.bottom() } else { rect.top() }));
                if row.dnd_release_payload::<usize>().is_some() {
                    change = Some(Change::Move { from: *from, to });
                }
            }
        }
        if renaming {
            if let Some(done) = rename_field(ui, rect, 40., manager, &group.name) {
                change = Some(match done {
                    Some(name) => Change::MoveTo {
                        paths: paths_of(group),
                        group: name,
                        from: Some(key.clone()),
                    },
                    None => Change::CancelRename,
                });
            }
        } else if own && row.double_clicked() {
            change = Some(Change::StartRename(key.clone(), group.name.clone()));
        } else if row.clicked() {
            change = Some(Change::Toggle(key.clone()));
        }
        let show = RowButtons {
            hidden: all_hidden,
            rename: own.then_some("Rename Group…"),
            export: (!group
                .presets
                .iter()
                .any(|(i, _)| library.presets[*i].builtin))
            .then_some("Export Group…"),
            delete: own,
        };
        if let Some(action) = row_actions(ui, rect, &row, ("group", &key), &show) {
            change = Some(match action {
                RowAction::Hide => Change::Hide {
                    ids,
                    hide: !all_hidden,
                },
                RowAction::Delete => Change::Delete(Removal {
                    title: format!("Delete the group \u{201c}{}\u{201d}?", group.name),
                    paths: paths_of(group),
                }),
                RowAction::Rename => Change::StartRename(key.clone(), group.name.clone()),
                RowAction::Export => Change::Export {
                    paths: paths_of(group),
                    name: format!("{}.zip", crate::presets::user::file_name(&group.name)),
                    zip: true,
                },
            });
        }
        if !open {
            continue;
        }
        for (i, name) in &group.presets {
            let p = &library.presets[*i];
            let is_hidden = layout.hidden.contains(&p.id);
            let own = user.manages(p);
            let key = format!("preset:{}", p.id);
            let renaming = manager.rename.as_ref().is_some_and(|(k, _)| *k == key);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW - 2.), Sense::hover());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let drag_id = ui.id().with(("preset-drag", &p.id));
            let sense = if own {
                Sense::click_and_drag()
            } else {
                Sense::click()
            };
            let mut row = ui.interact(rect, drag_id, sense);
            if own {
                row = row
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag onto one of your groups to move it there");
                row.dnd_set_drag_payload(PresetDrag(*i));
            }
            let dragged = own && dragging && ui.ctx().is_being_dragged(drag_id);
            paint_row(
                ui,
                ui.painter(),
                rect,
                &RowLook {
                    name: (!renaming).then_some(name.as_str()),
                    count: None,
                    open: None,
                    grip: own,
                    dim: is_hidden,
                    marked: dragged,
                },
            );
            if dragged {
                ghost(ui, drag_id, rect, name, None);
            }
            if renaming {
                if let Some(done) = rename_field(ui, rect, 46., manager, name) {
                    change = Some(match done {
                        Some(name) => Change::RenamePreset {
                            path: p.path.clone(),
                            name,
                        },
                        None => Change::CancelRename,
                    });
                }
            } else if own && row.double_clicked() {
                change = Some(Change::StartRename(key.clone(), name.clone()));
            }
            let show = RowButtons {
                hidden: is_hidden,
                rename: own.then_some("Rename…"),
                export: (!p.builtin).then_some("Export…"),
                delete: own,
            };
            match row_actions(ui, rect, &row, ("preset", &p.id), &show) {
                Some(RowAction::Hide) => {
                    change = Some(Change::Hide {
                        ids: vec![p.id.clone()],
                        hide: !is_hidden,
                    });
                }
                Some(RowAction::Delete) => {
                    change = Some(Change::Delete(Removal {
                        title: format!("Delete the preset \u{201c}{name}\u{201d}?"),
                        paths: vec![p.path.clone()],
                    }));
                }
                Some(RowAction::Rename) => {
                    change = Some(Change::StartRename(key, name.clone()));
                }
                Some(RowAction::Export) => {
                    change = Some(Change::Export {
                        paths: vec![p.path.clone()],
                        name: p
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        zip: false,
                    });
                }
                None => {}
            }
        }
    }
    if let Some((rect, y)) = drop_line {
        ui.painter().hline(
            rect.left() + 6.0..=rect.right() - 6.0,
            y,
            egui::Stroke::new(2., palette.tone(Tone::STRONG)),
        );
    }
    change
}

/// The name being typed in place of a row's, from `left`: Some(Some(name)) once
/// Return or a click elsewhere ends it with a new name, Some(None) for Escape or an
/// unchanged or empty name, None while typing.
fn rename_field(
    ui: &mut egui::Ui,
    rect: Rect,
    left: f32,
    manager: &mut PresetManager,
    current: &str,
) -> Option<Option<String>> {
    let (_, typed) = manager.rename.as_mut()?;
    let field = ui.put(
        Rect::from_min_max(
            Pos2::new(rect.left() + left - 2., rect.top() + 3.),
            Pos2::new(rect.right() - 110., rect.bottom() - 3.),
        ),
        egui::TextEdit::singleline(typed).font(egui::FontId::proportional(text::LABEL)),
    );
    if !field.has_focus() && !field.lost_focus() {
        field.request_focus();
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Some(None);
    }
    if !field.lost_focus() {
        return None;
    }
    let name = typed.trim();
    Some((!name.is_empty() && name != current).then(|| name.to_string()))
}

/// Two columns of three dots: a row that can be dragged.
fn grip(painter: &egui::Painter, center: Pos2, palette: &theme::Palette) {
    for col in 0..2 {
        for row in 0..3 {
            painter.circle_filled(
                Pos2::new(
                    center.x - 2. + col as f32 * 4.,
                    center.y - 4. + row as f32 * 4.,
                ),
                1.2,
                palette.tone(Tone::FAINT),
            );
        }
    }
}

/// What a row shows: a group's chevron (`open`) and count, a preset's name under
/// it; `name` is None while it is being typed instead.
struct RowLook<'a> {
    name: Option<&'a str>,
    count: Option<&'a str>,
    open: Option<bool>,
    grip: bool,
    dim: bool,
    /// Dragged, or the group a dragged preset would go to.
    marked: bool,
}

/// Paints a group's or a preset's row with `painter`, in the list or as the
/// dragged ghost.
fn paint_row(ui: &egui::Ui, painter: &egui::Painter, rect: Rect, look: &RowLook) {
    let palette = theme::palette(ui.ctx());
    if look.marked {
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    } else if ui.rect_contains_pointer(rect) {
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::CARD));
    }
    let y = rect.center().y;
    // A preset's grip and name sit under its group's name.
    let indent = if look.open.is_some() { 0. } else { 22. };
    if look.grip {
        grip(painter, Pos2::new(rect.left() + 12. + indent, y), &palette);
    }
    if let Some(open) = look.open {
        icons::paint_at(
            painter,
            if open {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            Pos2::new(rect.left() + 30., y),
            11.,
            palette.tone(Tone::MUTED),
        );
    }
    if let Some(name) = look.name {
        let galley = egui::WidgetText::from(name).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (rect.width() - 160.).max(1.),
            egui::FontId::proportional(text::LABEL),
        );
        let tone = match (look.dim, look.open.is_some()) {
            (true, _) => Tone::DISABLED,
            (false, true) => Tone::STRONG,
            (false, false) => Tone::SOFT,
        };
        painter.galley(
            Pos2::new(rect.left() + 40. + indent / 3., y - galley.size().y / 2.),
            galley,
            palette.tone(tone),
        );
    }
    if let Some(count) = look.count {
        painter.text(
            Pos2::new(rect.right() - 72., y),
            egui::Align2::RIGHT_CENTER,
            count,
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::FAINT),
        );
    }
}

/// The dragged row's ghost, following the pointer up and down over the list.
fn ghost(ui: &egui::Ui, id: egui::Id, rect: Rect, name: &str, count: Option<&str>) {
    let Some(dy) = ui.input(|i| {
        i.pointer
            .interact_pos()
            .zip(i.pointer.press_origin())
            .map(|(at, from)| at.y - from.y)
    }) else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Tooltip, id));
    let rect = rect.translate(Vec2::new(0., dy));
    painter.rect(
        rect,
        radius::CONTROL,
        palette.tone(Tone::RAISED),
        egui::Stroke::new(1., palette.tone(Tone::OUTLINE)),
        egui::StrokeKind::Inside,
    );
    paint_row(
        ui,
        &painter,
        rect,
        &RowLook {
            name: Some(name),
            count,
            open: count.map(|_| false),
            grip: true,
            dim: false,
            marked: true,
        },
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
}

enum RowAction {
    Rename,
    Export,
    Hide,
    Delete,
}

/// What a row offers: the eye always, and the menu's items, each named, when they
/// apply.
struct RowButtons {
    hidden: bool,
    rename: Option<&'static str>,
    export: Option<&'static str>,
    delete: bool,
}

/// The row's eye at its right end, and beside it the menu of what else applies,
/// which a right-click on the row opens too.
fn row_actions(
    ui: &mut egui::Ui,
    rect: Rect,
    row: &egui::Response,
    id: (&str, &String),
    show: &RowButtons,
) -> Option<RowAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    let button = |ui: &mut egui::Ui, x: f32, icon: Icon, tip: &str| {
        let r = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::splat(22.));
        let response = ui
            .interact(r, ui.id().with((id, tip)), Sense::click())
            .on_hover_text(tip);
        if response.hovered() {
            ui.painter()
                .rect_filled(r, radius::CONTROL, palette.tone(Tone::RAISED));
        }
        let tint = palette.tone(if response.hovered() {
            Tone::STRONG
        } else {
            Tone::MUTED
        });
        icons::paint_at(ui.painter(), icon, r.center(), 13., tint);
        response
    };
    let (eye, tip) = if show.hidden {
        (Icon::EyeOff, "Show in the Presets panel")
    } else {
        (Icon::Eye, "Hide from the Presets panel")
    };
    if button(ui, rect.right() - 44., eye, tip).clicked() {
        action = Some(RowAction::Hide);
    }
    if show.rename.is_none() && show.export.is_none() && !show.delete {
        return action;
    }
    let items = |ui: &mut egui::Ui, action: &mut Option<RowAction>| {
        ui.set_min_width(170.);
        let mut item = |ui: &mut egui::Ui, label: &str, chosen: RowAction| {
            if ui.add(egui::Button::new(label).frame(false)).clicked() {
                *action = Some(chosen);
                ui.close();
            }
        };
        if let Some(label) = show.rename {
            item(ui, label, RowAction::Rename);
        }
        if let Some(label) = show.export {
            item(ui, label, RowAction::Export);
        }
        if show.delete {
            ui.separator();
            item(ui, "Delete…", RowAction::Delete);
        }
    };
    let more = button(ui, rect.right() - 16., Icon::More, "More");
    egui::Popup::menu(&more).show(|ui| items(ui, &mut action));
    crate::app::ui::widgets::context_menu(row, |ui| items(ui, &mut action));
    action
}

#[cfg(test)]
mod tests {
    use crate::presets::PresetLayout;

    #[test]
    fn a_group_exports_as_a_zip_of_its_files() -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let a = d.path().join("a").join("Warm.xmp");
        let b = d.path().join("b").join("Warm.xmp");
        for (p, text) in [(&a, "one"), (&b, "two")] {
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(p, text)?;
        }
        let to = d.path().join("Group.zip");
        super::write_export(&[a.clone(), b], &to, true)?;
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&to)?)?;
        let mut names: Vec<_> = archive
            .file_names()
            .map(|n| n.map(|n| n.into_owned()))
            .collect::<Result<_, _>>()?;
        names.sort();
        assert_eq!(names, ["Warm 2.xmp", "Warm.xmp"]);
        assert_eq!(
            std::io::read_to_string(archive.by_name("Warm 2.xmp")?)?,
            "two"
        );
        let one = d.path().join("one.xmp");
        super::write_export(&[a], &one, false)?;
        assert_eq!(std::fs::read_to_string(one)?, "one");
        Ok(())
    }

    #[test]
    fn groups_the_layout_does_not_name_go_after_the_ordered_ones() {
        let layout = PresetLayout {
            order: vec!["imported:B".into(), "builtin:Color".into()],
            ..Default::default()
        };
        assert_eq!(layout.rank("imported:B"), 0);
        assert_eq!(layout.rank("builtin:Color"), 1);
        assert_eq!(layout.rank("imported:New"), usize::MAX);
        assert_ne!(
            PresetLayout::group_key(true, "Color"),
            PresetLayout::group_key(false, "Color")
        );
    }
}
