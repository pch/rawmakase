//! Develop's tab panel beside the adjustments: Presets, and Snapshots over
//! History, one at a time. A slim rail at the window's right edge first
//! switches the adjustments panel between Edit and the Crop, Remove, Red Eye
//! and Masking drawers, then, under them, switches the tab; the open tab's
//! button folds the panel and any brings it back. A window too narrow for it
//! and the photo shows it folded until a tab is asked for.
use crate::app::Editor;
use crate::app::develop::panels::inspector::{DRAWER_TOOLS, has_drawer, tool_icon};
use crate::app::develop::state::Tool;
use crate::app::shell::panels::{DevelopTab, PanelChange, WorkspacePanel};
use crate::app::ui::design::{Tone, radius, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{SectionGroup, SectionSide};
use eframe::egui::{self, Pos2, Rect, Sense, Vec2};

/// The rail's width.
const RAIL_WIDTH: f32 = 36.;
/// The tab panel's width until the user drags it.
const PANEL_WIDTH: f32 = 245.;
/// The photo's narrowest width before the tab panel folds by itself.
const PHOTO_MIN: f32 = 400.;
/// The tabs, top to bottom on the rail, with their icons and names.
const TABS: [(DevelopTab, Icon, &str); 2] = [
    (DevelopTab::Presets, Icon::Presets, "Presets"),
    (DevelopTab::History, Icon::History, "History"),
];

/// What a click on the rail asked for.
enum TabClick {
    /// Show this tab, opening the panel.
    Open(DevelopTab),
    Fold,
    /// Show the adjustments, closing the open tool's drawer.
    Edit,
    /// Open or close this tool, its drawer in place of the adjustments.
    Tool(Tool),
}

/// A rail button's id, fixed so tests can find it.
pub(in crate::app) fn rail_id(tab: DevelopTab) -> egui::Id {
    egui::Id::new(("develop-rail", tab))
}
/// The rail's Edit button's id.
pub(in crate::app) fn edit_id() -> egui::Id {
    egui::Id::new("develop-rail-edit")
}
/// A tool's rail button's id.
pub(in crate::app) fn tool_id(tool: Tool) -> egui::Id {
    egui::Id::new(("develop-rail-tool", format!("{tool:?}")))
}
/// Whether a tab was asked for while the window had no room for the panel.
fn peek_id() -> egui::Id {
    egui::Id::new("develop-tabs-peek")
}
/// Whether the window had room for the panel beside the photo last frame.
fn room_id() -> egui::Id {
    egui::Id::new("develop-tabs-room")
}

impl Editor {
    /// The rail, drawn first so it takes the window's right edge, outside
    /// the adjustments. The right panel's toggle (F8) hides it, the tab panel
    /// and the adjustments together; the rail folds the adjustments or the
    /// tab panel alone.
    /// Returns whether the tab panel opens, for [`Self::develop_tab_panel`]
    /// to draw once the adjustments are.
    pub(in crate::app) fn develop_rail_edge(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        if !self.panel_shown(WorkspacePanel::Right) {
            return false;
        }
        let width = |id: &str, default: f32| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id))
                .map_or(default, |state| state.outer_rect.width())
        };
        let adjustments = if self.develop_adjustments_shown() {
            width(
                "adjustments",
                crate::app::shell::workspace::ADJUSTMENTS_DEFAULT,
            )
        } else {
            0.
        };
        let room = ui.available_width() - RAIL_WIDTH - adjustments - width("presets", PANEL_WIDTH)
            >= PHOTO_MIN;
        ctx.data_mut(|d| d.insert_temp(room_id(), room));
        let click = self.develop_rail(ui, self.develop_tabs_open());
        match click {
            Some(TabClick::Open(tab)) => {
                self.set_develop_tab(tab);
                self.open_develop_tabs();
            }
            Some(TabClick::Fold) => {
                self.fold_develop_tabs();
            }
            Some(TabClick::Edit) => {
                // From a tool, back to the adjustments; on them, folds them.
                if has_drawer(self.view.tool) {
                    self.view.toggle(self.view.tool);
                    if !self.panel_shown(WorkspacePanel::Adjustments) {
                        self.change_panels(PanelChange::Toggle(WorkspacePanel::Adjustments));
                    }
                } else {
                    self.change_panels(PanelChange::Toggle(WorkspacePanel::Adjustments));
                }
            }
            Some(TabClick::Tool(tool)) => self.view.toggle(tool),
            None => {}
        }
        self.develop_tabs_open()
    }
    /// Whether the adjustments panel shows, under a shown right panel: the
    /// adjustments unless Edit folded them, or the open tool's drawer.
    pub(in crate::app) fn develop_adjustments_shown(&self) -> bool {
        self.panel_shown(WorkspacePanel::Adjustments) || has_drawer(self.view.tool)
    }
    /// Whether the tab panel shows its tab: not folded, and with room beside
    /// the photo as last laid out, or asked for in a narrow window.
    fn develop_tabs_open(&self) -> bool {
        let (room, peek) = self.context.data(|d| {
            (
                d.get_temp::<bool>(room_id()).unwrap_or(true),
                d.get_temp::<bool>(peek_id()).unwrap_or(false),
            )
        });
        self.panel_shown(WorkspacePanel::Presets) && (room || peek)
    }
    /// Opens the tab panel on its tab, in a narrow window too; false if the
    /// layout could not be saved.
    fn open_develop_tabs(&mut self) -> bool {
        let room = self
            .context
            .data(|d| d.get_temp::<bool>(room_id()).unwrap_or(true));
        let shown = self.panel_shown(WorkspacePanel::Presets)
            || self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets));
        self.context.data_mut(|d| d.insert_temp(peek_id(), !room));
        shown
    }
    /// Folds the tab panel. One asked for in a narrow window folds back to
    /// fit and stays unfolded for a wider one; false if the layout could not
    /// be saved.
    fn fold_develop_tabs(&mut self) -> bool {
        let (room, peek) = self.context.data(|d| {
            (
                d.get_temp::<bool>(room_id()).unwrap_or(true),
                d.get_temp::<bool>(peek_id()).unwrap_or(false),
            )
        });
        if peek && !room {
            self.context.data_mut(|d| d.insert_temp(peek_id(), false));
            true
        } else {
            self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets))
        }
    }
    /// Folds the tab panel if its tab shows, or opens it, as its open tab's
    /// rail button and the others do: for `panels:presets`. False if the
    /// layout could not be saved.
    pub(in crate::app) fn toggle_develop_tabs(&mut self) -> bool {
        if self.develop_tabs_open() {
            self.fold_develop_tabs()
        } else {
            self.open_develop_tabs()
        }
    }
    /// The tab panel between the adjustments and the photo, if `open`.
    pub(in crate::app) fn develop_tab_panel(&mut self, ui: &mut egui::Ui, open: bool) {
        // A tab not drawn this frame (folded or too narrow) lets go of what
        // it was doing: Presets ends a preview it was showing, History saves
        // a snapshot name being typed.
        let drawn = open.then_some(self.panels.develop_tab);
        if drawn != Some(DevelopTab::Presets) && self.presets.hover.is_some() {
            self.end_preset_hover();
        }
        if drawn != Some(DevelopTab::History) {
            self.commit_snapshot_rename();
        }
        if open {
            self.tab_panel(ui);
        }
    }
    fn tab_panel(&mut self, ui: &mut egui::Ui) {
        let fill = theme::palette(ui.ctx()).tone(Tone::WINDOW);
        egui::Panel::right("presets")
            .default_size(PANEL_WIDTH)
            .min_size(crate::app::shell::workspace::PRESETS_MIN)
            .max_size(400.)
            .frame(egui::Frame::side_top_panel(ui.style()).fill(fill))
            .show(ui, |ui| {
                let _side = SectionSide::enter(ui, SectionGroup::DevelopLeft);
                match self.panels.develop_tab {
                    DevelopTab::Presets => self.presets_ui(ui),
                    DevelopTab::History => self.history_tab(ui),
                }
            });
    }
    fn develop_rail(&mut self, ui: &mut egui::Ui, open: bool) -> Option<TabClick> {
        let palette = theme::palette(ui.ctx());
        let mut click = None;
        let tool = self.view.tool;
        let photo = self.document.full().is_some() && !self.view.compare.before_only();
        egui::Panel::right("develop-rail")
            .resizable(false)
            .exact_size(RAIL_WIDTH)
            .frame(egui::Frame::new().fill(palette.tone(Tone::WINDOW)))
            .show(ui, |ui| {
                let rail = ui.max_rect();
                ui.allocate_rect(rail, Sense::hover());
                // A rail button: its fill, the open one's marker on the
                // window's edge, and whether it was clicked.
                let button =
                    |ui: &mut egui::Ui,
                     y: f32,
                     id: egui::Id,
                     active: bool,
                     enabled: bool,
                     tip: String,
                     paint: &dyn Fn(&egui::Painter, Pos2, egui::Color32)| {
                        let center = Pos2::new(rail.center().x, rail.top() + y);
                        let rect = Rect::from_center_size(center, Vec2::splat(28.));
                        let sense = if enabled {
                            Sense::click()
                        } else {
                            Sense::hover()
                        };
                        let response = ui
                            .interact(rect, id, sense)
                            .on_hover_text(tip)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        let hovered = enabled && response.hovered();
                        if active || hovered {
                            let fill =
                                palette.tone(if active { Tone::OUTLINE } else { Tone::RAISED });
                            ui.painter().rect_filled(rect, radius::CARD, fill);
                        }
                        if active {
                            ui.painter().rect_filled(
                                Rect::from_min_size(
                                    Pos2::new(rail.right() - 2., rect.top() + 7.),
                                    Vec2::new(2., rect.height() - 14.),
                                ),
                                1.,
                                palette.accent(),
                            );
                        }
                        let tint = palette.tone(match (enabled, active, hovered) {
                            (false, ..) => Tone::OUTLINE_STRONG,
                            (_, true, _) => Tone::BRIGHT,
                            (_, _, true) => Tone::TEXT,
                            _ => Tone::MUTED,
                        });
                        paint(ui.painter(), center, tint);
                        response.clicked()
                    };
                // Edit and the tools first, each in the adjustments' place.
                let mut y = 24.;
                let edit = |p: &egui::Painter, c: Pos2, tint: egui::Color32| {
                    icons::paint_at(p, Icon::Edit, c, 16., tint)
                };
                let editing = !has_drawer(tool) && self.panel_shown(WorkspacePanel::Adjustments);
                let tip = if editing { "Hide Edit" } else { "Show Edit" };
                if button(ui, y, edit_id(), editing, true, tip.into(), &edit) {
                    click = Some(TabClick::Edit);
                }
                y += 36.;
                for (t, tip, _) in DRAWER_TOOLS {
                    let paint = |p: &egui::Painter, c: Pos2, tint: egui::Color32| {
                        icons::paint_at(p, tool_icon(t), c, 16., tint)
                    };
                    if button(ui, y, tool_id(t), tool == t, photo, tip.to_string(), &paint) {
                        click = Some(TabClick::Tool(t));
                    }
                    y += 36.;
                }
                // The tabs after a gap, in the panel beside the adjustments.
                y += 32.;
                for (tab, icon, name) in TABS {
                    let active = open && self.panels.develop_tab == tab;
                    let tip = if active {
                        format!("Hide {name}")
                    } else {
                        format!("Show {name}")
                    };
                    let paint = |p: &egui::Painter, c: Pos2, tint: egui::Color32| {
                        icons::paint_at(p, icon, c, 16., tint)
                    };
                    if button(ui, y, rail_id(tab), active, true, tip, &paint) {
                        click = Some(if active {
                            TabClick::Fold
                        } else {
                            TabClick::Open(tab)
                        });
                    }
                    y += 36.;
                }
            });
        click
    }
    /// The History tab: the open photo's Snapshots over its History.
    fn history_tab(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() {
            ui.add_space(space::M);
            ui.label(
                egui::RichText::new("Open a photo to see its snapshots and history.")
                    .size(text::SMALL)
                    .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
            );
            return;
        }
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("history-list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.snapshots_section(ui);
                self.history_section(ui);
            });
    }
}
