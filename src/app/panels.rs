//! Which of a module's panels show, as Lightroom's Tab, Shift+Tab and F6–F8
//! hide them: each module keeps its own, and the session keeps both across
//! launches. Hiding a panel changes only whether it is drawn, never what it holds.
use super::Editor;
use crate::app::Module;
use eframe::egui::{self, Key, Modifiers};
use serde::{Deserialize, Deserializer, Serialize};

/// A panel the user can hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspacePanel {
    /// The catalog, folders and collections, in both modules.
    Left,
    /// The Navigator, over Develop's adjustments or the Library's photo info
    /// and metadata; in Develop also the tab panel and its rail.
    Right,
    /// The filmstrip with the status bar above it.
    Filmstrip,
    /// Develop's Presets and History tabs, beside the adjustments.
    Presets,
    /// Develop's adjustments, which the rail's Edit folds; a tool's drawer
    /// shows in their place either way.
    Adjustments,
}
impl WorkspacePanel {
    const ALL: [Self; 5] = [
        Self::Left,
        Self::Right,
        Self::Filmstrip,
        Self::Presets,
        Self::Adjustments,
    ];
    /// The side panels `module` has, which Tab hides and shows together.
    fn sides(module: Module) -> &'static [Self] {
        match module {
            // The adjustments' fold is the rail's, under the right panel.
            Module::Develop => &[Self::Left, Self::Right, Self::Presets],
            Module::Library => &[Self::Left, Self::Right],
        }
    }
}

/// How a request changes a module's panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelChange {
    /// One panel (F7, F8, F6, Shift+F8, its bar button, or Develop's tab rail).
    Toggle(WorkspacePanel),
    /// Both side panels (Tab).
    Sides,
    /// The side panels and the filmstrip (Shift+Tab).
    All,
}

/// Whether a panel is drawn. Saved by name; any other value reads as shown, so
/// a session from another version never hides a panel by mistake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "String")]
pub(crate) enum Visibility {
    #[default]
    Shown,
    Hidden,
}
impl From<String> for Visibility {
    fn from(name: String) -> Self {
        if name == "hidden" {
            Self::Hidden
        } else {
            Self::Shown
        }
    }
}
impl Visibility {
    fn flipped(self) -> Self {
        match self {
            Self::Shown => Self::Hidden,
            Self::Hidden => Self::Shown,
        }
    }
}

/// One module's panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PanelLayout {
    pub left: Visibility,
    pub right: Visibility,
    pub filmstrip: Visibility,
    /// Develop's tab panel; the Library has none.
    pub presets: Visibility,
    /// Develop's adjustments; the Library has none.
    pub adjustments: Visibility,
}
impl PanelLayout {
    pub(super) fn shown(self, panel: WorkspacePanel) -> bool {
        self.visibility(panel) == Visibility::Shown
    }
    fn visibility(self, panel: WorkspacePanel) -> Visibility {
        match panel {
            WorkspacePanel::Left => self.left,
            WorkspacePanel::Right => self.right,
            WorkspacePanel::Filmstrip => self.filmstrip,
            WorkspacePanel::Presets => self.presets,
            WorkspacePanel::Adjustments => self.adjustments,
        }
    }
    fn slot(&mut self, panel: WorkspacePanel) -> &mut Visibility {
        match panel {
            WorkspacePanel::Left => &mut self.left,
            WorkspacePanel::Right => &mut self.right,
            WorkspacePanel::Filmstrip => &mut self.filmstrip,
            WorkspacePanel::Presets => &mut self.presets,
            WorkspacePanel::Adjustments => &mut self.adjustments,
        }
    }
    /// `module`'s layout after `change`. A group hides when any of it shows,
    /// and shows again only once all of it is hidden, as Lightroom's Tab does.
    pub(super) fn changed(self, change: PanelChange, module: Module) -> Self {
        let sides = WorkspacePanel::sides(module);
        let group: Vec<WorkspacePanel> = match change {
            PanelChange::Toggle(panel) => {
                let mut layout = self;
                *layout.slot(panel) = self.visibility(panel).flipped();
                return layout;
            }
            PanelChange::Sides => sides.to_vec(),
            PanelChange::All => [sides, &[WorkspacePanel::Filmstrip]].concat(),
        };
        let to = if group.iter().any(|panel| self.shown(*panel)) {
            Visibility::Hidden
        } else {
            Visibility::Shown
        };
        let mut layout = self;
        for panel in group {
            *layout.slot(panel) = to;
        }
        layout
    }
    /// The layout after `change`, and what it hid for the next Tab or
    /// Shift+Tab to bring back. Given what the last one hid, the same key
    /// again brings that layout back, unless the panels changed since.
    pub(super) fn changed_from(
        self,
        change: PanelChange,
        last: Option<Tabbed>,
        module: Module,
    ) -> (Self, Option<Tabbed>) {
        if let Some(last) = last
            && last.change == change
            && last.after == self
        {
            return (last.before, None);
        }
        let next = self.changed(change, module);
        let hid =
            !matches!(change, PanelChange::Toggle(_)) && self.hidden_by(next).next().is_some();
        (
            next,
            hid.then_some(Tabbed {
                change,
                before: self,
                after: next,
            }),
        )
    }
    /// The panels shown here and hidden in `next`.
    pub(super) fn hidden_by(self, next: Self) -> impl Iterator<Item = WorkspacePanel> {
        WorkspacePanel::ALL
            .into_iter()
            .filter(move |panel| self.shown(*panel) && !next.shown(*panel))
    }
}

/// What a Tab or Shift+Tab hid: the layout before, to bring back, and the
/// layout it left, which must still stand for that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tabbed {
    change: PanelChange,
    before: PanelLayout,
    after: PanelLayout,
}

/// The tab Develop's tab panel shows. Saved by name; any other value reads as
/// Presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "String")]
pub(crate) enum DevelopTab {
    #[default]
    Presets,
    /// Snapshots over History, as in Lightroom.
    History,
}
impl From<String> for DevelopTab {
    fn from(name: String) -> Self {
        if name == "history" {
            Self::History
        } else {
            Self::Presets
        }
    }
}

/// Develop's panels and the Library's, as the session keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspacePanels {
    pub develop: PanelLayout,
    pub library: PanelLayout,
    pub develop_tab: DevelopTab,
}
impl WorkspacePanels {
    pub(super) fn of(self, module: Module) -> PanelLayout {
        match module {
            Module::Develop => self.develop,
            Module::Library => self.library,
        }
    }
    pub(super) fn set(&mut self, module: Module, layout: PanelLayout) {
        match module {
            Module::Develop => self.develop = layout,
            Module::Library => self.library = layout,
        }
    }
}

impl Editor {
    /// Whether the module open shows `panel`. The whole-window mode (see
    /// `chrome`) hides every panel without changing the layout kept for it.
    pub(super) fn panel_shown(&self, panel: WorkspacePanel) -> bool {
        self.chrome.shown(self.module) && self.panels.of(self.module).shown(panel)
    }
    /// Changes the open module's panels. A panel being hidden first lets go of
    /// what it was doing: the Library's info panel saves a field still being
    /// typed, and stays if that fails (the status line says why); Develop's
    /// presets panel ends a hover preview and keeps a snapshot name being typed.
    /// Returns whether the panels changed.
    pub(super) fn change_panels(&mut self, change: PanelChange) -> bool {
        let layout = self.panels.of(self.module);
        let last = self
            .tabbed
            .filter(|(module, _)| *module == self.module)
            .map(|(_, tabbed)| tabbed);
        let (next, tabbed) = layout.changed_from(change, last, self.module);
        for panel in layout.hidden_by(next) {
            match (self.module, panel) {
                (Module::Library, WorkspacePanel::Right) => {
                    if !self.commit_library_drafts() {
                        return false;
                    }
                }
                (Module::Develop, WorkspacePanel::Presets) => {
                    self.end_preset_hover();
                    self.commit_snapshot_rename();
                }
                _ => {}
            }
        }
        self.panels.set(self.module, next);
        self.tabbed = tabbed.map(|tabbed| (self.module, tabbed));
        self.save_panels();
        true
    }
    /// A panel change asked for by a key, a command or a device: as
    /// [`Self::change_panels`], except that Develop's tab panel opens and
    /// folds as its rail does, in a narrow window too. Returns whether the
    /// panels changed.
    pub(super) fn request_panels(&mut self, change: PanelChange) -> bool {
        if change != PanelChange::Toggle(WorkspacePanel::Presets) {
            return self.change_panels(change);
        }
        if self.module == Module::Develop {
            return self.toggle_develop_tabs();
        }
        // The Library has no tab panel: Develop's folds or opens, as it shows
        // there next.
        self.panels.develop = self.panels.develop.changed(change, Module::Develop);
        self.save_panels();
        true
    }
    /// Switches Develop's tab panel to `tab`. The tab left first lets go of
    /// what it was doing, as hiding the panel does.
    pub(super) fn set_develop_tab(&mut self, tab: DevelopTab) {
        if self.panels.develop_tab == tab {
            return;
        }
        match self.panels.develop_tab {
            DevelopTab::Presets => self.end_preset_hover(),
            DevelopTab::History => self.commit_snapshot_rename(),
        }
        self.panels.develop_tab = tab;
        self.save_panels();
    }
    fn save_panels(&mut self) {
        if let Err(e) = self.save_session() {
            self.status = format!("Panel layout not saved: {e:#}");
        }
    }
    /// Tab, Shift+Tab, F6–F8 and Shift+F8 (Develop's tab panel), unless a field or control has the keyboard
    /// focus (Tab then moves it on) or a menu is open. Run before any panel is
    /// drawn, so the Tab taken here does not also focus the first slider.
    pub(super) fn panel_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) || egui::Popup::is_any_open(ctx) {
            return;
        }
        let change = ctx.input_mut(|i| {
            // Shift+Tab before Tab: the plain pattern also matches with Shift held.
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                Some(PanelChange::All)
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                Some(PanelChange::Sides)
            } else if i.consume_key(Modifiers::SHIFT, Key::F8) {
                // Before F8, whose plain pattern also matches with Shift held.
                Some(PanelChange::Toggle(WorkspacePanel::Presets))
            } else if i.consume_key(Modifiers::NONE, Key::F7) {
                Some(PanelChange::Toggle(WorkspacePanel::Left))
            } else if i.consume_key(Modifiers::NONE, Key::F8) {
                Some(PanelChange::Toggle(WorkspacePanel::Right))
            } else if i.consume_key(Modifiers::NONE, Key::F6) {
                Some(PanelChange::Toggle(WorkspacePanel::Filmstrip))
            } else {
                None
            }
        });
        if let Some(change) = change {
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            self.request_panels(change);
        }
    }
}

impl Editor {
    /// The workspace bar's button that hides or shows `panel`: the panel's
    /// icon while it shows, its "open" icon while hidden.
    pub(super) fn panel_toggle(&mut self, ui: &mut egui::Ui, panel: WorkspacePanel) {
        use super::icons::Icon;
        let shown = self.panel_shown(panel);
        let (icon, name, key) = match (panel, shown) {
            (WorkspacePanel::Left, true) => (Icon::PanelLeft, "the left panel", " · F7"),
            (WorkspacePanel::Left, false) => (Icon::PanelLeftOpen, "the left panel", " · F7"),
            (WorkspacePanel::Right, true) => (Icon::PanelRight, "the right panel", " · F8"),
            (WorkspacePanel::Right, false) => (Icon::PanelRightOpen, "the right panel", " · F8"),
            (WorkspacePanel::Filmstrip, true) => (Icon::PanelBottom, "the filmstrip", " · F6"),
            (WorkspacePanel::Filmstrip, false) => (Icon::PanelBottomOpen, "the filmstrip", " · F6"),
            (WorkspacePanel::Presets, true) => (Icon::PanelRight, "Presets and History", ""),
            (WorkspacePanel::Presets, false) => (Icon::PanelRightOpen, "Presets and History", ""),
            (WorkspacePanel::Adjustments, true) => (Icon::PanelRight, "the adjustments", ""),
            (WorkspacePanel::Adjustments, false) => (Icon::PanelRightOpen, "the adjustments", ""),
        };
        let verb = if shown { "Hide" } else { "Show" };
        let palette = super::theme::palette(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(26.), egui::Sense::hover());
        let response = ui.interact(rect, toggle_id(panel), egui::Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, 4., palette.gray(45));
        }
        let color = palette.gray(if response.hovered() { 235 } else { 160 });
        super::icons::paint_at(ui.painter(), icon, rect.center(), 15., color);
        if response
            .on_hover_text(format!("{verb} {name}{key}"))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            self.change_panels(PanelChange::Toggle(panel));
        }
    }
}
/// The side panels the user resizes by dragging their inner edge, by id.
const RESIZABLE: [&str; 4] = ["library-sidebar", "library-info", "presets", "adjustments"];
/// Shows the resize cursor over a side panel's edge, or while it is dragged.
/// The edge's grab area reaches over the Library's grid, whose cells are
/// drawn after the panel and so take the hover: egui then never shows its
/// cursor, though a drag there still resizes the panel.
pub(super) fn keep_resize_cursor(ctx: &egui::Context) {
    let dragged = ctx.dragged_id();
    let on_edge = RESIZABLE.iter().any(|panel| {
        ctx.read_response(egui::Id::new(*panel).with("__resize"))
            .is_some_and(|edge| {
                edge.dragged()
                    || (edge.contains_pointer() && dragged.is_none_or(|id| id == edge.id))
            })
    });
    if on_edge {
        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
}
/// The workspace bar's toggle for `panel`, by a fixed id.
pub(super) fn toggle_id(panel: WorkspacePanel) -> egui::Id {
    egui::Id::new(("panel-toggle", panel as u8))
}

/// The session's panels, or all shown if what was saved does not read.
pub(crate) fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<WorkspacePanels, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDDEN: Visibility = Visibility::Hidden;
    const SHOWN: Visibility = Visibility::Shown;

    fn layout(left: Visibility, right: Visibility, filmstrip: Visibility) -> PanelLayout {
        PanelLayout {
            left,
            right,
            filmstrip,
            presets: SHOWN,
            adjustments: SHOWN,
        }
    }

    #[test]
    fn tab_hides_both_sides_while_either_shows_and_brings_both_back() {
        let all = PanelLayout::default();
        let sides_hidden = all.changed(PanelChange::Sides, Module::Library);
        assert_eq!(sides_hidden, layout(HIDDEN, HIDDEN, SHOWN));
        assert_eq!(
            sides_hidden.changed(PanelChange::Sides, Module::Library),
            all
        );
        // One side already hidden: Tab hides the other first.
        let left_only = layout(SHOWN, HIDDEN, SHOWN);
        assert_eq!(
            left_only.changed(PanelChange::Sides, Module::Library),
            sides_hidden
        );
    }

    #[test]
    fn a_second_tab_brings_back_what_the_first_hid() {
        // The left panel hidden with its button; Tab hides the right one.
        let right_only = layout(HIDDEN, SHOWN, SHOWN);
        let (hidden, tabbed) = right_only.changed_from(PanelChange::Sides, None, Module::Library);
        assert_eq!(hidden, layout(HIDDEN, HIDDEN, SHOWN));
        // Tab again: the right panel alone comes back.
        let (back, tabbed) = hidden.changed_from(PanelChange::Sides, tabbed, Module::Library);
        assert_eq!(back, right_only);
        assert_eq!(tabbed, None);

        // Shift+Tab restores the layout it hid, filmstrip and all.
        let (none, tabbed) = right_only.changed_from(PanelChange::All, None, Module::Library);
        assert_eq!(none, layout(HIDDEN, HIDDEN, HIDDEN));
        assert_eq!(
            none.changed_from(PanelChange::All, tabbed, Module::Library)
                .0,
            right_only
        );

        // A panel changed in between: Tab starts from the panels as they are.
        let (hidden, tabbed) = right_only.changed_from(PanelChange::Sides, None, Module::Library);
        let (filmstrip_gone, tabbed) = hidden.changed_from(
            PanelChange::Toggle(WorkspacePanel::Filmstrip),
            tabbed,
            Module::Library,
        );
        assert_eq!(tabbed, None);
        assert_eq!(
            filmstrip_gone
                .changed_from(PanelChange::Sides, tabbed, Module::Library)
                .0,
            layout(SHOWN, SHOWN, HIDDEN)
        );
        // Shift+Tab after Tab is its own change, not Tab's undo.
        let (sides, tabbed) = right_only.changed_from(PanelChange::Sides, None, Module::Library);
        assert_eq!(
            sides
                .changed_from(PanelChange::All, tabbed, Module::Library)
                .0,
            layout(HIDDEN, HIDDEN, HIDDEN)
        );
    }

    #[test]
    fn shift_tab_takes_the_filmstrip_with_the_sides() {
        let all = PanelLayout::default();
        let none = all.changed(PanelChange::All, Module::Library);
        assert_eq!(none, layout(HIDDEN, HIDDEN, HIDDEN));
        assert_eq!(none.changed(PanelChange::All, Module::Library), all);
        // Tab's hidden sides with the filmstrip still up: Shift+Tab hides it too.
        assert_eq!(
            layout(HIDDEN, HIDDEN, SHOWN).changed(PanelChange::All, Module::Library),
            none
        );
    }

    #[test]
    fn a_single_toggle_changes_only_its_panel() {
        let all = PanelLayout::default();
        let right = all.changed(PanelChange::Toggle(WorkspacePanel::Right), Module::Library);
        assert_eq!(right, layout(SHOWN, HIDDEN, SHOWN));
        assert_eq!(
            all.hidden_by(right).collect::<Vec<_>>(),
            [WorkspacePanel::Right]
        );
        assert_eq!(
            right.changed(PanelChange::Toggle(WorkspacePanel::Right), Module::Library),
            all
        );
    }

    #[test]
    fn develops_tab_goes_with_its_sides() {
        let all = PanelLayout::default();
        let sides = all.changed(PanelChange::Sides, Module::Develop);
        assert_eq!(
            sides,
            PanelLayout {
                presets: HIDDEN,
                ..layout(HIDDEN, HIDDEN, SHOWN)
            }
        );
        assert_eq!(sides.changed(PanelChange::Sides, Module::Develop), all);
        // The tab panel folded alone: Tab hides the rest, then brings all back.
        let folded = all.changed(
            PanelChange::Toggle(WorkspacePanel::Presets),
            Module::Develop,
        );
        assert_eq!(folded.changed(PanelChange::Sides, Module::Develop), sides);
        // The Library has no tab panel: it never keeps Tab from showing its sides.
        let library_sides = all.changed(PanelChange::Sides, Module::Library);
        assert_eq!(
            library_sides.changed(PanelChange::Sides, Module::Library),
            all
        );
    }

    #[test]
    fn saved_panels_read_back_and_unknown_values_show() {
        let panels = WorkspacePanels {
            develop: layout(HIDDEN, SHOWN, HIDDEN),
            library: PanelLayout::default(),
            develop_tab: DevelopTab::History,
        };
        let json = serde_json::to_value(panels).unwrap();
        assert_eq!(json["develop"]["left"], "hidden");
        assert_eq!(json["develop_tab"], "history");
        assert_eq!(
            serde_json::from_value::<WorkspacePanels>(json).unwrap(),
            panels
        );
        let odd: WorkspacePanels = serde_json::from_str(
            r#"{"develop":{"left":"folded","right":"hidden","extra":1},"future":{},"develop_tab":"ai"}"#,
        )
        .unwrap();
        assert_eq!(odd.develop, layout(SHOWN, HIDDEN, SHOWN));
        assert_eq!(odd.library, PanelLayout::default());
        assert_eq!(odd.develop_tab, DevelopTab::Presets);
        // Sessions from before the tab panel show it.
        assert!(odd.develop.shown(WorkspacePanel::Presets));
    }
}
