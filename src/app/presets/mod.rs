use crate::app::Editor;
use crate::app::ui::design::{self, Tone, inset, radius, space, text};
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
        let presets = self.preset_library().clone();
        crate::app::task::spawn(
            tx,
            ctx,
            move |tx| {
                let library = Arc::new(presets.load());
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
                p.apply(&base, m, &self.document.profiles, None, self.lensfun_db())
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
        let mut manage = false;
        let mut view_chosen = None;
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
                    if response
                        .on_hover_text("Manage presets: order, hide, delete and import")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        manage = true;
                    }
                    // Grid or List, left of the manage icon and apart from it.
                    for (k, view, icon, tip) in [
                        (
                            1.,
                            PresetView::List,
                            crate::app::ui::icons::Icon::ListView,
                            "List",
                        ),
                        (
                            2.,
                            PresetView::Grid,
                            crate::app::ui::icons::Icon::GridView,
                            "Grid",
                        ),
                    ] {
                        let rect = add.translate(Vec2::new(-(k * 24. + space::M), 0.));
                        let response =
                            ui.interact(rect, ui.id().with(("preset-view", tip)), Sense::click());
                        let on = self.presets.view == view;
                        if on || response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                radius::CONTROL,
                                palette.tone(Tone::RAISED),
                            );
                        }
                        let tint = palette.tone(if on || response.hovered() {
                            Tone::STRONG
                        } else {
                            Tone::SECONDARY
                        });
                        crate::app::ui::icons::paint_at(
                            ui.painter(),
                            icon,
                            rect.center(),
                            14.,
                            tint,
                        );
                        let response = crate::app::ui::widgets::named(response, tip)
                            .on_hover_text(match view {
                                PresetView::Grid => "Show presets as thumbnails of this photo",
                                PresetView::List => "Show presets as a list",
                            })
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() && !on {
                            view_chosen = Some(view);
                        }
                    }
                    // The rest at Develop's adjustments' margins, clear of the
                    // scroll bar that floats over the panel's right edge.
                    egui::Frame::new()
                        .inner_margin(design::margin(inset::ADJUSTMENT_CONTENT, 0.))
                        .show(ui, |ui| {
                            crate::app::ui::widgets::search_field(
                                ui,
                                &mut self.presets.filter,
                                "Search presets",
                            );
                            let mut show =
                                match (self.presets.favorites_only, self.presets.compatible_only) {
                                    (true, _) => 1,
                                    (false, true) => 2,
                                    _ => 0,
                                };
                            // Each filter with how many presets it shows: Compatible,
                            // those that fit this camera's photo.
                            let shown = |p: &&crate::xmp::Preset| {
                                !self.presets.layout.hidden.contains(&p.id)
                            };
                            let total = library.presets.iter().filter(shown).count();
                            let favorites = library
                                .presets
                                .iter()
                                .filter(shown)
                                .filter(|p| self.presets.favorites.contains(&p.id))
                                .count();
                            let fit = library
                                .presets
                                .iter()
                                .zip(&self.presets.issues)
                                .filter(|(p, e)| e.is_none() && shown(p))
                                .count();
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
                            let force_open =
                                !self.presets.filter.is_empty() || self.presets.favorites_only;
                            let grid = self.presets.view == PresetView::Grid;
                            let layout = TileLayout::of(ui);
                            let now = ui.input(|i| i.time);
                            let inputs = grid
                                .then(|| {
                                    thumbnail_inputs(
                                        &self.document,
                                        &self.presets.library,
                                        crate::environment::lensfun(&self.core),
                                        self.view.display_profile(),
                                    )
                                })
                                .flatten();
                            if grid {
                                self.preset_thumbnails.begin(
                                    ui.ctx(),
                                    now,
                                    layout.edge,
                                    inputs.as_ref(),
                                );
                            }
                            // Tiles this far above or below the scrolled view are rendered
                            // too, so a short scroll finds them there.
                            let ahead = ui
                                .clip_rect()
                                .expand2(Vec2::new(0., layout.side + layout.gap));
                            let mut toggled = None;
                            for PresetGroup {
                                imported,
                                name: group,
                                presets,
                            } in groups.iter()
                            {
                                let id = group_open_id(*imported, group);
                                let open = force_open
                                    || ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                                let row = list_row(
                                    ui,
                                    group,
                                    Some(presets.len()),
                                    0,
                                    Some(open),
                                    false,
                                    true,
                                );
                                if crate::app::ui::widgets::named(row, group).clicked()
                                    && !force_open
                                {
                                    ui.ctx().data_mut(|d| d.insert_temp(id, !open));
                                }
                                if !open {
                                    continue;
                                }
                                // Incompatible presets are grayed out but still apply
                                // everything they can, as in Lightroom.
                                let enabled = self.document.full().is_some();
                                let item = |ui: &mut egui::Ui,
                                            response: egui::Response,
                                            star: egui::Rect,
                                            i: usize,
                                            failed: Option<&str>|
                                 -> Item {
                                    let p = &library.presets[i];
                                    preset_item(
                                        ui,
                                        response,
                                        star,
                                        PresetFacts {
                                            p,
                                            i,
                                            issue: self
                                                .presets
                                                .issues
                                                .get(i)
                                                .and_then(Option::as_ref),
                                            substitute: self
                                                .presets
                                                .substitutes
                                                .get(i)
                                                .and_then(Option::as_ref),
                                            favorite: self.presets.favorites.contains(&p.id),
                                            enabled,
                                            user: &user,
                                            tile: grid,
                                            failed,
                                        },
                                    )
                                };
                                let mut outcome = |out: Item, i: usize| {
                                    if out.clicked {
                                        clicked = Some(i);
                                    }
                                    if out.hovered {
                                        hovered = Some(i);
                                    }
                                    if out.toggled {
                                        toggled = Some(i);
                                    }
                                    if out.action.is_some() {
                                        user_action = out.action;
                                    }
                                };
                                if !grid {
                                    for (i, name) in presets {
                                        let i = *i;
                                        let p = &library.presets[i];
                                        let issue =
                                            self.presets.issues.get(i).and_then(Option::as_ref);
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
                                            egui::pos2(
                                                response.rect.right() - 12.,
                                                response.rect.center().y,
                                            ),
                                            Vec2::splat(16.),
                                        );
                                        outcome(item(ui, response, star, i, None), i);
                                    }
                                    continue;
                                }
                                ui.add_space(layout.gap);
                                for pair in presets.chunks(2) {
                                    let (row, _) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), layout.side),
                                        Sense::hover(),
                                    );
                                    for (k, (i, name)) in pair.iter().enumerate() {
                                        let i = *i;
                                        let p = &library.presets[i];
                                        let rect = layout.tile(row, k);
                                        if ahead.intersects(rect) {
                                            self.preset_thumbnails.want(i, &p.id);
                                        }
                                        let response = ui.interact(
                                            rect,
                                            ui.id().with(("preset-tile", i)),
                                            Sense::click(),
                                        );
                                        let response =
                                            crate::app::ui::widgets::named(response, name);
                                        if !ui.is_rect_visible(rect) {
                                            continue;
                                        }
                                        let issue =
                                            self.presets.issues.get(i).and_then(Option::as_ref);
                                        let shown = self.preset_thumbnails.tile(&p.id);
                                        let failed = match shown {
                                            thumbnails::Tile::Failed(why) => Some(why.to_owned()),
                                            _ => None,
                                        };
                                        paint_tile(
                                            ui,
                                            rect,
                                            &shown,
                                            TileLook {
                                                name,
                                                enabled: enabled && issue.is_none(),
                                                selected: self.presets.selected == p.id,
                                                hovered: enabled && response.hovered(),
                                            },
                                        );
                                        let star = egui::Rect::from_center_size(
                                            egui::pos2(rect.right() - 12., rect.top() + 12.),
                                            Vec2::splat(18.),
                                        );
                                        outcome(item(ui, response, star, i, failed.as_deref()), i);
                                    }
                                    ui.add_space(layout.gap);
                                }
                            }
                            if grid {
                                let idle = !self.preview.task.is_running();
                                self.preset_thumbnails
                                    .end(ui.ctx(), now, idle, inputs.as_ref());
                            }
                            if let Some(i) = toggled {
                                let id = library.presets[i].id.clone();
                                if !self.presets.favorites.remove(&id) {
                                    self.presets.favorites.insert(id);
                                }
                                self.presets.revision += 1;
                                if let Err(e) =
                                    crate::presets::save_favorites(&self.presets.favorites)
                                {
                                    self.messages.fail(e.to_string());
                                }
                            }
                        });
                });
            });
        if manage {
            self.modal = Some(crate::app::Modal::ManagePresets(Default::default()));
        }
        if let Some(view) = view_chosen {
            self.presets.view = view;
            if let Err(e) = self.save_session() {
                self.messages.fail(format!("Preset view not saved: {e:#}"));
            }
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
                    self.lensfun_db(),
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

/// Lightroom's preset Amount while it shows: after a preset is applied, until
/// anything else changes the photo. A preset without an Amount shows it dimmed,
/// so the list below stays where it was from one preset to the next.
pub(in crate::app) struct AmountSession {
    name: String,
    amount: f32,
    /// `None` for a preset without an Amount.
    scale: Option<crate::presets::amount::PresetAmount>,
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
            self.lensfun_db(),
        )?;
        let before = self.document.edit.replace(recipe);
        self.ensure_analysis();
        // The settings before it are kept once, and every Amount is computed
        // from them again, so dragging never drifts.
        let full = self.document.edit.recipe().clone();
        self.presets.amount = Some(AmountSession {
            name: crate::presets::display_name(&preset.name),
            amount: 1.,
            scale: crate::presets::amount::PresetAmount::new(preset, before, full.clone()).ok(),
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
        let Some(scale) = &session.scale else {
            return AmountChange::Same;
        };
        session.shown = scale.at(amount, m);
        session.amount = amount;
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
        let adjustable = session.scale.is_some();
        crate::app::ui::widgets::set_edit_context(ui, "Preset");
        ui.push_id("preset-amount", |ui| {
            ui.add_enabled_ui(adjustable, |ui| {
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
            })
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
    pub(in crate::app) imported: bool,
    pub(in crate::app) name: String,
    pub(in crate::app) presets: Vec<(usize, String)>,
}
impl PresetGroup {
    pub(in crate::app) fn key(&self) -> String {
        crate::presets::PresetLayout::group_key(self.imported, &self.name)
    }
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
        let groups = preset_groups(
            &browser.library,
            &browser.filter,
            &browser.layout,
            |i, p| {
                let issue = browser.issues.get(i).and_then(Option::as_ref);
                !browser.layout.hidden.contains(&p.id)
                    && !(browser.compatible_only && issue.is_some())
                    && !(browser.favorites_only && !browser.favorites.contains(&p.id))
            },
        );
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
/// imported groups by name, unless `layout` orders them. A built-in and an imported
/// group of the same name stay apart.
pub(in crate::app) fn preset_groups(
    library: &crate::presets::Library,
    filter: &str,
    layout: &crate::presets::PresetLayout,
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
    let mut groups: Vec<PresetGroup> = groups
        .into_iter()
        .map(|((imported, _, name), presets)| PresetGroup {
            imported,
            name,
            presets,
        })
        .collect();
    // Stable, so groups the layout doesn't name keep their default order.
    groups.sort_by_key(|g| layout.rank(&g.key()));
    groups
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

/// Where whether a group of the panel is open is kept, in the context's
/// temporary data.
pub(in crate::app) fn group_open_id(imported: bool, group: &str) -> egui::Id {
    egui::Id::new(("preset-group-open", imported, group))
}

/// How the Presets panel shows its presets: as tiles of the open photo with each
/// applied, or as a list of names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum PresetView {
    #[default]
    Grid,
    List,
}

/// The grid's two columns: square tiles `side` points across with `gap` between
/// them, rendered `edge` pixels square at the display's scale.
struct TileLayout {
    side: f32,
    gap: f32,
    edge: u32,
}
impl TileLayout {
    fn of(ui: &egui::Ui) -> Self {
        let gap = space::XS;
        let side = ((ui.available_width() - gap) / 2.).floor().max(1.);
        Self {
            side,
            gap,
            edge: (side * ui.ctx().pixels_per_point()).round().max(1.) as u32,
        }
    }
    /// The `k`th tile (0 or 1) of `row`.
    fn tile(&self, row: egui::Rect, k: usize) -> egui::Rect {
        egui::Rect::from_min_size(
            row.min + Vec2::new(k as f32 * (self.side + self.gap), 0.),
            Vec2::splat(self.side),
        )
    }
}

/// What the tiles are rendered from: the decoded photo and its edit, `None`
/// while no photo is decoded.
pub(in crate::app) fn thumbnail_inputs<'a>(
    document: &'a crate::app::develop::state::Document,
    library: &'a Arc<crate::presets::Library>,
    lensfun: &'a crate::lens::lensfun::installed::Handle,
    monitor: Option<std::path::PathBuf>,
) -> Option<thumbnails::Inputs<'a>> {
    Some(thumbnails::Inputs {
        image: document.full()?,
        recipe: document.edit.recipe(),
        metadata: document.metadata.as_ref()?,
        profiles: &document.profiles,
        lensfun,
        monitor,
        library,
        photo: (document.path.clone(), document.catalog_photo),
    })
}

/// What is known of a preset for its row or tile.
struct PresetFacts<'a> {
    p: &'a crate::xmp::Preset,
    i: usize,
    issue: Option<&'a String>,
    substitute: Option<&'a (String, String)>,
    favorite: bool,
    /// A photo is decoded to apply it to.
    enabled: bool,
    user: &'a crate::presets::user::UserPresets,
    /// Drawn as a tile, whose star sits on the photo.
    tile: bool,
    /// Why its tile could not be rendered.
    failed: Option<&'a str>,
}

/// What a preset's row or tile was asked this frame.
#[derive(Default)]
struct Item {
    clicked: bool,
    hovered: bool,
    /// Its favorite star was clicked.
    toggled: bool,
    action: Option<crate::app::presets::user_presets::PresetAction>,
}

/// A preset's row or tile: its favorite star at `star`, shown on hover or when
/// set, the click that applies it, its menu, and its tooltip.
fn preset_item(
    ui: &mut egui::Ui,
    response: egui::Response,
    star: egui::Rect,
    f: PresetFacts,
) -> Item {
    let palette = theme::palette(ui.ctx());
    let mut out = Item::default();
    let star_response = ui.interact(star, ui.id().with(("favorite", f.i)), Sense::click());
    if f.favorite || response.hovered() || star_response.hovered() {
        if f.tile {
            // A backdrop, so the star reads over any photo.
            ui.painter()
                .circle_filled(star.center(), 9., egui::Color32::from_black_alpha(140));
        }
        ui.painter().text(
            star.center(),
            egui::Align2::CENTER_CENTER,
            "★",
            egui::FontId::proportional(text::LABEL),
            palette.tone(if star_response.hovered() {
                Tone::STRONG
            } else if f.favorite {
                Tone::SOFT
            } else if f.tile {
                Tone::MUTED
            } else {
                Tone::DISABLED
            }),
        );
    }
    let star_response = crate::app::ui::widgets::named(star_response, favorite_text(f.favorite));
    if star_response
        .on_hover_text(favorite_text(f.favorite))
        .clicked()
    {
        out.toggled = true;
        return out;
    }
    // Control-click opens the menu on macOS, as a right-click does.
    if f.enabled && response.clicked() && !crate::app::ui::widgets::context_clicked(&response) {
        out.clicked = true;
    }
    // Presets made here can be changed, as Lightroom's own; imported ones renamed
    // or deleted, and any preset file shown where it is.
    if !f.p.builtin {
        crate::app::ui::widgets::context_menu(&response, |ui| {
            if let Some(action) =
                crate::app::presets::user_presets::preset_menu(ui, f.user, f.p, f.i)
            {
                out.action = Some(action);
            }
        });
    }
    if response.hovered() && f.enabled {
        out.hovered = true;
    }
    // Built only while it shows.
    response.on_hover_ui(|ui| {
        let mut detail = preset_detail(f.p, f.issue, f.substitute);
        if let Some(why) = f.failed {
            detail.push_str(&format!("\nThumbnail not rendered: {why}"));
        }
        ui.label(detail);
    });
    out
}

fn favorite_text(favorite: bool) -> &'static str {
    if favorite {
        "Remove from favorites"
    } else {
        "Add to favorites"
    }
}

/// How a tile is drawn besides its picture.
struct TileLook<'a> {
    name: &'a str,
    /// Applies in full to a decoded photo; otherwise dimmed, as the list grays it.
    enabled: bool,
    selected: bool,
    hovered: bool,
}

/// Height of the strip a tile's name sits on.
const NAME_STRIP: f32 = 20.;

/// A preset's tile: the photo with it applied, cut square from the middle, or a
/// placeholder until it lands (with a warning mark when it failed), the name on a
/// strip over its bottom, dimmed when it doesn't apply in full, and outlined when
/// selected or hovered.
fn paint_tile(ui: &egui::Ui, rect: egui::Rect, shown: &thumbnails::Tile, look: TileLook) {
    use egui::{Color32, CornerRadius, Stroke, StrokeKind};
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter_at(rect);
    let corners = CornerRadius::same(radius::CONTROL as u8);
    match shown {
        thumbnails::Tile::Picture(texture) => {
            egui::Image::new((texture.id(), rect.size()))
                .corner_radius(corners)
                .paint_at(ui, rect);
        }
        thumbnails::Tile::Missing | thumbnails::Tile::Failed(_) => {
            painter.rect_filled(rect, corners, palette.tone(Tone::CARD));
        }
    }
    let strip = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - NAME_STRIP),
        rect.max,
    );
    if let thumbnails::Tile::Failed(_) = shown {
        crate::app::ui::icons::paint_at(
            &painter,
            crate::app::ui::icons::Icon::Failed,
            egui::pos2(rect.center().x, (rect.top() + strip.top()) / 2.),
            18.,
            palette.warning(),
        );
    }
    if !look.enabled {
        painter.rect_filled(rect, corners, Color32::from_black_alpha(120));
    }
    painter.rect_filled(
        strip,
        CornerRadius {
            sw: corners.sw,
            se: corners.se,
            ..Default::default()
        },
        Color32::from_black_alpha(150),
    );
    let galley = egui::WidgetText::from(look.name).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (strip.width() - 2. * space::S).max(1.),
        egui::FontId::proportional(text::SMALL),
    );
    painter.galley(
        egui::pos2(
            strip.left() + space::S,
            strip.center().y - galley.size().y / 2.,
        ),
        galley,
        palette.tone(match (look.enabled, look.hovered || look.selected) {
            (false, _) => Tone::DISABLED,
            (true, true) => Tone::STRONG,
            (true, false) => Tone::SOFT,
        }),
    );
    if look.selected {
        painter.rect_stroke(
            rect,
            corners,
            Stroke::new(2., palette.tone(Tone::STRONG)),
            StrokeKind::Inside,
        );
    } else if look.hovered {
        painter.rect_stroke(
            rect,
            corners,
            Stroke::new(1., palette.tone(Tone::SOFT)),
            StrokeKind::Inside,
        );
    }
}

/// A preset's tooltip: its name, whether it fits this photo, its notes and
/// the profile it renders with in place of a missing one.
fn preset_detail(
    p: &crate::xmp::Preset,
    issue: Option<&String>,
    substitute: Option<&(String, String)>,
) -> String {
    let mut lines = vec![p.name.clone()];
    if let Some(issue) = issue {
        lines.push(format!("Not fully compatible: {issue}"));
        lines.push("Only the supported settings apply".into());
    }
    lines.extend(p.notes.iter().filter(|n| !n.is_empty()).cloned());
    if let Some((asked, used)) = substitute {
        lines.push(format!(
            "Made for {asked}; renders with {used} because {asked} isn't imported for this camera"
        ));
    }
    lines.join("\n")
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
    fn a_preset_tooltip_says_only_what_the_row_does_not() {
        let mut p = crate::xmp::Preset {
            id: "warm".into(),
            name: "Warm".into(),
            group: "Color".into(),
            path: Default::default(),
            settings: Default::default(),
            curves: Default::default(),
            look: String::new(),
            blockers: Vec::new(),
            notes: Vec::new(),
            photo_settings: false,
            local: Default::default(),
            builtin: false,
        };
        assert_eq!(preset_detail(&p, None, None), "Warm");
        p.notes = vec!["Uses its own curve".into()];
        let issue = "The profile Velvia is for another camera".to_string();
        let substitute = ("Velvia".to_string(), "Camera Look".to_string());
        assert_eq!(
            preset_detail(&p, Some(&issue), Some(&substitute)),
            "Warm\n\
             Not fully compatible: The profile Velvia is for another camera\n\
             Only the supported settings apply\n\
             Uses its own curve\n\
             Made for Velvia; renders with Camera Look because Velvia isn't imported for this camera"
        );
    }

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
#[cfg(test)]
mod thumbnails_editor_tests;

pub(in crate::app) mod bulk_import;
pub(in crate::app) mod lut_import;
pub(in crate::app) mod manager;
pub(in crate::app) mod profile_manager;
pub(in crate::app) mod thumbnails;
pub(in crate::app) mod user_presets;
