use super::Editor;
use super::bulk_import::ImportKind;
use super::dialogs::FileDialog;
use super::widgets::segmented;
use super::worker::Event;
use crate::app::theme;
use crate::model::recipe::Recipe;
use anyhow::Context as _;
use eframe::egui::{self, Sense, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a preset is hovered before the photo previews it.
const HOVER_PREVIEW: Duration = Duration::from_millis(300);

impl Editor {
    pub(super) fn reload_presets(&mut self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let scan = self.presets.next_scan();
        super::task::spawn(
            tx,
            ctx,
            move |tx| {
                let library = Arc::new(crate::presets::load_library());
                let _ = tx.send(Event::XmpLibrary { scan, library });
            },
            move |tx, error| {
                let _ = tx.send(Event::PresetScanFailed { scan, error });
            },
        );
    }
    /// A scan that failed: the presets read before stay, and commands stop waiting.
    pub(super) fn preset_scan_failed(&mut self, scan: u64, error: String) {
        if self.presets.is_latest(scan) {
            self.presets.scanned = true;
            self.status = format!("Presets not read: {error}");
        }
    }
    /// A finished library scan, unless a later one has started since: scans run in
    /// parallel and may finish in any order.
    pub(super) fn presets_scanned(&mut self, scan: u64, library: Arc<crate::presets::Library>) {
        if self.presets.is_latest(scan) {
            self.presets.library = library;
            self.presets.scanned = true;
            // A preset named as a raw default may have been imported or changed.
            if let Err(e) = self.set_raw_defaults(self.raw_defaults.settings().clone()) {
                self.status = format!("Raw defaults not saved: {e:#}");
            }
            self.refresh_preset_support();
        }
    }
    pub(super) fn refresh_preset_support(&mut self) {
        let had_preview = self.presets.preview.take().is_some();
        self.presets.hover = None;
        if had_preview {
            self.schedule();
        }
        // The list depends on which presets fit this photo.
        self.presets.revision += 1;
        let Some(m) = &self.document.metadata else {
            self.presets.issues.clear();
            self.presets.substitutes.clear();
            return;
        };
        let base = Recipe::with_profiles(m, &self.document.profiles);
        self.presets.issues = self
            .presets
            .library
            .presets
            .iter()
            .map(|p| {
                p.apply(&base, m, &self.document.profiles, None)
                    .err()
                    .map(|e| format!("{e:#}"))
            })
            .collect();
        self.presets.substitutes = self
            .presets
            .library
            .presets
            .iter()
            .map(|p| p.profile_substitute(m, &self.document.profiles))
            .collect();
    }
    pub(super) fn presets_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let library = self.presets.library.clone();
        let mut clicked = None;
        let mut hovered = None;
        let mut user_action = None;
        let user = crate::presets::user::UserPresets::default();
        let mut import = None;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("preset-list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 8.);
                    // The caption names the tab, with how many presets fit
                    // this camera and the import menu at its right.
                    let caption = super::widgets::panel_caption(ui, "Presets");
                    let add = egui::Rect::from_center_size(
                        egui::pos2(caption.right() - 14., caption.center().y),
                        Vec2::splat(22.),
                    );
                    let response = ui.interact(add, ui.id().with("preset-import"), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(add, 4., palette.gray(44));
                    }
                    let tint = palette.gray(if response.hovered() { 235 } else { 150 });
                    crate::app::icons::paint_at(
                        ui.painter(),
                        crate::app::icons::Icon::Add,
                        add.center(),
                        13.,
                        tint,
                    );
                    let response = response
                        .on_hover_text("Import Lightroom presets (.xmp)")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&response).show(|ui| {
                        ui.set_min_width(190.);
                        if ui
                            .add(egui::Button::new("Import Presets…").frame(false))
                            .on_hover_text("Choose one or more .xmp presets")
                            .clicked()
                        {
                            import = Some(FileDialog::ImportXmp);
                            ui.close();
                        }
                        if ui
                            .add(egui::Button::new("Import Folder…").frame(false))
                            .on_hover_text("Import every preset in a folder and its subfolders")
                            .clicked()
                        {
                            import = Some(FileDialog::ImportFolder(ImportKind::Presets));
                            ui.close();
                        }
                    });
                    super::widgets::search_field(ui, &mut self.presets.filter, "Search presets");
                    let mut show = match (self.presets.favorites_only, self.presets.compatible_only)
                    {
                        (true, _) => 1,
                        (false, true) => 2,
                        _ => 0,
                    };
                    // Each filter with how many presets it shows: Compatible,
                    // those that fit this camera's photo.
                    let total = library.presets.len();
                    let favorites = library
                        .presets
                        .iter()
                        .filter(|p| self.presets.favorites.contains(&p.id))
                        .count();
                    let fit = self.presets.issues.iter().filter(|e| e.is_none()).count();
                    let labels = [
                        format!("All {total}"),
                        format!("Favorites {favorites}"),
                        // Nothing to fit without a photo.
                        if self.document.metadata.is_some() {
                            format!("Compatible {fit}")
                        } else {
                            "Compatible".into()
                        },
                    ];
                    let w = ui.available_width();
                    if segmented(
                        ui,
                        &mut show,
                        &[(0, &labels[0]), (1, &labels[1]), (2, &labels[2])],
                        w,
                    ) {
                        self.presets.favorites_only = show == 1;
                        self.presets.compatible_only = show == 2;
                    }
                    self.preset_amount_ui(ui);
                    if !library.errors.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} unreadable files",
                                library.errors.len()
                            ))
                            .size(10.)
                            .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(library.errors.join("\n"));
                    }
                    let groups = self.preset_list();
                    if groups.is_empty() && !library.presets.is_empty() {
                        let empty = EmptyList::of(&self.presets, favorites);
                        if empty_state(ui, &empty) {
                            match empty.action {
                                EmptyAction::ClearSearch => self.presets.filter.clear(),
                                EmptyAction::ShowAll => {
                                    self.presets.favorites_only = false;
                                    self.presets.compatible_only = false;
                                }
                            }
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 0.;
                    let force_open = !self.presets.filter.is_empty() || self.presets.favorites_only;
                    for PresetGroup {
                        imported,
                        name: group,
                        presets,
                    } in groups.iter()
                    {
                        let id = ui.make_persistent_id(("preset-group", imported, group));
                        let open = force_open
                            || ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                        if list_row(ui, group, Some(presets.len()), 0, Some(open), false, true)
                            .clicked()
                            && !force_open
                        {
                            ui.ctx().data_mut(|d| d.insert_temp(id, !open));
                        }
                        if !open {
                            continue;
                        }
                        for (i, name) in presets {
                            let i = *i;
                            let p = &library.presets[i];
                            let issue = self.presets.issues.get(i).and_then(Option::as_ref);
                            // Incompatible presets are grayed out but still apply
                            // everything they can, as in Lightroom.
                            let enabled = self.document.full().is_some();
                            let favorite = self.presets.favorites.contains(&p.id);
                            let response = list_row(
                                ui,
                                name,
                                None,
                                1,
                                None,
                                self.presets.selected == p.id,
                                enabled && issue.is_none(),
                            );
                            // Off screen in the scrolled list: nothing more to draw.
                            if !ui.is_rect_visible(response.rect) {
                                continue;
                            }
                            // Favorite star on the right, shown on hover or when set.
                            let star = egui::Rect::from_center_size(
                                egui::pos2(response.rect.right() - 12., response.rect.center().y),
                                Vec2::splat(16.),
                            );
                            let star_response =
                                ui.interact(star, ui.id().with(("favorite", i)), Sense::click());
                            if favorite || response.hovered() || star_response.hovered() {
                                ui.painter().text(
                                    star.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "★",
                                    egui::FontId::proportional(12.),
                                    palette.gray(if star_response.hovered() {
                                        240
                                    } else if favorite {
                                        200
                                    } else {
                                        110
                                    }),
                                );
                            }
                            if star_response
                                .on_hover_text(if favorite {
                                    "Remove from favorites"
                                } else {
                                    "Add to favorites"
                                })
                                .clicked()
                            {
                                if favorite {
                                    self.presets.favorites.remove(&p.id);
                                } else {
                                    self.presets.favorites.insert(p.id.clone());
                                }
                                self.presets.revision += 1;
                                if let Err(e) =
                                    crate::presets::save_favorites(&self.presets.favorites)
                                {
                                    self.status = e.to_string();
                                }
                                continue;
                            }
                            if enabled && response.clicked() {
                                clicked = Some(i);
                            }
                            // Presets made here can be changed, as Lightroom's own.
                            if user.owns(p) {
                                response.context_menu(|ui| {
                                    use super::user_presets::PresetAction;
                                    if ui.button("Update with Current Settings").clicked() {
                                        user_action = Some(PresetAction::Update(i));
                                    }
                                    if ui.button("Rename…").clicked() {
                                        user_action = Some(PresetAction::StartRename(i));
                                    }
                                    if ui.button("Delete").clicked() {
                                        user_action = Some(PresetAction::Delete(i));
                                    }
                                });
                            }
                            if response.hovered() && enabled {
                                hovered = Some(i);
                            }
                            // Built only while it shows.
                            let substitute =
                                self.presets.substitutes.get(i).and_then(Option::as_ref);
                            response.on_hover_ui(|ui| {
                                ui.label(preset_detail(p, issue, substitute));
                            });
                        }
                    }
                });
            });
        if let Some(dialog) = import {
            self.dialog(dialog, &ui.ctx().clone());
        }
        if let Some(action) = user_action {
            self.preset_action(action);
        }
        if let Some(i) = clicked {
            // The status bar reports a preset that could not be applied.
            let _ = self.apply_preset(i);
        } else if let Some(i) = hovered {
            if self.presets.hover.as_ref().is_none_or(|(old, _)| *old != i) {
                if self.presets.preview.take().is_some() {
                    self.schedule();
                }
                self.presets.hover = Some((i, Instant::now()));
            }
            if self.presets.preview.is_none()
                && self
                    .presets
                    .hover
                    .as_ref()
                    .is_some_and(|(_, t)| t.elapsed() > HOVER_PREVIEW)
                && let Some(m) = &self.document.metadata
                && let Ok((mut r, _)) = library.presets[i].apply_lenient(
                    self.document.edit.recipe(),
                    m,
                    &self.document.profiles,
                    self.document
                        .measures()
                        .as_ref()
                        .map(|m| m as &dyn crate::xmp::PhotoMeasures),
                )
            {
                this_photos_upright(&mut r, self.document.edit.recipe());
                self.presets.preview = Some(r);
                self.schedule();
            }
            // Woken when the hover has lasted long enough to preview; not after,
            // when a preset that cannot apply would otherwise keep it awake.
            if self.presets.preview.is_none()
                && let Some((_, t)) = &self.presets.hover
                && let Some(left) = HOVER_PREVIEW.checked_sub(t.elapsed())
            {
                ui.ctx()
                    .request_repaint_after(left + Duration::from_millis(10));
            }
        } else {
            self.end_preset_hover();
        }
    }
    /// Ends a hover over a preset, rendering the edit again if it was previewed.
    pub(super) fn end_preset_hover(&mut self) {
        self.presets.hover = None;
        if self.presets.preview.take().is_some() {
            self.schedule();
        }
    }
}

/// A preset applied to the open photo, as the status bar and the control API report it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct AppliedPreset {
    pub name: String,
    /// The profile used in place of one the preset names but this camera lacks.
    pub substitute: Option<String>,
    /// The settings that do not fit this photo and were left as they were.
    pub skipped: Vec<String>,
}

impl std::fmt::Display for AppliedPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Applied {}", self.name)?;
        if let Some(used) = &self.substitute {
            write!(f, " · using {used}")?;
        }
        if !self.skipped.is_empty() {
            write!(f, " · skipped: {}", self.skipped.join("; "))?;
        }
        Ok(())
    }
}

/// Whether an Amount change changed the photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AmountChange {
    Same,
    Changed,
}

/// Lightroom's preset Amount while it shows: after a preset that supports one is
/// applied, until anything else changes the photo.
pub(super) struct AmountSession {
    name: String,
    amount: f32,
    scale: crate::presets::amount::PresetAmount,
    /// The settings it last set, to notice any other change.
    shown: Recipe,
}

impl AmountSession {
    /// Whether `current` is still what this Amount set. Upright's corrections are
    /// analysed from the photo and land whenever the analysis finishes, so they don't
    /// count; a new Upright mode or guide does.
    fn still_shown(&self, current: &Recipe) -> bool {
        if current.upright.corrections == self.shown.upright.corrections {
            return self.shown == *current;
        }
        let mut shown = self.shown.clone();
        shown
            .upright
            .corrections
            .clone_from(&current.upright.corrections);
        shown == *current
    }
}

impl Editor {
    /// Applies preset `i` of the library to the open photo, as a click does, and
    /// says so in the status bar either way.
    pub(super) fn apply_preset(&mut self, i: usize) -> anyhow::Result<AppliedPreset> {
        let applied = self.apply_library_preset(i);
        self.status = match &applied {
            Ok(applied) => applied.to_string(),
            Err(e) => format!("Preset not applied: {e:#}"),
        };
        applied
    }
    fn apply_library_preset(&mut self, i: usize) -> anyhow::Result<AppliedPreset> {
        self.presets.preview = None;
        self.presets.hover = None;
        self.presets.amount = None;
        let library = self.presets.library.clone();
        let preset = &library.presets[i];
        let m = self
            .document
            .metadata
            .as_ref()
            .context("Open a photo in Develop first")?;
        let (mut r, skipped) = preset.apply_lenient(
            self.document.edit.recipe(),
            m,
            &self.document.profiles,
            self.document
                .measures()
                .as_ref()
                .map(|m| m as &dyn crate::xmp::PhotoMeasures),
        )?;
        let substitute = preset
            .profile_substitute(m, &self.document.profiles)
            .map(|(_, used)| used);
        this_photos_upright(&mut r, self.document.edit.recipe());
        let before = self.document.edit.replace(r);
        self.ensure_upright();
        // The settings before it are kept once, and every Amount is computed
        // from them again, so dragging never drifts.
        let full = self.document.edit.recipe().clone();
        self.presets.amount =
            crate::presets::amount::PresetAmount::new(preset, before, full.clone())
                .ok()
                .map(|scale| AmountSession {
                    name: crate::presets::display_name(&preset.name),
                    amount: 1.,
                    scale,
                    shown: full,
                });
        self.presets.selected = preset.id.clone();
        Ok(AppliedPreset {
            name: crate::presets::display_name(&preset.name),
            substitute,
            skipped,
        })
    }
    /// Sets the Amount of the preset just applied (0–2, 1 = 100%), and whether that
    /// changed the photo.
    pub(super) fn set_preset_amount(&mut self, amount: f32) -> AmountChange {
        // An edit earlier in this frame (a mask's name kept as the slider was clicked)
        // ends the Amount now: the settings it holds would take that edit back.
        if self.presets.amount.is_some() {
            self.end_stale_preset_amount();
            if self.presets.amount.is_none() {
                self.context.data_mut(|d| {
                    d.remove_temp::<(String, String)>(super::widgets::history_step_id())
                });
                return AmountChange::Same;
            }
        }
        let (Some(session), Some(m)) = (&mut self.presets.amount, &self.document.metadata) else {
            return AmountChange::Same;
        };
        session.amount = amount;
        session.shown = session.scale.at(amount, m);
        // Upright's analysis may have landed since: the photo's.
        session
            .shown
            .upright
            .corrections
            .clone_from(&self.document.edit.recipe().upright.corrections);
        // Named only when it changes the photo: a label left over would name the
        // next edit.
        if session.shown == *self.document.edit.recipe() {
            // The slider named a step; with nothing changed it would name the next edit.
            self.context
                .data_mut(|d| d.remove_temp::<(String, String)>(super::widgets::history_step_id()));
            return AmountChange::Same;
        }
        self.document.edit.replace(session.shown.clone());
        self.document.edit.name_next_step(super::history::Step::new(
            "Preset Amount",
            format!("{:.0}", amount * 100.),
        ));
        AmountChange::Changed
    }
    /// Ends the Amount once anything else has changed the photo, as Lightroom hides it.
    pub(super) fn end_stale_preset_amount(&mut self) {
        if self
            .presets
            .amount
            .as_ref()
            .is_some_and(|s| !s.still_shown(self.document.edit.recipe()))
        {
            self.presets.amount = None;
        }
    }
    /// The Amount slider, as Lightroom shows it at the top of the Presets panel. Any
    /// other change to the photo (an edit, Undo, another preset) ends it; see
    /// `end_stale_preset_amount`, which runs every frame.
    fn preset_amount_ui(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &self.presets.amount else {
            return;
        };
        let mut amount = session.amount;
        ui.add_space(2.);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&session.name)
                    .size(11.)
                    .color(theme::palette(ui.ctx()).gray(170)),
            )
            .truncate(),
        );
        super::widgets::set_edit_context(ui, "Preset");
        ui.push_id("preset-amount", |ui| {
            super::widgets::slider_with(
                ui,
                "Amount",
                &mut amount,
                crate::presets::amount::RANGE,
                1.,
                Some((100., 0)),
                None,
            )
        });
        super::widgets::set_edit_context(ui, "");
        if amount != session.amount {
            self.set_preset_amount(amount);
        }
    }
}

/// The preset list as shown: its groups in order, each with its presets'
/// indices and display names. Building it means formatting and matching
/// every preset, so it is kept until the search, the filters, the library,
/// the favorites or the photo's compatibility change.
pub(super) struct PresetList {
    library: Arc<crate::presets::Library>,
    filter: String,
    favorites_only: bool,
    compatible_only: bool,
    revision: u64,
    groups: Arc<Vec<PresetGroup>>,
}
pub(super) struct PresetGroup {
    imported: bool,
    pub(super) name: String,
    pub(super) presets: Vec<(usize, String)>,
}

impl PresetList {
    /// Whether this list is still the one to show; compares without
    /// allocating, as it runs every frame.
    fn shows(&self, browser: &super::state::PresetBrowser) -> bool {
        Arc::ptr_eq(&self.library, &browser.library)
            && self.filter == browser.filter
            && self.favorites_only == browser.favorites_only
            && self.compatible_only == browser.compatible_only
            && self.revision == browser.revision
    }
    fn build(browser: &super::state::PresetBrowser) -> Self {
        let query = browser.filter.to_lowercase();
        // Built-in groups first, in Lightroom's order, then imported groups
        // by name. A built-in and an imported group of the same name stay
        // apart.
        let mut groups: std::collections::BTreeMap<(bool, usize, String), Vec<(usize, String)>> =
            Default::default();
        for (i, p) in browser.library.presets.iter().enumerate() {
            let issue = browser.issues.get(i).and_then(Option::as_ref);
            if (browser.compatible_only && issue.is_some())
                || (browser.favorites_only && !browser.favorites.contains(&p.id))
                || !crate::presets::display_name(&format!("{} {}", p.group, p.name))
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            let rank = if p.builtin {
                crate::presets::builtin::group_rank(&p.group)
            } else {
                0
            };
            groups
                .entry((!p.builtin, rank, p.group.clone()))
                .or_default()
                .push((i, crate::presets::display_name(&p.name)));
        }
        Self {
            library: browser.library.clone(),
            filter: browser.filter.clone(),
            favorites_only: browser.favorites_only,
            compatible_only: browser.compatible_only,
            revision: browser.revision,
            groups: Arc::new(
                groups
                    .into_iter()
                    .map(|((imported, _, name), presets)| PresetGroup {
                        imported,
                        name,
                        presets,
                    })
                    .collect(),
            ),
        }
    }
}

impl Editor {
    /// The preset list for the current search and filters; see `PresetList`.
    pub(super) fn preset_list(&mut self) -> Arc<Vec<PresetGroup>> {
        let list = match self.presets.list.take() {
            Some(list) if list.shows(&self.presets) => list,
            _ => PresetList::build(&self.presets),
        };
        let groups = list.groups.clone();
        self.presets.list = Some(list);
        groups
    }
}

/// A preset's tooltip: its name, then whether it fits this photo and how.
fn preset_detail(
    p: &crate::xmp::Preset,
    issue: Option<&String>,
    substitute: Option<&(String, String)>,
) -> String {
    let detail = match issue {
        Some(issue) => format!(
            "{}\nNot fully compatible: {issue}\nClick to apply the supported settings",
            p.name
        ),
        None => format!(
            "{}\nClick to apply · hover to preview\n{}",
            p.name,
            p.notes.join("\n")
        ),
    };
    match substitute {
        Some((asked, used)) => format!(
            "{detail}\nMade for {asked}; renders with {used} because {asked} isn't imported for this camera"
        ),
        None => detail,
    }
}

/// Why the preset list shows nothing, and the one step that brings presets
/// back.
struct EmptyList {
    title: String,
    detail: &'static str,
    action: EmptyAction,
}
#[derive(Clone, Copy)]
enum EmptyAction {
    ClearSearch,
    ShowAll,
}
impl EmptyList {
    /// For a list emptied by the browser's search and filters; `favorites`
    /// is how many favorites there are.
    fn of(browser: &super::state::PresetBrowser, favorites: usize) -> Self {
        let query = browser.filter.trim();
        if browser.favorites_only && favorites == 0 {
            Self {
                title: "No favorites yet".into(),
                detail: "Hover a preset and click its \u{2605} to keep it here.",
                action: EmptyAction::ShowAll,
            }
        } else if !query.is_empty() {
            Self {
                title: format!("No presets match \u{201c}{query}\u{201d}"),
                detail: if browser.favorites_only {
                    "Searched your favorites only."
                } else if browser.compatible_only {
                    "Searched the presets that fit this camera only."
                } else {
                    "Try another name or group."
                },
                action: EmptyAction::ClearSearch,
            }
        } else {
            Self {
                title: "No presets fit this camera".into(),
                detail: "Compatible shows the presets this photo can take in full.",
                action: EmptyAction::ShowAll,
            }
        }
    }
}
/// The empty list's message, centred, with its action as a button; returns
/// whether the button was clicked.
fn empty_state(ui: &mut egui::Ui, empty: &EmptyList) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut clicked = false;
    ui.add_space(20.);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 6.;
        ui.label(
            egui::RichText::new(&empty.title)
                .size(12.5)
                .color(palette.gray(215)),
        );
        ui.label(
            egui::RichText::new(empty.detail)
                .size(11.)
                .color(palette.gray(130)),
        );
        ui.add_space(6.);
        let label = match empty.action {
            EmptyAction::ClearSearch => "Clear Search",
            EmptyAction::ShowAll => "Show All Presets",
        };
        clicked = ui
            .add(
                egui::Button::new(egui::RichText::new(label).size(12.))
                    .corner_radius(6.)
                    .min_size(Vec2::new(140., 26.)),
            )
            .clicked();
    });
    clicked
}

/// A Lightroom-style list row for preset folders and presets: fixed height,
/// indent, optional disclosure triangle and right-aligned count.
fn list_row(
    ui: &mut egui::Ui,
    text: &str,
    count: Option<usize>,
    depth: usize,
    open: Option<bool>,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::click());
    // Scrolled out of view: keep its place but skip laying out its text.
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = enabled && response.hovered();
    if selected || hovered {
        let fill = if selected {
            palette.selected_row()
        } else {
            palette.gray(34)
        };
        ui.painter().rect_filled(rect, 4., fill);
    }
    // A group's chevron as the folder tree's; a preset's name under the
    // group's.
    let x = rect.left() + 10. + depth as f32 * 14.;
    let y = rect.center().y;
    if let Some(open) = open {
        let chevron = if open {
            crate::app::icons::Icon::ChevronDown
        } else {
            crate::app::icons::Icon::ChevronRight
        };
        let tint = palette.gray(if hovered { 200 } else { 140 });
        crate::app::icons::paint_at(ui.painter(), chevron, Pos2::new(x, y), 11., tint);
    }
    let text_left = x + if open.is_some() { 12. } else { 0. };
    let right = rect.right() - if count.is_some() { 36. } else { 26. };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - text_left).max(1.),
        FontId::proportional(12.),
    );
    ui.painter().galley(
        Pos2::new(text_left, y - galley.size().y / 2.),
        galley,
        palette.gray(match (enabled, selected || hovered) {
            (false, _) => 95,
            (true, true) => 240,
            (true, false) => 200,
        }),
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(10.5),
            palette.gray(120),
        );
    }
    response
}

/// A preset's Upright mode, with this photo's own corrections rather than any the preset
/// carries from the photo it was saved from; a mode without one is analysed on apply.
/// New lens settings call for a new analysis, as pasting them does.
fn this_photos_upright(r: &mut Recipe, current: &Recipe) {
    if r.upright != current.upright {
        let mode = r.upright.mode;
        r.upright.clone_from(&current.upright);
        r.upright.mode = mode;
        if mode == crate::model::transform::UprightMode::Guided && r.upright.correction().is_none()
        {
            r.upright.mode = current.upright.mode;
        }
    }
    use crate::model::transform::LensInputs;
    if LensInputs::of(r) != LensInputs::of(current) {
        r.upright.analyse_again();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_preset_list_says_why_and_offers_the_way_back() {
        let mut browser = crate::app::state::PresetBrowser {
            favorites_only: true,
            ..Default::default()
        };
        let none_yet = EmptyList::of(&browser, 0);
        assert_eq!(none_yet.title, "No favorites yet");
        assert!(matches!(none_yet.action, EmptyAction::ShowAll));
        // With favorites, it is the search that found nothing among them.
        browser.filter = " portra ".into();
        let searched = EmptyList::of(&browser, 3);
        assert_eq!(searched.title, "No presets match \u{201c}portra\u{201d}");
        assert_eq!(searched.detail, "Searched your favorites only.");
        assert!(matches!(searched.action, EmptyAction::ClearSearch));
        // Compatible with nothing fitting and no search.
        browser = crate::app::state::PresetBrowser {
            compatible_only: true,
            ..Default::default()
        };
        let none_fit = EmptyList::of(&browser, 3);
        assert_eq!(none_fit.title, "No presets fit this camera");
        assert!(matches!(none_fit.action, EmptyAction::ShowAll));
    }
    /// A preset that changes only a lens setting leaves no Upright correction analysed
    /// through the old one: the editor analyses the photo again.
    #[test]
    fn presets_with_new_lens_settings_drop_the_photos_upright_analysis() {
        let mut current = Recipe::default();
        current.upright.mode = crate::model::transform::UprightMode::Level;
        current.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
        let mut same = current.clone();
        same.exposure = 1.;
        this_photos_upright(&mut same, &current);
        assert_eq!(same.upright, current.upright);
        let mut lens = current.clone();
        lens.lens_manual_distortion = 0.3;
        this_photos_upright(&mut lens, &current);
        assert!(lens.upright.corrections.is_empty());
        assert_eq!(
            lens.upright.mode,
            crate::model::transform::UprightMode::Level
        );
    }
}
