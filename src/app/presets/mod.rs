use crate::app::Editor;
use crate::app::presets::bulk_import::ImportKind;
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::design::{Tone, radius, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::segmented;
use crate::app::worker::Event;
use crate::model::recipe::Recipe;
use anyhow::Context as _;
use eframe::egui::{self, Sense, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a preset is hovered before the photo previews it.
const HOVER_PREVIEW: Duration = Duration::from_millis(300);

impl Editor {
    pub(in crate::app) fn reload_presets(&mut self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let scan = self.presets.next_scan();
        crate::app::task::spawn(
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
    pub(in crate::app) fn preset_scan_failed(&mut self, scan: u64, error: String) {
        if self.presets.is_latest(scan) {
            self.presets.scanned = true;
            self.messages.fail(format!("Presets not read: {error}"));
        }
    }
    /// A finished library scan, unless a later one has started since: scans run in
    /// parallel and may finish in any order.
    pub(in crate::app) fn presets_scanned(
        &mut self,
        scan: u64,
        library: Arc<crate::presets::Library>,
    ) {
        if self.presets.is_latest(scan) {
            self.presets.library = library;
            self.presets.scanned = true;
            // A preset named as a raw default may have been imported or changed.
            if let Err(e) = self.set_raw_defaults(self.raw_defaults.settings().clone()) {
                self.messages.fail(format!("Raw defaults not saved: {e:#}"));
            }
            self.refresh_preset_support();
        }
    }
    pub(in crate::app) fn refresh_preset_support(&mut self) {
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
    pub(in crate::app) fn presets_ui(&mut self, ui: &mut egui::Ui) {
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
                    ui.spacing_mut().item_spacing = Vec2::new(space::XS, space::M);
                    // The caption names the tab, with how many presets fit
                    // this camera and the import menu at its right.
                    let caption = crate::app::ui::widgets::panel_caption(ui, "Presets");
                    let add = egui::Rect::from_center_size(
                        egui::pos2(caption.right() - 14., caption.center().y),
                        Vec2::splat(22.),
                    );
                    let response = ui.interact(add, ui.id().with("preset-import"), Sense::click());
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(add, radius::CONTROL, palette.tone(Tone::RAISED));
                    }
                    let tint = palette.tone(if response.hovered() {
                        Tone::STRONG
                    } else {
                        Tone::SECONDARY
                    });
                    crate::app::ui::icons::paint_at(
                        ui.painter(),
                        crate::app::ui::icons::Icon::Add,
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
                    crate::app::ui::widgets::search_field(
                        ui,
                        &mut self.presets.filter,
                        "Search presets",
                    );
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
                            .size(text::MICRO)
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
                                    egui::FontId::proportional(text::LABEL),
                                    palette.tone(if star_response.hovered() {
                                        Tone::STRONG
                                    } else if favorite {
                                        Tone::SOFT
                                    } else {
                                        Tone::DISABLED
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
                                    self.messages.fail(e.to_string());
                                }
                                continue;
                            }
                            // Control-click opens the menu on macOS, as a right-click does.
                            if enabled
                                && response.clicked()
                                && !crate::app::ui::widgets::context_clicked(&response)
                            {
                                clicked = Some(i);
                            }
                            // Presets made here can be changed, as Lightroom's own;
                            // imported ones renamed or deleted, and any preset file
                            // shown where it is.
                            if !p.builtin {
                                crate::app::ui::widgets::context_menu(&response, |ui| {
                                    if let Some(action) =
                                        crate::app::presets::user_presets::preset_menu(
                                            ui, &user, p, i,
                                        )
                                    {
                                        user_action = Some(action);
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
            // The top bar reports a preset that could not be applied.
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
                && let Ok(applied) = crate::presets::apply_to_photo(
                    &library.presets[i],
                    self.document.edit.recipe(),
                    m,
                    &self.document.profiles,
                    self.document
                        .measures()
                        .as_ref()
                        .map(|m| m as &dyn crate::xmp::PhotoMeasures),
                )
            {
                self.presets.preview = Some(applied.recipe);
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
    pub(in crate::app) fn end_preset_hover(&mut self) {
        self.presets.hover = None;
        if self.presets.preview.take().is_some() {
            self.schedule();
        }
    }
}

/// A preset applied to the open photo, as the status bar and the control API report it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::app) struct AppliedPreset {
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
pub(in crate::app) enum AmountChange {
    Same,
    Changed,
}

/// Lightroom's preset Amount while it shows: after a preset that supports one is
/// applied, until anything else changes the photo.
pub(in crate::app) struct AmountSession {
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
    /// says so in the top bar either way.
    pub(in crate::app) fn apply_preset(&mut self, i: usize) -> anyhow::Result<AppliedPreset> {
        let applied = self.apply_library_preset(i);
        match &applied {
            Ok(applied) => self.messages.note(applied.to_string()),
            Err(e) => self.messages.fail(format!("Preset not applied: {e:#}")),
        }
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
        let crate::presets::Applied {
            recipe,
            skipped,
            substitute,
        } = crate::presets::apply_to_photo(
            preset,
            self.document.edit.recipe(),
            m,
            &self.document.profiles,
            self.document
                .measures()
                .as_ref()
                .map(|m| m as &dyn crate::xmp::PhotoMeasures),
        )?;
        let before = self.document.edit.replace(recipe);
        self.ensure_analysis();
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
    pub(in crate::app) fn set_preset_amount(&mut self, amount: f32) -> AmountChange {
        // An edit earlier in this frame (a mask's name kept as the slider was clicked)
        // ends the Amount now: the settings it holds would take that edit back.
        if self.presets.amount.is_some() {
            self.end_stale_preset_amount();
            if self.presets.amount.is_none() {
                self.step_name.discard();
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
            self.step_name.discard();
            return AmountChange::Same;
        }
        self.document.edit.replace(session.shown.clone());
        self.document
            .edit
            .name_next_step(crate::app::history::Step::new(
                "Preset Amount",
                format!("{:.0}", amount * 100.),
            ));
        AmountChange::Changed
    }
    /// Ends the Amount once anything else has changed the photo, as Lightroom hides it.
    pub(in crate::app) fn end_stale_preset_amount(&mut self) {
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
        ui.add_space(space::XXS);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&session.name)
                    .size(text::SMALL)
                    .color(theme::palette(ui.ctx()).tone(Tone::LABEL)),
            )
            .truncate(),
        );
        crate::app::ui::widgets::set_edit_context(ui, "Preset");
        ui.push_id("preset-amount", |ui| {
            crate::app::ui::widgets::slider_with(
                ui,
                &mut self.step_name,
                "Amount",
                &mut amount,
                crate::presets::amount::RANGE,
                1.,
                Some((100., 0)),
                None,
            )
        });
        crate::app::ui::widgets::set_edit_context(ui, "");
        if amount != session.amount {
            self.set_preset_amount(amount);
        }
    }
}

/// The preset list as shown: its groups in order, each with its presets'
/// indices and display names. Building it means formatting and matching
/// every preset, so it is kept until the search, the filters, the library,
/// the favorites or the photo's compatibility change.
pub(in crate::app) struct PresetList {
    library: Arc<crate::presets::Library>,
    filter: String,
    favorites_only: bool,
    compatible_only: bool,
    revision: u64,
    groups: Arc<Vec<PresetGroup>>,
}
pub(in crate::app) struct PresetGroup {
    imported: bool,
    pub(in crate::app) name: String,
    pub(in crate::app) presets: Vec<(usize, String)>,
}

impl PresetList {
    /// Whether this list is still the one to show; compares without
    /// allocating, as it runs every frame.
    fn shows(&self, browser: &crate::app::develop::state::PresetBrowser) -> bool {
        Arc::ptr_eq(&self.library, &browser.library)
            && self.filter == browser.filter
            && self.favorites_only == browser.favorites_only
            && self.compatible_only == browser.compatible_only
            && self.revision == browser.revision
    }
    fn build(browser: &crate::app::develop::state::PresetBrowser) -> Self {
        let groups = preset_groups(&browser.library, &browser.filter, |i, p| {
            let issue = browser.issues.get(i).and_then(Option::as_ref);
            !(browser.compatible_only && issue.is_some())
                && !(browser.favorites_only && !browser.favorites.contains(&p.id))
        });
        Self {
            library: browser.library.clone(),
            filter: browser.filter.clone(),
            favorites_only: browser.favorites_only,
            compatible_only: browser.compatible_only,
            revision: browser.revision,
            groups: Arc::new(groups),
        }
    }
}

/// The presets of `library` that `keep` keeps and whose group or name contains
/// `filter`, in their groups: built-in groups first, in Lightroom's order, then
/// imported groups by name. A built-in and an imported group of the same name stay
/// apart.
pub(in crate::app) fn preset_groups(
    library: &crate::presets::Library,
    filter: &str,
    keep: impl Fn(usize, &crate::xmp::Preset) -> bool,
) -> Vec<PresetGroup> {
    let query = filter.to_lowercase();
    let mut groups: std::collections::BTreeMap<(bool, usize, String), Vec<(usize, String)>> =
        Default::default();
    for (i, p) in library.presets.iter().enumerate() {
        if !keep(i, p)
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
    groups
        .into_iter()
        .map(|((imported, _, name), presets)| PresetGroup {
            imported,
            name,
            presets,
        })
        .collect()
}

impl Editor {
    /// The preset list for the current search and filters; see `PresetList`.
    pub(in crate::app) fn preset_list(&mut self) -> Arc<Vec<PresetGroup>> {
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
    fn of(browser: &crate::app::develop::state::PresetBrowser, favorites: usize) -> Self {
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
    ui.add_space(space::XXL);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = space::S;
        ui.label(
            egui::RichText::new(&empty.title)
                .size(text::LABEL)
                .color(palette.tone(Tone::TEXT)),
        );
        ui.label(
            egui::RichText::new(empty.detail)
                .size(text::SMALL)
                .color(palette.tone(Tone::FAINT)),
        );
        ui.add_space(space::S);
        let label = match empty.action {
            EmptyAction::ClearSearch => "Clear Search",
            EmptyAction::ShowAll => "Show All Presets",
        };
        clicked = ui
            .add(
                egui::Button::new(egui::RichText::new(label).size(text::LABEL))
                    .corner_radius(radius::CARD)
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
            palette.tone(Tone::CARD)
        };
        ui.painter().rect_filled(rect, radius::CONTROL, fill);
    }
    // A group's chevron as the folder tree's; a preset's name under the
    // group's.
    let x = rect.left() + 10. + depth as f32 * 14.;
    let y = rect.center().y;
    if let Some(open) = open {
        let chevron = if open {
            crate::app::ui::icons::Icon::ChevronDown
        } else {
            crate::app::ui::icons::Icon::ChevronRight
        };
        let tint = palette.tone(if hovered { Tone::SOFT } else { Tone::MUTED });
        crate::app::ui::icons::paint_at(ui.painter(), chevron, Pos2::new(x, y), 11., tint);
    }
    let text_left = x + if open.is_some() { 12. } else { 0. };
    let right = rect.right() - if count.is_some() { 36. } else { 26. };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - text_left).max(1.),
        FontId::proportional(text::LABEL),
    );
    ui.painter().galley(
        Pos2::new(text_left, y - galley.size().y / 2.),
        galley,
        palette.tone(match (enabled, selected || hovered) {
            (false, _) => Tone::DISABLED,
            (true, true) => Tone::STRONG,
            (true, false) => Tone::SOFT,
        }),
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(text::SMALL),
            palette.tone(Tone::FAINT),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_preset_list_says_why_and_offers_the_way_back() {
        let mut browser = crate::app::develop::state::PresetBrowser {
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
        browser = crate::app::develop::state::PresetBrowser {
            compatible_only: true,
            ..Default::default()
        };
        let none_fit = EmptyList::of(&browser, 3);
        assert_eq!(none_fit.title, "No presets fit this camera");
        assert!(matches!(none_fit.action, EmptyAction::ShowAll));
    }
}

#[cfg(test)]
mod editor_tests;

pub(in crate::app) mod bulk_import;
pub(in crate::app) mod lut_import;
pub(in crate::app) mod user_presets;
