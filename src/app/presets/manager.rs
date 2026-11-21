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
    /// The group being renamed in place: its key and the name being typed.
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
    StartRename(String, String),
    CancelRename,
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
                            "Drag groups to set their order in the Presets panel. Hidden \
                             presets stay on disk; presets made here or imported can be deleted.",
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
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Import Presets…").clicked() {
                        change = Some(Change::Import(FileDialog::ImportXmp));
                    }
                    if ui
                        .button("Import Folder…")
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
                            .add_enabled(!layout.order.is_empty(), egui::Button::new("Reset Order"))
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
        let drag_id = ui.id().with(("preset-group", &key));
        // The row stays in place while dragged, marked; the line shows where it goes.
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
        let row = ui
            .interact(rect, drag_id, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        row.dnd_set_drag_payload(gi);
        let dragged = ui.ctx().is_being_dragged(drag_id);
        // A preset dragged over one of the user's groups it is not in yet.
        let target = own
            && group.imported
            && row
                .dnd_hover_payload::<PresetDrag>()
                .is_some_and(|d| !group.presets.iter().any(|(i, _)| *i == d.0));
        let renaming = manager.rename.as_ref().is_some_and(|(k, _)| *k == key);
        paint_group(ui, rect, group, hidden, open, dragged || target, !renaming);
        if target && let Some(d) = row.dnd_release_payload::<PresetDrag>() {
            change = Some(Change::MoveTo {
                paths: vec![library.presets[d.0].path.clone()],
                group: group.name.clone(),
                from: None,
            });
        }
        // Dropped on this row: before it in its upper half, after it in its lower.
        if let Some(from) = row.dnd_hover_payload::<usize>() {
            let below = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| p.y > rect.center().y);
            drop_line = Some((rect, if below { rect.bottom() } else { rect.top() }));
            if row.dnd_release_payload::<usize>().is_some() {
                change = Some(Change::Move {
                    from: *from,
                    to: gi + usize::from(below),
                });
            }
        }
        let name_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 38., rect.top() + 3.),
            Pos2::new(rect.right() - 110., rect.bottom() - 3.),
        );
        if renaming && let Some((_, typed)) = manager.rename.as_mut() {
            let field = ui.put(
                name_rect,
                egui::TextEdit::singleline(typed).font(egui::FontId::proportional(text::LABEL)),
            );
            if !field.has_focus() && !field.lost_focus() {
                field.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                change = Some(Change::CancelRename);
            } else if field.lost_focus() {
                let name = typed.trim().to_string();
                change = Some(if name.is_empty() || name == group.name {
                    Change::CancelRename
                } else {
                    Change::MoveTo {
                        paths: paths_of(group),
                        group: name,
                        from: Some(key.clone()),
                    }
                });
            }
        }
        let actions = row_actions(
            ui,
            rect,
            ("group", &key),
            RowButtons {
                hidden: all_hidden,
                own,
                rename: own,
                export: !group
                    .presets
                    .iter()
                    .any(|(i, _)| library.presets[*i].builtin),
            },
        );
        if let Some(action) = actions {
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
        if !renaming {
            let toggle = Rect::from_min_max(rect.min, Pos2::new(rect.right() - 60., rect.bottom()));
            let clicks = ui.interact(toggle, ui.id().with(("toggle", &key)), Sense::click());
            if own && clicks.double_clicked() {
                change = Some(Change::StartRename(key.clone(), group.name.clone()));
            } else if clicks.clicked() {
                change = Some(Change::Toggle(key.clone()));
            }
        }
        if !open {
            continue;
        }
        for (i, name) in &group.presets {
            let p = &library.presets[*i];
            let is_hidden = layout.hidden.contains(&p.id);
            let own = user.manages(p);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW - 4.), Sense::hover());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            if own {
                let drag_id = ui.id().with(("preset-drag", &p.id));
                let row = ui
                    .interact(rect, drag_id, Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag onto one of your groups to move it there");
                row.dnd_set_drag_payload(PresetDrag(*i));
                if ui.ctx().is_being_dragged(drag_id) {
                    ui.painter()
                        .rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
                }
                grip(
                    ui.painter(),
                    Pos2::new(rect.left() + 32., rect.center().y),
                    &palette,
                );
            }
            let galley = egui::WidgetText::from(name.as_str()).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (rect.width() - 110.).max(1.),
                egui::FontId::proportional(text::LABEL),
            );
            ui.painter().galley(
                Pos2::new(rect.left() + 46., rect.center().y - galley.size().y / 2.),
                galley,
                palette.tone(if is_hidden {
                    Tone::DISABLED
                } else {
                    Tone::SOFT
                }),
            );
            let buttons = RowButtons {
                hidden: is_hidden,
                own,
                rename: false,
                export: !p.builtin,
            };
            match row_actions(ui, rect, ("preset", &p.id), buttons) {
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
                Some(RowAction::Rename) | None => {}
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

/// A group's row: the grip, its chevron, name (unless it is being typed), and how
/// many of its presets show. `marked` while it is dragged or a preset is dropped on it.
fn paint_group(
    ui: &egui::Ui,
    rect: Rect,
    group: &PresetGroup,
    hidden: usize,
    open: bool,
    marked: bool,
    show_name: bool,
) {
    let all_hidden = hidden == group.presets.len();
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    if marked {
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::RAISED));
    } else if ui.rect_contains_pointer(rect) {
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::CARD));
    }
    let y = rect.center().y;
    grip(painter, Pos2::new(rect.left() + 12., y), &palette);
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
    let total = group.presets.len();
    let count = if hidden == 0 {
        total.to_string()
    } else {
        format!("{} of {total}", total - hidden)
    };
    if show_name {
        let galley = egui::WidgetText::from(group.name.as_str()).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (rect.width() - 160.).max(1.),
            egui::FontId::proportional(text::LABEL),
        );
        painter.galley(
            Pos2::new(rect.left() + 40., y - galley.size().y / 2.),
            galley,
            palette.tone(if all_hidden {
                Tone::DISABLED
            } else {
                Tone::STRONG
            }),
        );
    }
    painter.text(
        Pos2::new(rect.right() - 72., y),
        egui::Align2::RIGHT_CENTER,
        count,
        egui::FontId::proportional(text::SMALL),
        palette.tone(Tone::FAINT),
    );
}

enum RowAction {
    Rename,
    Export,
    Hide,
    Delete,
}

/// What a row offers: the eye always, the menu's items when they apply.
struct RowButtons {
    hidden: bool,
    own: bool,
    rename: bool,
    export: bool,
}

/// The row's eye at its right end, and beside it the menu of what else applies:
/// rename, export and delete.
fn row_actions(
    ui: &mut egui::Ui,
    rect: Rect,
    id: (&str, &String),
    show: RowButtons,
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
    if !(show.rename || show.export || show.own) {
        return action;
    }
    let more = button(ui, rect.right() - 16., Icon::More, "More");
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(170.);
        let mut item = |ui: &mut egui::Ui, label: &str, chosen: RowAction| {
            if ui.add(egui::Button::new(label).frame(false)).clicked() {
                action = Some(chosen);
                ui.close();
            }
        };
        if show.rename {
            item(ui, "Rename Group…", RowAction::Rename);
        }
        if show.export {
            item(
                ui,
                if show.rename {
                    "Export Group…"
                } else {
                    "Export…"
                },
                RowAction::Export,
            );
        }
        if show.own {
            ui.separator();
            item(ui, "Delete…", RowAction::Delete);
        }
    });
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
