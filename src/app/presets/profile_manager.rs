//! Manage Profiles: the profile library's files in one window, by the Profile
//! menu's groups. Looks can be renamed and moved to another group, any file moved
//! to the Trash, groups hidden from the Profile menu and profiles made favourites,
//! as in Lightroom's profile browser. The library's own rules (which names a look
//! may take, what a rename keeps) are `camera_profiles::manage`'s; this window
//! only asks for them and shows the result.
//!
//! Changes to files run off the UI thread, one at a time, and never while an
//! import or a community profile download writes to the library (nor does an
//! import start while one runs). Each ends by listing the library again, and a
//! change lists the open photo's profiles again as an import does.
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{self, modal_frame, plural, primary_button, spinner};
use crate::app::{Editor, ui::theme, worker::Event};
use crate::camera_profiles::manage::{self, Entry, Kind, Library};
use crate::catalog::file_ops::Trash;
use eframe::egui::{self, Color32};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// The Profile menu's view of the library that is kept in the session, not in the
/// files: the groups it hides and the profiles it lists first, by
/// [`manage::key`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuChoices {
    pub hidden_groups: BTreeSet<String>,
    pub favorites: BTreeSet<String>,
}
impl MenuChoices {
    /// Whether the profile named `name`, with the look UUID `uuid`, is a favourite.
    pub(crate) fn favorite(&self, name: &str, uuid: Option<&str>) -> bool {
        !self.favorites.is_empty() && self.favorites.contains(&manage::key(name, uuid))
    }
}

/// A change asked of the library.
#[derive(Clone, Debug)]
enum Op {
    /// Only list it.
    List,
    Rename {
        file: PathBuf,
        name: String,
    },
    /// Move these looks to `group`; `renaming` is the group they all came from when
    /// a whole group is renamed, whose hidden state the new name takes.
    Move {
        files: Vec<PathBuf>,
        group: String,
        renaming: Option<String>,
    },
    Trash(Vec<PathBuf>),
}

/// What a job did, for [`Editor::profiles_managed`].
pub(crate) struct Managed {
    generation: u64,
    entries: Vec<Entry>,
    /// What to say, or why it failed.
    said: Result<String, String>,
    /// Whether any file changed.
    changed: bool,
    /// A whole group renamed: its old name and its new.
    renamed_group: Option<(String, String)>,
}

/// What the window asks before a change.
#[derive(Clone, Debug, PartialEq)]
enum Prompt {
    Rename {
        file: PathBuf,
        name: String,
    },
    Move {
        files: Vec<PathBuf>,
        group: String,
    },
    RenameGroup {
        old: String,
        name: String,
    },
    Trash {
        files: Vec<PathBuf>,
        /// The looks the removal leaves without their base profile.
        dependents: Vec<String>,
    },
}

/// The change running on the library, if any. Kept by the editor rather than the
/// window, so closing the window while a change runs keeps imports and downloads
/// waiting for it, and a window opened again doesn't start a second one.
#[derive(Debug, Default)]
pub(crate) struct Jobs {
    generation: u64,
    running: Option<u64>,
}

/// The window's state.
pub(crate) struct ProfileManager {
    library: Library,
    trash: Arc<dyn Trash + Send + Sync>,
    /// The library as last listed; None until the first listing.
    entries: Option<Vec<Entry>>,
    selected: BTreeSet<PathBuf>,
    filter: String,
    prompt: Option<Prompt>,
    /// Why the last change failed, said in the window.
    error: Option<String>,
}
impl ProfileManager {
    fn new(library: Library, trash: Arc<dyn Trash + Send + Sync>) -> Self {
        Self {
            library,
            trash,
            entries: None,
            selected: BTreeSet::new(),
            filter: String::new(),
            prompt: None,
            error: None,
        }
    }
    fn entry(&self, file: &PathBuf) -> Option<&Entry> {
        self.entries.as_ref()?.iter().find(|e| e.file == *file)
    }
    fn selected_entries(&self) -> Vec<&Entry> {
        self.selected.iter().filter_map(|f| self.entry(f)).collect()
    }
}

/// Runs `op` on `library`, then lists it.
fn run(library: &Library, trash: &dyn Trash, op: Op) -> (Result<String, String>, bool) {
    match op {
        Op::List => (Ok(String::new()), false),
        Op::Rename { file, name } => match library.rename_look(&file, &name) {
            Ok(()) => (Ok(format!("Renamed to {}", name.trim())), true),
            Err(e) => (Err(format!("{e:#}")), false),
        },
        Op::Move { files, group, .. } => match library.set_group(&files, &group) {
            Ok(n) => {
                let group = group.trim();
                let to = if group.is_empty() {
                    "out of their group".to_string()
                } else {
                    format!("to {group}")
                };
                (
                    Ok(format!("Moved {} {to}", plural(n, "look", "looks"))),
                    n > 0,
                )
            }
            Err(e) => {
                // Looks moved before a failed write are moved: list the profiles again.
                let partial = e
                    .downcast_ref::<manage::Partial>()
                    .is_some_and(|p| p.done > 0);
                (Err(format!("{e:#}")), partial)
            }
        },
        Op::Trash(files) => {
            let removal = library.remove(&files, |file| {
                trash
                    .trash(file)
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e}"))
            });
            let done = format!(
                "Moved {} to the Trash",
                plural(removal.removed, "profile", "profiles")
            );
            let said = match removal.failed.first() {
                None => Ok(done),
                Some((file, why)) => Err(format!(
                    "{done}; {} not: {}: {why}",
                    plural(removal.failed.len(), "file", "files"),
                    file.file_name().unwrap_or_default().to_string_lossy()
                )),
            };
            (said, removal.removed > 0)
        }
    }
}

impl Editor {
    /// Opens Manage Profiles on the user's library.
    pub(in crate::app) fn open_profile_manager(&mut self, ctx: &egui::Context) {
        self.open_profile_manager_on(
            Library::user(),
            Arc::new(crate::platform::trash::SystemTrash),
            ctx,
        );
    }
    /// [`Self::open_profile_manager`] on `library`, moving files to `trash`.
    pub(in crate::app) fn open_profile_manager_on(
        &mut self,
        library: Library,
        trash: Arc<dyn Trash + Send + Sync>,
        ctx: &egui::Context,
    ) {
        if self.profile_manager.is_none() {
            self.profile_manager = Some(ProfileManager::new(library, trash));
        }
        self.manage_profiles(Op::List, ctx);
    }
    pub(in crate::app) fn profile_manager_open(&self) -> bool {
        self.profile_manager.is_some()
    }
    /// Whether a change to the library is running.
    pub(in crate::app) fn managing_profiles(&self) -> bool {
        self.profile_jobs.running.is_some()
    }
    /// Starts `op` in the background, unless another change, an import or a
    /// download is writing to the library; the window says why not. A listing asked
    /// while a change runs is left to that change's, which lists the library too.
    fn manage_profiles(&mut self, op: Op, ctx: &egui::Context) {
        let blocked = if self.importing.is_some() {
            Some("An import is writing to the profile library. Try again when it finishes.".into())
        } else {
            self.profile_download.downloading().map(|name| {
                format!(
                    "{name} is downloading into the profile library. Try again when it finishes."
                )
            })
        };
        let Some(manager) = self.profile_manager.as_mut() else {
            return;
        };
        if self.profile_jobs.running.is_some() {
            return;
        }
        if let Some(why) = blocked.filter(|_| !matches!(op, Op::List)) {
            manager.error = Some(why);
            return;
        }
        self.profile_jobs.generation += 1;
        let generation = self.profile_jobs.generation;
        self.profile_jobs.running = Some(generation);
        manager.error = None;
        let (library, trash) = (manager.library.clone(), manager.trash.clone());
        let renaming = match &op {
            Op::Move {
                group, renaming, ..
            } => renaming.clone().map(|old| (old, group.trim().to_string())),
            _ => None,
        };
        let fallback = library.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let (said, changed) = run(&library, &*trash, op);
                let renamed_group = renaming.filter(|_| said.is_ok() && changed);
                let _ = tx.send(Event::ProfilesManaged(Box::new(Managed {
                    generation,
                    entries: library.entries(),
                    said,
                    changed,
                    renamed_group,
                })));
            },
            move |tx, error| {
                // A change may have been made before the panic: list again. This
                // runs outside the task's own panic guard, so a listing that
                // panics again reports none: the busy state must still clear.
                let entries =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fallback.entries()))
                        .unwrap_or_default();
                let _ = tx.send(Event::ProfilesManaged(Box::new(Managed {
                    generation,
                    entries,
                    said: Err(error),
                    changed: true,
                    renamed_group: None,
                })));
            },
        );
    }
    /// A job's result: the listing, what to say, and the open photo's profiles
    /// listed again after a change.
    pub(in crate::app) fn profiles_managed(&mut self, done: Managed) {
        if self.profile_jobs.running != Some(done.generation) {
            return;
        }
        self.profile_jobs.running = None;
        // A window opened again since the change began shows its result too.
        if let Some(manager) = self.profile_manager.as_mut() {
            manager
                .selected
                .retain(|f| done.entries.iter().any(|e| e.file == *f));
            manager.entries = Some(done.entries);
            match &done.said {
                Ok(_) => manager.prompt = None,
                Err(why) => manager.error = Some(why.clone()),
            }
        }
        if let Some((old, new)) = done.renamed_group
            && self.profile_menu.hidden_groups.remove(&old)
        {
            self.profile_menu.hidden_groups.insert(new);
            let _ = self.save_session();
        }
        if done.changed {
            self.list_profiles_after_import();
        }
        match done.said {
            Ok(said) if !said.is_empty() => self.messages.done(said),
            Ok(_) => {}
            Err(why) => self.messages.fail(why),
        }
    }
    fn toggle_hidden(&mut self, group: &str) {
        let hidden = &mut self.profile_menu.hidden_groups;
        if !hidden.remove(group) {
            hidden.insert(group.to_string());
        }
        let _ = self.save_session();
    }
    fn toggle_favorite(&mut self, key: String) {
        let favorites = &mut self.profile_menu.favorites;
        if !favorites.remove(&key) {
            favorites.insert(key);
        }
        let _ = self.save_session();
    }
    /// The window, while open.
    pub(in crate::app) fn profile_manager_window(&mut self, ctx: &egui::Context) {
        let busy = self.managing_profiles();
        let Some(manager) = self.profile_manager.as_mut() else {
            return;
        };
        let palette = theme::palette(ctx);
        let choices = self.profile_menu.clone();
        let mut asked: Vec<Asked> = Vec::new();
        let response = egui::Modal::new(egui::Id::new("manage-profiles"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                window(ui, manager, busy, &choices, &palette, &mut asked);
            });
        if response.should_close() && manager.prompt.is_none() {
            asked.push(Asked::Close);
        }
        for asked in asked {
            match asked {
                Asked::Close => self.profile_manager = None,
                Asked::Op(op) => self.manage_profiles(op, ctx),
                Asked::Hide(group) => self.toggle_hidden(&group),
                Asked::Favorite(key) => self.toggle_favorite(key),
                Asked::ShowAll => {
                    self.profile_menu.hidden_groups.clear();
                    let _ = self.save_session();
                }
            }
        }
    }
}

/// What the window asked for this frame.
enum Asked {
    Close,
    Op(Op),
    Hide(String),
    Favorite(String),
    ShowAll,
}

/// The entries by group: the top level (no group) first, then groups by name,
/// ignoring case. Camera profiles and unreadable files are at the top level, as in
/// the Profile menu.
fn by_group<'a>(entries: &'a [Entry], filter: &str) -> Vec<(String, Vec<&'a Entry>)> {
    let filter = filter.trim().to_lowercase();
    let mut groups: Vec<(String, Vec<&Entry>)> = Vec::new();
    for entry in entries {
        if !filter.is_empty()
            && !entry.name.to_lowercase().contains(&filter)
            && !entry.group.to_lowercase().contains(&filter)
        {
            continue;
        }
        let group = match entry.kind {
            Kind::Look if entry.group != "Profiles" => entry.group.clone(),
            _ => String::new(),
        };
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, items)) => items.push(entry),
            None => groups.push((group, vec![entry])),
        }
    }
    groups.sort_by_key(|(g, _)| (!g.is_empty(), g.to_lowercase(), g.clone()));
    groups
}

fn window(
    ui: &mut egui::Ui,
    manager: &mut ProfileManager,
    busy: bool,
    choices: &MenuChoices,
    palette: &theme::Palette,
    asked: &mut Vec<Asked>,
) {
    ui.set_width(640.);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Manage Profiles")
                .size(text::TITLE)
                .color(palette.tone(Tone::STRONG)),
        );
        if busy {
            spinner(ui, 14.);
        }
    });
    ui.add_space(space::S);
    ui.label(
        egui::RichText::new(
            "Rename looks, move them between the Profile menu's groups, hide groups \
             and choose favorites. Edits keep the profiles they use, whatever happens \
             to the files here.",
        )
        .size(text::SMALL)
        .color(palette.tone(Tone::SECONDARY)),
    );
    ui.add_space(space::M);
    // The filter and the buttons beside it share one height, so their edges line up.
    let height = widgets::DROPDOWN_HEIGHT;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut manager.filter)
                .hint_text("Filter by name or group")
                .desired_width(260.)
                .min_size(egui::vec2(0., height))
                .vertical_align(egui::Align::Center),
        );
        // The library's own actions sit at the far side, apart from the filter.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = |label: &str| egui::Button::new(label).min_size(egui::vec2(0., height));
            if ui.add(button(crate::platform::reveal::LABEL)).clicked() {
                let _ = crate::platform::reveal::reveal(manager.library.dir());
            }
            if !choices.hidden_groups.is_empty()
                && ui
                    .add(button("Show All Groups"))
                    .on_hover_text("Lists every hidden group in the Profile menu again")
                    .clicked()
            {
                asked.push(Asked::ShowAll);
            }
        });
    });
    ui.add_space(space::S);
    let idle = !busy;
    let mut rows = Vec::new();
    egui::ScrollArea::vertical()
        .max_height((ui.ctx().content_rect().height() - 260.).clamp(120., 420.))
        .auto_shrink([false, false])
        .show(ui, |ui| match &manager.entries {
            None => {
                ui.label("Reading the profile library…");
            }
            Some(entries) if entries.is_empty() => {
                ui.label("No profiles are imported yet.");
            }
            Some(entries) => {
                for (group, items) in by_group(entries, &manager.filter) {
                    let view = RowsView {
                        selected: &manager.selected,
                        idle,
                    };
                    group_rows(ui, &group, &items, view, choices, palette, asked, &mut rows);
                }
            }
        });
    for row in rows {
        match row {
            Row::Toggle(file) => {
                if !manager.selected.remove(&file) {
                    manager.selected.insert(file);
                }
            }
            Row::RenameGroup(old) => {
                manager.error = None;
                manager.prompt = Some(Prompt::RenameGroup {
                    name: old.clone(),
                    old,
                });
            }
        }
    }
    ui.add_space(space::M);
    if let Some(error) = &manager.error {
        ui.label(
            egui::RichText::new(error)
                .size(text::SMALL)
                .color(palette.tone(Tone::SECONDARY)),
        );
        ui.add_space(space::S);
    }
    if let Some(prompt) = manager.prompt.clone() {
        prompt_row(ui, manager, prompt, idle, asked);
    } else {
        actions(ui, manager, idle, asked);
    }
}

/// What the rows need of the manager, apart from what they change.
#[derive(Clone, Copy)]
struct RowsView<'a> {
    selected: &'a BTreeSet<PathBuf>,
    idle: bool,
}
/// What a row asked of the window itself.
enum Row {
    Toggle(PathBuf),
    RenameGroup(String),
}

#[allow(clippy::too_many_arguments)]
fn group_rows(
    ui: &mut egui::Ui,
    group: &str,
    items: &[&Entry],
    view: RowsView<'_>,
    choices: &MenuChoices,
    palette: &theme::Palette,
    asked: &mut Vec<Asked>,
    rows: &mut Vec<Row>,
) {
    // A hairline between groups, then the group's name with its controls at
    // the row's far end, on the same height as the profile rows below.
    ui.add_space(space::M);
    let (line, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), space::HAIR),
        egui::Sense::hover(),
    );
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        egui::Stroke::new(1., palette.tone(Tone::RAISED)),
    );
    ui.add_space(space::S);
    row(ui, |ui| {
        let shown = !choices.hidden_groups.contains(group);
        let title = if group.is_empty() { "Top level" } else { group };
        ui.label(
            egui::RichText::new(title)
                .size(text::LABEL)
                .strong()
                .color(palette.tone(if shown { Tone::STRONG } else { Tone::MUTED })),
        );
        if group.is_empty() {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::switch(ui, shown, &format!("Show {group} in menu"))
                .on_hover_text(
                    "Lists this group in the Profile menu; a hidden group's looks stay installed",
                )
                .clicked()
            {
                asked.push(Asked::Hide(group.to_string()));
            }
            ui.label(
                egui::RichText::new("Show in menu")
                    .size(text::SMALL)
                    .color(palette.tone(Tone::SOFT)),
            );
            ui.add_space(space::L);
            let editable = items.iter().any(|e| e.editable());
            if ui
                .add_enabled(
                    view.idle && editable,
                    egui::Button::new(egui::RichText::new("Rename…").size(text::SMALL)),
                )
                .on_hover_text("Renames this group for every look in it")
                .on_disabled_hover_text("Only groups of imported looks can be renamed")
                .clicked()
            {
                rows.push(Row::RenameGroup(group.to_string()));
            }
        });
    });
    for entry in items {
        row(ui, |ui| {
            let mut checked = view.selected.contains(&entry.file);
            if ui.checkbox(&mut checked, "").changed() {
                rows.push(Row::Toggle(entry.file.clone()));
            }
            // The star has a slot of its own, kept empty for a file that can't be
            // read, so every name starts at the same edge.
            let slot = egui::vec2(STAR, ROW);
            if entry.kind == Kind::Unreadable {
                ui.allocate_exact_size(slot, egui::Sense::hover());
            } else {
                let key = entry.key();
                let favorite = choices.favorites.contains(&key);
                let star = if favorite { "★" } else { "☆" };
                if ui
                    .add_sized(slot, egui::Button::new(star).frame(false))
                    .on_hover_text(if favorite {
                        "Remove from Favorites"
                    } else {
                        "Add to Favorites: listed first in the Profile menu"
                    })
                    .clicked()
                {
                    asked.push(Asked::Favorite(key));
                }
            }
            ui.label(&entry.name);
            let what = match entry.kind {
                Kind::Camera => format!("Camera profile · {}", entry.camera),
                Kind::Look if entry.adobe => "Adobe look".to_string(),
                Kind::Look => match (&entry.base, entry.camera.is_empty()) {
                    (Some(base), _) => format!("Look over {base}"),
                    (None, false) => format!("Look · {}", entry.camera),
                    (None, true) => "Look".to_string(),
                },
                Kind::Unreadable => "Can't be read".to_string(),
            };
            let note = ui.label(
                egui::RichText::new(what)
                    .size(text::SMALL)
                    .color(palette.tone(Tone::MUTED)),
            );
            let file = entry.file.file_name().unwrap_or_default().to_string_lossy();
            match &entry.error {
                Some(error) => note.on_hover_text(format!("{file}: {error}")),
                None => note.on_hover_text(file.into_owned()),
            };
        });
    }
}

/// One line of the list, as tall as a button, its contents on its middle.
fn row(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ROW),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            contents(ui);
            // The full height, whatever the contents' own: a row of a label
            // alone takes as much room as one with a button.
            ui.set_min_height(ROW);
        },
    );
}

/// The height of a line in the list.
const ROW: f32 = 28.;
/// The width of the favourite star's slot.
const STAR: f32 = 20.;

/// The buttons for the selection.
fn actions(ui: &mut egui::Ui, manager: &mut ProfileManager, idle: bool, asked: &mut Vec<Asked>) {
    let selected = manager.selected_entries();
    let editable = !selected.is_empty() && selected.iter().all(|e| e.editable());
    let one = (selected.len() == 1 && editable).then(|| selected[0].clone());
    let files: Vec<PathBuf> = selected.iter().map(|e| e.file.clone()).collect();
    let group = selected
        .first()
        .map(|e| e.group.clone())
        .unwrap_or_default();
    // Said above the buttons it explains, as their hover tips aren't seen
    // until a disabled button is pointed at.
    if !selected.is_empty() && !editable {
        let unreadable = selected.iter().all(|e| e.kind == Kind::Unreadable);
        ui.label(
            egui::RichText::new(if unreadable {
                "A file that can't be read can only be moved to the Trash."
            } else {
                "Camera profiles and Adobe's looks keep their names and groups, because \
                 edits, presets and Lightroom find them by name. Only imported looks and \
                 LUTs can be renamed or moved."
            })
            .size(text::SMALL)
            .color(theme::palette(ui.ctx()).tone(Tone::SECONDARY)),
        );
        ui.add_space(space::S);
    }
    let mut prompt = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = button::DIALOG;
        if ui
            .add_enabled(idle && one.is_some(), egui::Button::new("Rename…"))
            .on_disabled_hover_text(
                "Select one look. Camera profiles and Adobe's looks keep their names.",
            )
            .clicked()
            && let Some(entry) = &one
        {
            prompt = Some(Prompt::Rename {
                file: entry.file.clone(),
                name: entry.name.clone(),
            });
        }
        if ui
            .add_enabled(idle && editable, egui::Button::new("Move to Group…"))
            .on_disabled_hover_text(
                "Select looks. Camera profiles and Adobe's looks keep their places.",
            )
            .clicked()
        {
            prompt = Some(Prompt::Move {
                files: files.clone(),
                group,
            });
        }
        if ui
            .add_enabled(
                idle && !files.is_empty(),
                egui::Button::new("Move to Trash…"),
            )
            .clicked()
        {
            let dependents = manager
                .entries
                .as_deref()
                .map(|entries| manage::dependents(entries, &files))
                .unwrap_or_default();
            prompt = Some(Prompt::Trash {
                files: files.clone(),
                dependents,
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, "Done").clicked() {
                asked.push(Asked::Close);
            }
        });
    });
    if prompt.is_some() {
        manager.error = None;
        manager.prompt = prompt;
    }
}

/// A rename, a move or a removal, asked before it is made.
fn prompt_row(
    ui: &mut egui::Ui,
    manager: &mut ProfileManager,
    prompt: Prompt,
    idle: bool,
    asked: &mut Vec<Asked>,
) {
    let mut confirm = None;
    let cancel = match prompt {
        Prompt::Rename { file, mut name } => {
            let old = manager
                .entry(&file)
                .map_or_else(String::new, |e| e.name.clone());
            prompt_label(ui, &format!("New name for {old}"));
            let (edit, ok, no) = buttons(ui, "Rename", idle, |ui| field(ui, &mut name, "", 360.));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(Prompt::Rename { name: kept, .. }) = &mut manager.prompt {
                kept.clone_from(&name);
            }
            if ok || enter {
                confirm = Some(Op::Rename { file, name });
            }
            no
        }
        Prompt::Move { files, mut group } => {
            prompt_label(
                ui,
                &format!(
                    "Move {} to a group. A new name makes a new group; leave it empty \
                     for the top of the menu.",
                    plural(files.len(), "look", "looks")
                ),
            );
            let groups: BTreeSet<String> = manager
                .entries
                .iter()
                .flatten()
                .filter(|e| e.editable() && !e.group.is_empty())
                .map(|e| e.group.clone())
                .collect();
            let ((), ok, no) = buttons(ui, "Move", idle, |ui| {
                field(ui, &mut group, "New or existing group", 260.);
                // In a scope: the drop-down styles the rest of its row, and
                // Cancel and Move follow it there.
                ui.scope(|ui| {
                    widgets::dropdown(ui, "manage-profiles-groups")
                        .selected_text("Existing groups")
                        .width(160.)
                        .show_ui(ui, |ui| {
                            for g in &groups {
                                if ui.selectable_label(*g == group, g).clicked() {
                                    group.clone_from(g);
                                }
                            }
                        });
                });
            });
            if let Some(Prompt::Move { group: kept, .. }) = &mut manager.prompt {
                kept.clone_from(&group);
            }
            if ok {
                confirm = Some(Op::Move {
                    files,
                    group,
                    renaming: None,
                });
            }
            no
        }
        Prompt::RenameGroup { old, mut name } => {
            prompt_label(ui, &format!("New name for the group {old}"));
            let (edit, ok, no) = buttons(ui, "Rename", idle, |ui| field(ui, &mut name, "", 360.));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(Prompt::RenameGroup { name: kept, .. }) = &mut manager.prompt {
                kept.clone_from(&name);
            }
            if (ok || enter) && !name.trim().is_empty() {
                let files = manager
                    .entries
                    .iter()
                    .flatten()
                    .filter(|e| e.editable() && e.group == old)
                    .map(|e| e.file.clone())
                    .collect();
                confirm = Some(Op::Move {
                    files,
                    group: name,
                    renaming: Some(old),
                });
            }
            no
        }
        Prompt::Trash { files, dependents } => {
            // By name, as the filter may hide some of the selection.
            let mut names: Vec<String> = files
                .iter()
                .map(|f| {
                    manager.entry(f).map_or_else(
                        || {
                            f.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        },
                        |e| e.name.clone(),
                    )
                })
                .collect();
            if names.len() > 12 {
                let more = names.len() - 10;
                names.truncate(10);
                names.push(format!("and {more} more"));
            }
            let mut said = format!(
                "Move {} to the Trash: {}? Edits that use them keep them; presets that name them can't be applied until they are imported again.",
                plural(files.len(), "profile", "profiles"),
                names.join(", ")
            );
            if !dependents.is_empty() {
                said += &format!(
                    "\n\nThese looks need a removed camera profile and will no longer be listed: {}.",
                    dependents.join(", ")
                );
            }
            ui.label(said);
            ui.add_space(space::S);
            let ((), ok, no) = buttons(ui, "Move to Trash", idle, |_| ());
            if ok {
                confirm = Some(Op::Trash(files));
            }
            no
        }
    };
    if let Some(op) = confirm {
        asked.push(Asked::Op(op));
    }
    if cancel {
        manager.prompt = None;
    }
}

/// What a prompt asks, a small gap above its field.
fn prompt_label(ui: &mut egui::Ui, text: &str) {
    ui.label(text);
    ui.add_space(space::XS);
}

/// A prompt's text field, as tall as the drop-downs and buttons beside it.
fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .min_size(egui::vec2(0., widgets::DROPDOWN_HEIGHT))
            .vertical_align(egui::Align::Center),
    )
}

/// One row: `leading` (a prompt's field) at its start, Cancel and the
/// prompt's action at its end, all one height. Returns what `leading` gave and
/// whether the action and Cancel were clicked.
fn buttons<R>(
    ui: &mut egui::Ui,
    action: &str,
    idle: bool,
    leading: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool, bool) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), widgets::DROPDOWN_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let led = leading(ui);
            let (ok, no) = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    let ok = ui
                        .add_enabled_ui(idle, |ui| primary_button(ui, action).clicked())
                        .inner;
                    (ok, ui.button("Cancel").clicked())
                })
                .inner;
            (led, ok, no)
        },
    )
    .inner
}

#[cfg(test)]
#[path = "profile_manager_editor_tests.rs"]
mod editor_tests;
