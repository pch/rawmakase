use crate::app::develop::step_name::StepName;
use crate::app::ui::design::{self, Tone, button, inset, radius, size, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::develop::effects::EffectsRendering;
use crate::model::panels::PanelState;
use crate::model::params::{self, LocalParameterId, ParameterId, format_value};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

pub(in crate::app) fn toolbar_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12., 32.), Sense::hover());
    ui.painter().line_segment(
        [
            rect.center_top() + Vec2::new(0., 6.),
            rect.center_bottom() - Vec2::new(0., 6.),
        ],
        Stroke::new(1., theme::palette(ui.ctx()).tone(Tone::OUTLINE)),
    );
}
pub(in crate::app) fn toolbar_action(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    selected: bool,
    enabled: bool,
    icon: u8,
) -> egui::Response {
    toolbar_action_sized(ui, label, Vec2::new(width, 32.), selected, enabled, icon)
}
/// A toolbar button of `size`: Develop's are 32 points high, the Library's bars'
/// 22.
pub(in crate::app) fn toolbar_action_sized(
    ui: &mut egui::Ui,
    label: &str,
    size: Vec2,
    selected: bool,
    enabled: bool,
    icon: u8,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hover = enabled && response.hovered();
    let fill = palette.tone(if selected {
        Tone::HOVER
    } else if hover {
        Tone::OUTLINE
    } else {
        Tone::CARD
    });
    ui.painter().rect_filled(rect, radius::CONTROL, fill);
    let color = palette.tone(if !enabled {
        Tone::OUTLINE_STRONG
    } else if selected || hover {
        Tone::STRONG
    } else {
        Tone::LABEL
    });
    if !label.is_empty() {
        ui.painter().text(
            rect.center() + Vec2::new(if icon > 0 { 8. } else { 0. }, 0.),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(text::LABEL),
            color,
        );
    }
    let c = if label.is_empty() {
        rect.center()
    } else {
        Pos2::new(rect.left() + 13., rect.center().y)
    };
    let glyph = match icon {
        1 => Some(Icon::Undo),
        2 => Some(Icon::Redo),
        3 => Some(Icon::BeforeAfter),
        4 => Some(Icon::Search),
        5 => Some(Icon::Copy),
        6 => Some(Icon::Paste),
        7 => Some(Icon::Sync),
        _ => None,
    };
    if let Some(glyph) = glyph {
        icons::paint_at(ui.painter(), glyph, c, 15., color);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}
/// A resettable Develop panel; returns whether reset was clicked. A click names
/// its History step in `step`, before the contents are drawn with it.
pub(in crate::app) fn adjustment_section(
    ui: &mut egui::Ui,
    step: &mut StepName,
    title: &str,
    contents: impl FnOnce(&mut egui::Ui, &mut StepName),
) -> bool {
    section_header(ui, title, HeaderButton::Reset, None, step, contents)
}
/// Where the set of collapsed section titles lives in egui memory; the editor
/// seeds it from the session and saves it back when it changes.
pub(in crate::app) fn collapsed_sections_id() -> egui::Id {
    egui::Id::new("rawmakase-collapsed-sections")
}
/// The panel or sub-panel being drawn ("Detail", then "Sharpening"), so a
/// slider's step reads "Sharpening Amount" rather than "Amount".
pub(in crate::app) fn set_edit_context(ui: &egui::Ui, title: &str) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("rawmakase-edit-context"), title.to_string()));
}
fn edit_context(ui: &egui::Ui) -> String {
    ui.ctx()
        .data(|d| d.get_temp(egui::Id::new("rawmakase-edit-context")))
        .unwrap_or_default()
}
/// Collapsible Lightroom-style panel with no header button.
pub(in crate::app) fn section(
    ui: &mut egui::Ui,
    title: &str,
    contents: impl FnOnce(&mut egui::Ui),
) {
    section_with(ui, title, HeaderButton::None, contents);
}
/// The button at the right of a panel header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum HeaderButton {
    None,
    /// Resets the panel (and names the History step).
    Reset,
    /// Adds an item, as the Snapshots panel's +; says what it adds.
    Add(&'static str),
}
/// [`section`] with a header button other than Reset, which names a History
/// step ([`adjustment_section`]); returns whether it was clicked.
pub(in crate::app) fn section_with(
    ui: &mut egui::Ui,
    title: &str,
    button: HeaderButton,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    debug_assert!(button != HeaderButton::Reset, "Reset names a step");
    // Without Reset the header names no step.
    let mut unnamed = StepName::default();
    section_header(ui, title, button, None, &mut unnamed, |ui, _| contents(ui))
}
/// A resettable Develop panel with Lightroom's on/off switch in its header, over
/// the photo's `Enable*` setting; returns whether reset was clicked.
pub(in crate::app) fn switched_section(
    ui: &mut egui::Ui,
    step: &mut StepName,
    title: &str,
    switch: &mut PanelState,
    contents: impl FnOnce(&mut egui::Ui, &mut StepName),
) -> bool {
    section_header(ui, title, HeaderButton::Reset, Some(switch), step, contents)
}
/// The side of the window a panel is in. Solo Mode is per side, as in Lightroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SectionGroup {
    DevelopLeft,
    DevelopRight,
    LibraryLeft,
    LibraryRight,
}
impl SectionGroup {
    /// A stable name, as saved in the session.
    pub(in crate::app) fn key(self) -> &'static str {
        match self {
            SectionGroup::DevelopLeft => "develop-left",
            SectionGroup::DevelopRight => "develop-right",
            SectionGroup::LibraryLeft => "library-left",
            SectionGroup::LibraryRight => "library-right",
        }
    }
}
/// Where the sides in Solo Mode live in egui memory, by [`SectionGroup::key`]; the
/// editor seeds it from the session and saves it back when it changes.
pub(in crate::app) fn solo_sections_id() -> egui::Id {
    egui::Id::new("rawmakase-solo-sections")
}
fn section_group_id() -> egui::Id {
    egui::Id::new("rawmakase-section-group")
}
/// Every section title drawn so far on each side, so Solo Mode can close the others.
fn section_titles_id() -> egui::Id {
    egui::Id::new("rawmakase-section-titles")
}
type SectionTitles = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;
/// While alive, the sections drawn belong to one side of the window.
pub(in crate::app) struct SectionSide {
    ctx: egui::Context,
}
impl SectionSide {
    pub(in crate::app) fn enter(ui: &egui::Ui, group: SectionGroup) -> Self {
        ui.ctx()
            .data_mut(|d| d.insert_temp(section_group_id(), Some(group.key())));
        Self {
            ctx: ui.ctx().clone(),
        }
    }
}
impl Drop for SectionSide {
    fn drop(&mut self) {
        self.ctx
            .data_mut(|d| d.insert_temp::<Option<&'static str>>(section_group_id(), None));
    }
}
fn current_group(ui: &egui::Ui) -> Option<&'static str> {
    ui.ctx()
        .data(|d| d.get_temp::<Option<&'static str>>(section_group_id()))
        .flatten()
}
fn solo(ui: &egui::Ui, group: &str) -> bool {
    ui.ctx().data(|d| {
        d.get_temp::<std::collections::BTreeSet<String>>(solo_sections_id())
            .is_some_and(|set| set.contains(group))
    })
}
fn set_open(ui: &egui::Ui, title: &str, open: bool) {
    ui.ctx().data_mut(|d| {
        let set = d
            .get_temp_mut_or_default::<std::collections::BTreeSet<String>>(collapsed_sections_id());
        if open {
            set.remove(title);
        } else {
            set.insert(title.to_string());
        }
    });
}
/// Closes every other section on `group`'s side, as opening one in Solo Mode does.
fn close_others(ui: &egui::Ui, group: &str, title: &str) {
    let others: Vec<String> = ui.ctx().data(|d| {
        d.get_temp::<SectionTitles>(section_titles_id())
            .and_then(|titles| titles.get(group).cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|other| other != title)
            .collect()
    });
    for other in others {
        set_open(ui, &other, false);
    }
}
/// Turns Solo Mode on or off for `group`; turning it on leaves only `title` open,
/// if it is.
fn toggle_solo(ui: &egui::Ui, group: &str, title: &str) {
    let on = !solo(ui, group);
    ui.ctx().data_mut(|d| {
        let set =
            d.get_temp_mut_or_default::<std::collections::BTreeSet<String>>(solo_sections_id());
        if on {
            set.insert(group.to_string());
        } else {
            set.remove(group);
        }
    });
    if on {
        close_others(ui, group, title);
    }
}
/// The name a section's open state and Solo Mode go by. B&W replaces the Color
/// Mixer in the same place when a photo is black and white, as in Lightroom, so
/// they open and close as one panel.
fn section_key(title: &str) -> &str {
    match title {
        "B&W" => "Color Mixer",
        title => title,
    }
}
fn section_header(
    ui: &mut egui::Ui,
    title: &str,
    button: HeaderButton,
    switch: Option<&mut PanelState>,
    step: &mut StepName,
    contents: impl FnOnce(&mut egui::Ui, &mut StepName),
) -> bool {
    let palette = theme::palette(ui.ctx());
    let resettable = button != HeaderButton::None;
    let id = ui.make_persistent_id(("adjustment-section-v3", title));
    let key = section_key(title);
    let group = current_group(ui);
    if let Some(group) = group {
        ui.ctx().data_mut(|d| {
            d.get_temp_mut_or_default::<SectionTitles>(section_titles_id())
                .entry(group.to_string())
                .or_default()
                .insert(key.to_string());
        });
    }
    let mut open = !ui.ctx().data(|d| {
        d.get_temp::<std::collections::BTreeSet<String>>(collapsed_sections_id())
            .is_some_and(|set| set.contains(key))
    });
    let sidebar = group == Some(SectionGroup::LibraryLeft.key());
    // Contents, and the header's title and chevron over them, from the panel's
    // edges.
    let content_inset = if group == Some(SectionGroup::DevelopRight.key())
        || group == Some(SectionGroup::DevelopLeft.key())
        || group == Some(SectionGroup::LibraryRight.key())
    {
        inset::ADJUSTMENT_CONTENT
    } else {
        inset::SECTION_CONTENT
    };
    // Space parts the panels; the catalog's sidebar lists sources closer.
    ui.add_space(if sidebar { space::M } else { inset::SECTION });
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), size::HEADER),
        Sense::hover(),
    );
    // Outside the catalog's sidebar the chevron keeps the header's right end,
    // its buttons before it.
    let chevron_rect = (!sidebar).then(|| {
        Rect::from_center_size(
            // The 12 pt chevron ends where the contents do.
            Pos2::new(rect.right() - content_inset - 6., rect.center().y),
            Vec2::splat(24.),
        )
    });
    let end = chevron_rect.map_or(rect.right(), |c| c.left());
    let reset_rect =
        Rect::from_center_size(Pos2::new(end - 14., rect.center().y), Vec2::splat(24.));
    let right = if resettable { reset_rect.left() } else { end };
    let switch_rect =
        Rect::from_center_size(Pos2::new(right - 18., rect.center().y), Vec2::new(26., 20.));
    let toggle_right = if switch.is_some() {
        switch_rect.left()
    } else {
        right
    };
    let toggle_rect = Rect::from_min_max(rect.min, Pos2::new(toggle_right, rect.bottom()));
    let toggle = ui
        .interact(toggle_rect, id.with("toggle"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let chevron = chevron_rect.map(|c| {
        ui.interact(c, id.with("chevron"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    });
    let reset = ui.interact(
        reset_rect,
        id.with("reset"),
        if resettable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let reset = match button {
        HeaderButton::Reset => reset.on_hover_text(format!("Reset {title}")),
        HeaderButton::Add(what) => reset.on_hover_text(what),
        HeaderButton::None => reset,
    };
    let enabled = switch.as_deref().is_none_or(|s| *s == PanelState::On);
    let hovered = toggle.hovered() || chevron.as_ref().is_some_and(|c| c.hovered());
    // Every panel's header is a quiet caption with its chevron on the right.
    // In the catalog's sidebar both show only while hovered (the chevron also
    // while folded); elsewhere the header is a band with its chevron always.
    if !sidebar || hovered {
        let fill = match (sidebar, hovered) {
            (true, _) => Tone::CARD,
            (false, true) => Tone::RAISED,
            (false, false) => Tone::FIELD,
        };
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(fill));
    }
    ui.painter().text(
        // Over the sidebar rows' text at their 10 px inset; elsewhere over the
        // contents' labels.
        Pos2::new(
            rect.left() + if sidebar { 10. } else { content_inset },
            rect.center().y,
        ),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        egui::FontId::proportional(text::SMALL),
        palette.tone(if enabled { Tone::TEXT } else { Tone::DISABLED }),
    );
    if !sidebar || hovered || !open {
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        // Ending where the sidebar's counts do, or at the header's right end.
        let x = chevron_rect.map_or(right - 15.5, |c| c.center().x);
        let tint = palette.tone(if hovered { Tone::SOFT } else { Tone::DISABLED });
        icons::paint_at(
            ui.painter(),
            chevron,
            Pos2::new(x, rect.center().y),
            12.,
            tint,
        );
    }
    if resettable {
        let color = palette.tone(if reset.hovered() {
            Tone::STRONG
        } else {
            Tone::SECONDARY
        });
        let icon = if matches!(button, HeaderButton::Add(_)) {
            Icon::Add
        } else {
            Icon::Reset
        };
        icons::paint_at(ui.painter(), icon, reset_rect.center(), 12., color);
    }
    if let Some(state) = switch {
        let response = ui
            .interact(switch_rect, id.with("switch"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(match state {
                PanelState::On => format!("Turn {title} off"),
                PanelState::Off => format!("Turn {title} on"),
            });
        paint_switch(
            ui.painter(),
            switch_rect.center(),
            *state,
            response.hovered(),
        );
        if response.clicked() {
            *state = match state {
                PanelState::On => PanelState::Off,
                PanelState::Off => PanelState::On,
            };
        }
    }
    if let Some(group) = group {
        context_menu(&toggle, |ui| {
            // Menu rows fill the width they are given, and an unsized popup gives
            // them all it can.
            menu_width(ui, 160.);
            if menu_item(ui, "Solo Mode", "", true, solo(ui, group)) {
                toggle_solo(ui, group, key);
                ui.close();
            }
        });
    }
    if (toggle.clicked() && !context_clicked(&toggle))
        || chevron.as_ref().is_some_and(|c| c.clicked())
    {
        open = !open;
        set_open(ui, key, open);
        if open && let Some(group) = group.filter(|g| solo(ui, g)) {
            close_others(ui, group, key);
        }
    }
    if button == HeaderButton::Reset && reset.clicked() {
        step.name(format!("Reset {title}"), String::new());
    }
    if open {
        set_edit_context(ui, title);
        // The catalog's rows span its sidebar, their own inset making the margin.
        let side = if group == Some(SectionGroup::LibraryLeft.key()) {
            0
        } else {
            content_inset as i8
        };
        // Scope ids per section so equal slider labels (e.g. two "Amount"s) never clash.
        ui.push_id(title, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: side,
                    right: side,
                    top: space::M as i8,
                    bottom: space::L as i8,
                })
                .show(ui, |ui| contents(ui, step));
        });
    }
    resettable && reset.clicked()
}
/// An on/off switch drawn as the panels' is, in a 26 × 20 px slot: what
/// [`paint_switch`] draws, clickable on its own. Named `label` for screen
/// readers. Returns whether it was clicked; the caller turns it.
pub(in crate::app) fn switch(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26., 20.), Sense::click());
    switch_at(ui, rect, on, label, response)
}
/// [`switch`] at `rect`, over what was laid out there: a row it sits on
/// takes no click on it.
pub(in crate::app) fn switch_in(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    on: bool,
    label: &str,
) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    switch_at(ui, rect, on, label, response)
}
fn switch_at(
    ui: &mut egui::Ui,
    rect: Rect,
    on: bool,
    label: &str,
    response: egui::Response,
) -> egui::Response {
    let enabled = ui.is_enabled();
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, label));
    paint_switch(
        ui.painter(),
        rect.center(),
        if on { PanelState::On } else { PanelState::Off },
        response.hovered() && enabled,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}
/// Lightroom's panel switch: a small track with its knob to the right when on.
fn paint_switch(painter: &egui::Painter, c: Pos2, state: PanelState, hovered: bool) {
    let palette = theme::palette(painter.ctx());
    let track = Rect::from_center_size(c, Vec2::new(20., 10.));
    let on = state == PanelState::On;
    painter.rect_filled(
        track,
        5.,
        palette.tone(match (on, hovered) {
            (true, true) => Tone::LABEL,
            (true, false) => Tone::SECONDARY,
            (false, true) => Tone::OUTLINE_STRONG,
            (false, false) => Tone::CARD,
        }),
    );
    painter.rect_stroke(
        track,
        5.,
        Stroke::new(
            1.,
            palette.tone(if on { Tone::FAINT } else { Tone::DISABLED }),
        ),
        egui::StrokeKind::Inside,
    );
    let knob = if on {
        track.right_center() - Vec2::new(5., 0.)
    } else {
        track.left_center() + Vec2::new(5., 0.)
    };
    painter.circle_filled(
        knob,
        3.5,
        palette.tone(if on { Tone::STRONG } else { Tone::MUTED }),
    );
}
#[derive(Clone, Default)]
struct CurveInteraction {
    selected: Option<usize>,
    dragging: Option<usize>,
}
/// Corner radius of a curve's well.
const CURVE_RADIUS: f32 = radius::CONTROL;
/// A curve's well: a dark field with the photo's histogram behind the curve,
/// grey for RGB and in the channel's color for R, G and B, a quarter grid and
/// the identity diagonal. `channel` 0 is RGB, 1–3 are R, G, B.
fn curve_backdrop(ui: &egui::Ui, rect: Rect, histogram: &[[u32; 256]; 3], channel: usize) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, CURVE_RADIUS, palette.tone(Tone::DEEP));
    let bins: Vec<f32> = (0..256)
        .map(|i| match channel {
            1..=3 => histogram[channel - 1][i] as f32,
            _ => histogram.iter().map(|h| h[i] as f32).sum::<f32>(),
        })
        .collect();
    // Ignore the extreme bins when scaling so clipped pixels don't flatten the rest.
    let max = bins[1..255].iter().copied().fold(1., f32::max);
    let fill = match channel {
        1..=3 => curve_color(&palette, channel).gamma_multiply(0.22),
        _ => palette.tone(Tone::RAISED),
    };
    if bins.iter().any(|v| *v > 0.) {
        // Inside the rounded corners, so the fill never pokes past them.
        let area = rect.shrink2(Vec2::new(CURVE_RADIUS / 2., 1.));
        let mut mesh = egui::Mesh::default();
        for (i, v) in bins.iter().enumerate() {
            let x = area.left() + i as f32 / 255. * area.width();
            let h = ((v / max).sqrt()).min(1.) * area.height() * 0.85;
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(x, area.bottom()), fill);
            mesh.colored_vertex(Pos2::new(x, area.bottom() - h), fill);
            if i > 0 {
                mesh.add_triangle(base - 2, base - 1, base);
                mesh.add_triangle(base - 1, base, base + 1);
            }
        }
        painter.add(mesh);
    }
    let grid = Stroke::new(1., Color32::from_white_alpha(14));
    for i in 1..4 {
        let t = i as f32 / 4.;
        let x = rect.left() + t * rect.width();
        let y = rect.bottom() - t * rect.height();
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1., Color32::from_white_alpha(28)),
    );
    painter.rect_stroke(
        rect,
        CURVE_RADIUS,
        Stroke::new(1., palette.tone(Tone::CARD)),
        egui::StrokeKind::Outside,
    );
}
fn curve_color(palette: &theme::Palette, channel: usize) -> Color32 {
    match channel {
        1 => Color32::from_rgb(240, 110, 100),
        2 => Color32::from_rgb(120, 215, 125),
        3 => Color32::from_rgb(120, 160, 245),
        _ => palette.tone(Tone::BRIGHT),
    }
}
/// The width of [`curve_channels`]: four 22 pt dots.
pub(in crate::app) const CURVE_CHANNELS_WIDTH: f32 = 4. * 22.;
/// The point curve's channel picker: a dot in each channel's color, RGB's
/// white, the selected one ringed. Returns whether the channel changed.
pub(in crate::app) fn curve_channels(ui: &mut egui::Ui, channel: &mut usize) -> bool {
    let palette = theme::palette(ui.ctx());
    let mut changed = false;
    for (index, name) in ["RGB", "Red", "Green", "Blue"].into_iter().enumerate() {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.), Sense::click());
        let selected = *channel == index;
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, selected, name)
        });
        let response = response
            .on_hover_text(format!("{name} curve"))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = curve_color(&palette, index);
        let center = rect.center();
        if selected {
            ui.painter().circle_filled(center, 5., color);
            ui.painter()
                .circle_stroke(center, 8.5, Stroke::new(1.5, color.gamma_multiply(0.7)));
        } else {
            let strength = if response.hovered() { 0.85 } else { 0.45 };
            ui.painter()
                .circle_filled(center, 5., color.gamma_multiply(strength));
        }
        if response.clicked() && !selected {
            *channel = index;
            changed = true;
        }
    }
    changed
}
/// A small read-out in a curve's top-left corner: what is under the pointer.
fn curve_overlay(ui: &egui::Ui, rect: Rect, text: &str) {
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(text::MICRO),
        palette.tone(Tone::SOFT),
    );
    let pos = rect.left_top() + Vec2::new(space::S, space::S);
    let back = Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(space::XS, space::XXS));
    ui.painter().rect_filled(
        back,
        radius::SMALL,
        palette.tone(Tone::DEEP).gamma_multiply(0.85),
    );
    ui.painter()
        .galley_with_override_text_color(pos, galley, palette.tone(Tone::SOFT));
}
/// The parametric curve: dragging up or down in the graph changes the region
/// under the pointer, and the three handles below move the region splits.
/// `targeted` is the region a Targeted Adjustment Tool drag is moving, which
/// shows as a hovered one does.
pub(in crate::app) fn parametric_curve_ui(
    ui: &mut egui::Ui,
    effects: &mut crate::model::effects::Effects,
    histogram: &[[u32; 256]; 3],
    targeted: Option<usize>,
) {
    let palette = theme::palette(ui.ctx());
    let size = ui.available_width();
    let (outer, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let rect = outer.shrink(5.);
    curve_backdrop(ui, rect, histogram, 0);
    let hover = response.hover_pos().filter(|p| rect.contains(*p));
    let hovered_region =
        hover.map(|p| effects.parametric_region((p.x - rect.left()) / rect.width()));
    if let Some(i) = hovered_region.or(targeted) {
        let bounds = [
            0.,
            effects.splits[0],
            effects.splits[1],
            effects.splits[2],
            1.,
        ];
        ui.painter().rect_filled(
            Rect::from_x_y_ranges(
                rect.left() + bounds[i] * rect.width()..=rect.left() + bounds[i + 1] * rect.width(),
                rect.y_range(),
            ),
            0.,
            Color32::from_white_alpha(14),
        );
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        let i = effects.parametric_region((origin.x - rect.left()) / rect.width());
        let delta = -response.drag_delta().y / rect.height() * 2.;
        effects.parametric[i] = (effects.parametric[i] + delta).clamp(-1., 1.);
    }
    if response.double_clicked()
        && let Some(i) = hovered_region
    {
        effects.parametric[i] = 0.;
    }
    let pts: Vec<_> = crate::develop::parametric::samples(effects, 128)
        .into_iter()
        .enumerate()
        .map(|(i, y)| {
            Pos2::new(
                rect.left() + i as f32 / 128. * rect.width(),
                rect.bottom() - y * rect.height(),
            )
        })
        .collect();
    ui.painter()
        .with_clip_rect(rect.expand(1.))
        .add(egui::Shape::line(
            pts,
            Stroke::new(1.75, palette.tone(Tone::BRIGHT)),
        ));
    let name = hovered_region.map(|i| ["Shadows", "Darks", "Lights", "Highlights"][i]);
    if let Some(i) = hovered_region.or(targeted) {
        let region = ["Shadows", "Darks", "Lights", "Highlights"][i];
        curve_overlay(
            ui,
            rect,
            &format!("{region} {:+.0}", effects.parametric[i] * 100.),
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text(name.map_or(String::new(), |n| {
            format!("{n}: drag up or down · double-click to reset")
        }));
    // Split handles, as under Lightroom's parametric curve.
    let (strip, _) = ui.allocate_exact_size(Vec2::new(size, 14.), Sense::hover());
    let track = strip.shrink2(Vec2::new(4., 0.));
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(track.x_range(), track.top() + 2.0..=track.top() + 4.),
        1.,
        palette.tone(Tone::HOVER),
    );
    for i in 0..3 {
        let x = track.left() + effects.splits[i] * track.width();
        let handle = Rect::from_center_size(Pos2::new(x, track.center().y), Vec2::new(12., 14.));
        let r = ui
            .interact(handle, ui.id().with(("split", i)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            let lo = if i == 0 {
                0.05
            } else {
                effects.splits[i - 1] + 0.05
            };
            let hi = if i == 2 {
                0.95
            } else {
                effects.splits[i + 1] - 0.05
            };
            effects.splits[i] = ((p.x - track.left()) / track.width()).clamp(lo, hi);
        }
        if r.double_clicked() {
            effects.splits[i] = [0.25, 0.5, 0.75][i];
        }
        let color = palette.tone(if r.hovered() || r.dragged() {
            Tone::STRONG
        } else {
            Tone::LABEL
        });
        let top = Pos2::new(x, track.top() + 3.);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![top, top + Vec2::new(5., 9.), top + Vec2::new(-5., 9.)],
            color,
            Stroke::NONE,
        ));
    }
}
pub(in crate::app) fn tone_curve_ui(
    ui: &mut egui::Ui,
    curve: &mut crate::color::curve::ToneCurve,
    histogram: &[[u32; 256]; 3],
    channel: usize,
) {
    let palette = theme::palette(ui.ctx());
    let id = ui.make_persistent_id("tone-curve-editor");
    let mut state = ui
        .ctx()
        .data(|d| d.get_temp::<CurveInteraction>(id))
        .unwrap_or_default();
    state.selected = state.selected.filter(|i| *i < curve.points.len());
    let (outer, response) =
        ui.allocate_exact_size(Vec2::splat(ui.available_width()), Sense::click_and_drag());
    let rect = outer.shrink(5.);
    let screen = |p: [f32; 2]| {
        Pos2::new(
            rect.left() + p[0] * rect.width(),
            rect.bottom() - p[1] * rect.height(),
        )
    };
    let snap = curve.natural;
    let value = |p: Pos2| {
        let p = [
            (p.x - rect.left()) / rect.width(),
            (rect.bottom() - p.y) / rect.height(),
        ];
        if snap {
            p.map(|v| (v * 255.).round() / 255.)
        } else {
            p
        }
    };
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.hover_pos()));
    let hit = pointer.and_then(|p| {
        curve
            .points
            .iter()
            .position(|q| screen(*q).distance(p) < 9.)
    });
    if response.clicked_by(egui::PointerButton::Secondary) {
        if let Some(i) = hit {
            curve.remove(i);
            state.selected = None;
        }
    } else if response.drag_started() || response.clicked() {
        response.request_focus();
        // Drag origin keeps the same point selected even after a fast initial movement.
        let origin = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin()).or(pointer)
        } else {
            pointer
        };
        if let Some(p) = origin {
            let i = curve
                .points
                .iter()
                .position(|q| screen(*q).distance(p) < 9.)
                .unwrap_or_else(|| curve.insert(value(p)));
            state.selected = Some(i);
            if response.drag_started() {
                state.dragging = Some(i);
            }
        }
    }
    if response.dragged()
        && let (Some(i), Some(p)) = (state.dragging, pointer)
    {
        curve.move_point(i, value(p));
    }
    if response.drag_stopped() {
        state.dragging = None;
    }
    if response.has_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        && let Some(i) = state.selected
    {
        curve.remove(i);
        state.selected = None;
    }
    curve_backdrop(ui, rect, histogram, channel);
    let plot = crate::color::curve::CurveLut::new(curve);
    let color = curve_color(&palette, channel);
    let pts = (0..=256)
        .map(|i| {
            let x = i as f32 / 256.;
            screen([x, plot.evaluate(x)])
        })
        .collect();
    ui.painter()
        .with_clip_rect(rect.expand(1.))
        .add(egui::Shape::line(pts, Stroke::new(1.75, color)));
    // Hollow handles, filled once selected, haloed while under the pointer.
    for (i, p) in curve.points.iter().enumerate() {
        let p = screen(*p);
        let selected = state.selected == Some(i);
        let hovered = hit == Some(i) || state.dragging == Some(i);
        if hovered {
            ui.painter()
                .circle_filled(p, 9., color.gamma_multiply(0.18));
        }
        ui.painter().circle(
            p,
            if selected || hovered { 5. } else { 4.5 },
            if selected {
                color
            } else {
                palette.tone(Tone::DEEP)
            },
            Stroke::new(1.5, color),
        );
    }
    let readout = state.dragging.map(|i| curve.points[i]).or_else(|| {
        pointer.filter(|p| rect.contains(*p)).map(|p| {
            let [x, _] = value(p);
            [x, plot.evaluate(x)]
        })
    });
    if let Some([x, y]) = readout {
        curve_overlay(ui, rect, &format!("{:.0} → {:.0}", x * 255., y * 255.));
    }
    response
        .on_hover_cursor(if state.dragging.is_some() {
            egui::CursorIcon::Grabbing
        } else if hit.is_some() {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Crosshair
        })
        .on_hover_text("Click to add a point, drag to shape it, right-click to remove it");
    // The selected point's values, typed. The row stays while none is selected,
    // so selecting one never moves what lies below.
    ui.add_space(space::S);
    let mut point = state.selected.map(|i| curve.points[i].map(|v| v * 255.));
    let changed = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            let x = point_field(ui, "Input", point.as_mut().map(|p| &mut p[0]));
            ui.add_space(space::M);
            let y = point_field(ui, "Output", point.as_mut().map(|p| &mut p[1]));
            x || y
        })
        .inner;
    if changed && let (Some(i), Some(point)) = (state.selected, point) {
        curve.move_point(i, point.map(|v| v / 255.));
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, state));
}
/// A curve point's Input or Output, 0–255: a quiet caption and a number field,
/// blank and disabled while no point is selected. Returns whether it changed.
fn point_field(ui: &mut egui::Ui, label: &str, value: Option<&mut f32>) -> bool {
    let palette = theme::palette(ui.ctx());
    ui.label(
        egui::RichText::new(label)
            .size(text::SMALL)
            .color(palette.tone(Tone::MUTED)),
    );
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for (visuals, fill) in [
            (&mut widgets.inactive, Tone::FIELD),
            (&mut widgets.hovered, Tone::RAISED),
            (&mut widgets.active, Tone::RAISED),
            (&mut widgets.noninteractive, Tone::CARD),
        ] {
            visuals.weak_bg_fill = palette.tone(fill);
            visuals.bg_fill = palette.tone(fill);
            visuals.bg_stroke = Stroke::NONE;
            visuals.corner_radius = radius::CONTROL.into();
            visuals.expansion = 0.;
        }
        let size = Vec2::new(40., size::CONTROL);
        match value {
            Some(value) => named(
                ui.add_sized(
                    size,
                    egui::DragValue::new(value)
                        .range(0. ..=255.)
                        .update_while_editing(false)
                        .speed(1.)
                        .fixed_decimals(0),
                ),
                label,
            )
            .changed(),
            None => {
                ui.add_enabled_ui(false, |ui| {
                    ui.add_sized(size, egui::Button::new("–"));
                });
                false
            }
        }
    })
    .inner
}
/// The radius of a slider's knob.
const SLIDER_KNOB: f32 = 7.5;
/// The knob's radius while hovered or dragged, which its travel leaves room for.
const SLIDER_KNOB_HOVERED: f32 = SLIDER_KNOB + 1.;
/// The space above a slider, parting it from the one before.
pub(in crate::app) const SLIDER_GAP: f32 = space::M;
/// A slider's height: its label and value line, then its rail.
pub(in crate::app) const SLIDER_ROW: f32 = 42.;
/// The height of a slider's label and value line, above its rail.
pub(in crate::app) const SLIDER_LABEL: f32 = 18.;
/// Width of a slider's number field above its rail.
const SLIDER_VALUE_WIDTH: f32 = 52.;
/// Lightroom-style slider. Normalized ranges within ±1 display as integers
/// (×100), matching Lightroom's numbers while the recipe keeps its units.
pub(in crate::app) fn slider(
    ui: &mut egui::Ui,
    step: &mut StepName,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
) {
    slider_with(ui, step, label, value, range, default, None, None);
}
/// What happened to a slider this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SliderEvent {
    None,
    /// Double-clicked back to its default.
    Reset,
    /// Its label clicked: the slider the Basic panel's keys step.
    LabelClicked,
}
/// Whether a slider shows a tick at its fill's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tick {
    Shown,
    Hidden,
}
/// Where a slider's fill starts, and whether it is marked.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FillOrigin {
    value: f32,
    tick: Tick,
}
/// A slider from zero up fills from zero, as Lightroom's Feather, Amount and Opacity
/// do, so a default in the middle doesn't make it look centred. Any other (one below
/// zero, a colored one like Temp, or one with a neutral point like Levels' Midtone
/// or Scale) fills from its default and marks it.
fn fill_origin(start: f32, default: f32, coloured: bool) -> FillOrigin {
    if start != 0. || coloured {
        FillOrigin {
            value: default,
            tick: Tick::Shown,
        }
    } else {
        FillOrigin {
            value: start,
            tick: Tick::Hidden,
        }
    }
}
/// [`slider`] with its shown scale and a colored rail; the parameters are
/// those of a slider row, and `step` the edit's History name.
#[expect(clippy::too_many_arguments)]
pub(in crate::app) fn slider_with(
    ui: &mut egui::Ui,
    step: &mut StepName,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    display: Option<(f32, usize)>,
    gradient: Option<(Color32, Color32)>,
) -> SliderEvent {
    let style = SliderStyle {
        display,
        gradient,
        ..SliderStyle::default()
    };
    slider_styled(ui, step, label, value, range, default, style)
}
/// How a slider looks and moves beyond its label, range and default.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct SliderStyle<'a> {
    /// What screen readers call the slider when it shows no label of its own,
    /// as the upper end of Defringe's hue range under the lower one's label.
    pub name: Option<&'a str>,
    /// Shown scale and decimals, e.g. Sharpening's 0–150; picked from the range
    /// when `None`.
    pub display: Option<(f32, usize)>,
    /// A colored rail, from its left end to its right.
    pub gradient: Option<(Color32, Color32)>,
    /// The rail is even in the value's reciprocal, as Temp moves in mireds.
    pub reciprocal: bool,
    /// What dragging snaps to, as Exposure's 0.05 EV; typed values stay exact.
    pub drag_step: Option<f32>,
    /// A hue wheel of 0–360° as the rail.
    pub hue_rail: bool,
    /// What a typed value may be, `(low, high)`, when wider than the rail: a
    /// descriptor's valid range, as Exposure's ±8 EV beside its ±5 rail. `None`
    /// keeps typing to the rail's range.
    pub typed: Option<(f32, f32)>,
    /// The label stands out: the slider the Basic panel's keys step.
    pub selected: bool,
}
/// The Temp slider's rail, blue to yellow.
pub(in crate::app) const TEMPERATURE_GRADIENT: (Color32, Color32) = (
    Color32::from_rgb(74, 123, 182),
    Color32::from_rgb(194, 169, 93),
);
/// The Tint slider's rail, green to magenta.
pub(in crate::app) const TINT_GRADIENT: (Color32, Color32) = (
    Color32::from_rgb(91, 156, 112),
    Color32::from_rgb(165, 105, 158),
);
/// The slider for a develop setting, as its descriptor describes it.
pub(in crate::app) fn setting_slider(
    ui: &mut egui::Ui,
    step: &mut StepName,
    id: ParameterId,
    value: &mut f32,
    default: f32,
) -> SliderEvent {
    keyed_setting_slider(ui, step, id, value, default, false)
}
/// [`setting_slider`] for a slider the Basic panel's keys step: its label stands
/// out while `selected`, and a click on it selects it.
pub(in crate::app) fn keyed_setting_slider(
    ui: &mut egui::Ui,
    step: &mut StepName,
    id: ParameterId,
    value: &mut f32,
    default: f32,
    selected: bool,
) -> SliderEvent {
    let gradient = match id {
        ParameterId::Temperature | ParameterId::IncrementalTemperature => {
            Some(TEMPERATURE_GRADIENT)
        }
        ParameterId::Tint | ParameterId::IncrementalTint | ParameterId::ShadowTint => {
            Some(TINT_GRADIENT)
        }
        _ => None,
    };
    descriptor_slider(
        ui,
        step,
        id.descriptor(),
        value,
        default,
        gradient,
        selected,
    )
}
/// The slider for a mask's adjustment, as its descriptor describes it.
pub(in crate::app) fn local_setting_slider(
    ui: &mut egui::Ui,
    step: &mut StepName,
    id: LocalParameterId,
    value: &mut f32,
    default: f32,
) -> SliderEvent {
    let gradient = match id {
        LocalParameterId::Temperature => Some(TEMPERATURE_GRADIENT),
        LocalParameterId::Tint => Some(TINT_GRADIENT),
        _ => None,
    };
    descriptor_slider(ui, step, id.descriptor(), value, default, gradient, false)
}
fn descriptor_slider<Id>(
    ui: &mut egui::Ui,
    step: &mut StepName,
    d: &params::Descriptor<Id>,
    value: &mut f32,
    default: f32,
    gradient: Option<(Color32, Color32)>,
    selected: bool,
) -> SliderEvent {
    let style = SliderStyle {
        name: None,
        display: Some((d.display.scale, d.display.decimals)),
        gradient,
        reciprocal: matches!(d.tick, params::Tick::Mireds(_)),
        drag_step: d.drag_step,
        hue_rail: false,
        typed: Some((*d.valid.start(), *d.valid.end())),
        selected,
    };
    slider_styled(
        ui,
        step,
        d.label,
        value,
        d.interactive.clone(),
        default,
        style,
    )
}
/// How a slider's number field was changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberInput {
    /// Typed and entered: any value the setting may hold.
    Typed,
    /// Dragged, or stepped with the arrow keys: relative to the value before.
    Relative,
}
/// The value a number field entry gives, from `before`: a typed value within
/// `typed`; a relative one as dials and arrow keys move it, within `rail` and never
/// further out than it was.
fn number_input(
    input: NumberInput,
    before: f32,
    entered: f32,
    rail: std::ops::RangeInclusive<f32>,
    (typed_low, typed_high): (f32, f32),
) -> f32 {
    match input {
        NumberInput::Typed => entered.clamp(typed_low, typed_high),
        NumberInput::Relative => params::nudged(before, entered - before, rail),
    }
}
pub(in crate::app) fn slider_styled(
    ui: &mut egui::Ui,
    step: &mut StepName,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    style: SliderStyle<'_>,
) -> SliderEvent {
    let palette = theme::palette(ui.ctx());
    let SliderStyle {
        name,
        display,
        gradient,
        reciprocal,
        drag_step: snap,
        hue_rail,
        typed,
        selected,
    } = style;
    let mut event = SliderEvent::None;
    let start = *range.start();
    let end = *range.end();
    let span = end - start;
    // Typing takes any value the setting may hold; the rail and dragging the number
    // stay within the rail.
    let (typed_low, typed_high) = typed.unwrap_or((start, end));
    let (scale, decimals) = display.unwrap_or(if start >= -1. && end <= 1. {
        (100., 0)
    } else if span >= 100. {
        (1., 0)
    } else {
        (1., 2)
    });
    let signed = start < 0. && default == 0.;
    let origin = fill_origin(start, default, gradient.is_some());
    let to_rail = move |v: f32, rail: Rect| {
        let t = if reciprocal {
            egui::remap_clamp(1. / v, 1. / start..=1. / end, 0. ..=1.)
        } else {
            egui::remap_clamp(v, start..=end, 0. ..=1.)
        };
        egui::lerp(rail.left()..=rail.right(), t)
    };
    let from_rail = move |x: f32, rail: Rect| {
        let t = egui::remap_clamp(x, rail.left()..=rail.right(), 0. ..=1.);
        if reciprocal {
            1. / egui::lerp(1. / start..=1. / end, t)
        } else {
            egui::lerp(start..=end, t)
        }
    };
    let before = *value;
    ui.push_id(label, |ui| {
        ui.add_space(SLIDER_GAP);
        let (row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), SLIDER_ROW), Sense::hover());
        let label_rect = Rect::from_min_max(
            row.min,
            Pos2::new(
                row.right() - SLIDER_VALUE_WIDTH - space::M,
                row.top() + SLIDER_LABEL,
            ),
        );
        let label_response = ui.interact(label_rect, ui.id().with("label"), Sense::click());
        let label_pos = label_rect.left_center();
        let font = egui::FontId::proportional(text::SMALL);
        if selected {
            // A lit label marks the slider the keys step, as in Lightroom.
            let galley =
                ui.painter()
                    .layout_no_wrap(label.to_string(), font, palette.tone(Tone::BRIGHT));
            let text = egui::Align2::LEFT_CENTER.anchor_size(label_pos, galley.size());
            ui.painter().rect_filled(
                text.expand2(Vec2::new(3., 1.)),
                2.,
                palette.tone(Tone::PRESSED),
            );
            ui.painter()
                .galley(text.min, galley, palette.tone(Tone::BRIGHT));
        } else {
            ui.painter().text(
                label_pos,
                egui::Align2::LEFT_CENTER,
                label,
                font,
                palette.tone(Tone::SOFT),
            );
        }
        if label_response.clicked() {
            event = SliderEvent::LabelClicked;
        }
        if label_response.double_clicked() {
            *value = default.clamp(start, end);
            event = SliderEvent::Reset;
        }
        let value_rect = Rect::from_min_max(
            Pos2::new(row.right() - SLIDER_VALUE_WIDTH, row.top()),
            Pos2::new(row.right(), row.top() + SLIDER_LABEL),
        );
        let mut displayed = *value * scale;
        // The arrow keys step a focused number; egui's DragValue consumes them while
        // it draws, so they are read before. A screen reader steps it with
        // accessibility actions instead, which reach it without focus.
        let arrows = ui.input(|i| {
            [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
            ]
            .into_iter()
            .any(|key| i.key_pressed(key))
        });
        let assistive_step = ui.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::AccessKitActionRequest(request)
                        if matches!(
                            request.action,
                            egui::accesskit::Action::Increment | egui::accesskit::Action::Decrement
                        )
                )
            })
        });
        // A fixed field that fits the widest value ("50000", "+5.00"): typing or
        // dragging never widens it over the rail.
        let value_response = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(value_rect)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(value_rect.intersect(ui.clip_rect()));
                    ui.style_mut().override_font_id = Some(egui::FontId::proportional(text::SMALL));
                    let spacing = ui.spacing_mut();
                    spacing.interact_size.x = 0.;
                    spacing.button_padding.x = 0.;
                    // Keep the value quiet until it is hovered or edited.
                    let widgets = &mut ui.visuals_mut().widgets;
                    for (state, fill) in [
                        (&mut widgets.inactive, Tone::CARD),
                        (&mut widgets.hovered, Tone::FIELD),
                    ] {
                        state.bg_fill = palette.tone(fill);
                        state.weak_bg_fill = palette.tone(fill);
                    }
                    widgets.inactive.bg_fill = Color32::TRANSPARENT;
                    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                    widgets.inactive.bg_stroke = Stroke::NONE;
                    // Typed into, the number is a field the width of its slot and
                    // the height of the label line, so its frame is never clipped.
                    // DragValue keys its editing on the next automatic id.
                    let editing = ui.memory(|m| m.has_focus(ui.next_auto_id()));
                    if editing {
                        let spacing = ui.spacing_mut();
                        spacing.interact_size = Vec2::new(SLIDER_VALUE_WIDTH, SLIDER_LABEL);
                        spacing.button_padding = Vec2::new(space::S, 0.);
                        let visuals = ui.visuals_mut();
                        visuals.text_edit_bg_color = Some(palette.tone(Tone::FIELD));
                        // No outline: the fill marks the field. The stroke's color is
                        // also the selected text's, so it stays the theme's.
                        visuals.selection.stroke.width = 0.;
                        visuals.text_cursor.stroke.width = 1.5;
                        for state in [
                            &mut visuals.widgets.inactive,
                            &mut visuals.widgets.hovered,
                            &mut visuals.widgets.active,
                        ] {
                            state.corner_radius = radius::CONTROL.into();
                            state.expansion = 0.;
                        }
                    }
                    named(
                        ui.add(
                            // An imported value beyond the range shows as it is, rather
                            // than being clamped by drawing it.
                            egui::DragValue::new(&mut displayed)
                                .range(typed_low * scale..=typed_high * scale)
                                .clamp_existing_to_range(false)
                                .speed(span * scale / 500.)
                                .custom_formatter(move |v, _| format_value(v, decimals, signed))
                                .custom_parser(|s| s.trim().trim_start_matches('+').parse().ok()),
                        ),
                        name.unwrap_or(label),
                    )
                },
            )
            .inner;
        if value_response.changed() {
            // Dragging the number, the arrow keys stepping it while it has focus, or a
            // screen reader stepping it, move it relative to where it was; anything
            // else was typed.
            let stepped = (value_response.has_focus() && arrows) || assistive_step;
            let input = if value_response.dragged() || stepped {
                NumberInput::Relative
            } else {
                NumberInput::Typed
            };
            *value = number_input(
                input,
                before,
                displayed / scale,
                start..=end,
                (typed_low, typed_high),
            );
        }
        let area = Rect::from_min_max(Pos2::new(row.left(), row.top() + SLIDER_LABEL), row.max);
        let response = ui.interact(area, ui.id().with("rail"), Sense::click_and_drag());
        let rail = Rect::from_min_max(
            Pos2::new(area.left(), area.center().y - 1.5),
            Pos2::new(area.right(), area.center().y + 1.5),
        );
        // The knob stays on the rail, as in Lightroom: its centre travels a
        // hovered knob's radius in from either end, so even enlarged it never
        // passes the rail's ends, the label or the value.
        let travel = rail.shrink2(Vec2::new(SLIDER_KNOB_HOVERED, 0.));
        if hue_rail {
            let mut mesh = egui::Mesh::default();
            let color = |h| {
                let c = crate::develop::color::hue_rgb(h);
                Color32::from_rgb(
                    (c[0] * 180.) as u8,
                    (c[1] * 180.) as u8,
                    (c[2] * 180.) as u8,
                )
            };
            // The hues lie where the knob's centre picks them; the ends past its
            // travel continue red, where the circle closes.
            for cap in [
                Rect::from_x_y_ranges(rail.left()..=travel.left(), rail.y_range()),
                Rect::from_x_y_ranges(travel.right()..=rail.right(), rail.y_range()),
            ] {
                ui.painter().rect_filled(cap, 0., color(0.));
            }
            for sector in 0..6 {
                let left = egui::lerp(travel.left()..=travel.right(), sector as f32 / 6.);
                let right = egui::lerp(travel.left()..=travel.right(), (sector + 1) as f32 / 6.);
                let a = color(sector as f32 / 6.);
                let b = color((sector + 1) as f32 / 6.);
                let first = mesh.vertices.len() as u32;
                mesh.colored_vertex(Pos2::new(left, rail.top()), a);
                mesh.colored_vertex(Pos2::new(right, rail.top()), b);
                mesh.colored_vertex(Pos2::new(right, rail.bottom()), b);
                mesh.colored_vertex(Pos2::new(left, rail.bottom()), a);
                mesh.add_triangle(first, first + 1, first + 2);
                mesh.add_triangle(first, first + 2, first + 3);
            }
            ui.painter().add(mesh);
        } else if let Some((a, b)) = gradient {
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(rail.left_top(), a);
            mesh.colored_vertex(rail.right_top(), b);
            mesh.colored_vertex(rail.right_bottom(), b);
            mesh.colored_vertex(rail.left_bottom(), a);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(mesh);
        } else {
            ui.painter()
                .rect_filled(rail, radius::SMALL, palette.tone(Tone::OUTLINE_STRONG));
        }
        // A tick where a centred slider rests, or a colored one's default (Temp's As
        // Shot); a slider from zero up, like Feather, fills from its left end.
        let neutral = to_rail(origin.value, travel);
        if origin.tick == Tick::Shown {
            ui.painter().line_segment(
                [
                    Pos2::new(neutral, area.center().y - 4.),
                    Pos2::new(neutral, area.center().y + 4.),
                ],
                Stroke::new(1., palette.tone(Tone::DISABLED)),
            );
        }
        if response.double_clicked() {
            *value = default.clamp(start, end);
            event = SliderEvent::Reset;
        } else if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            let v = from_rail(p.x, travel);
            let v = snap.map_or(v, |snap| (v / snap).round() * snap);
            *value = v.clamp(start, end);
        } else if let Some(nudge) = hovered_nudge(ui, row) {
            // Lightroom's keys over a hovered slider: Up and Down move it by its
            // smallest shown step, ten with Shift.
            let unit = snap.unwrap_or(10f32.powi(-(decimals as i32)) / scale);
            *value = crate::model::params::nudged(*value, nudge * unit, start..=end);
        }
        let x = to_rail(*value, travel);
        if gradient.is_none() {
            // A fill from a range end starts at the rail's end, not where the
            // knob's travel does, so no gap shows there.
            let from = if neutral <= travel.left() {
                rail.left()
            } else if neutral >= travel.right() {
                rail.right()
            } else {
                neutral
            };
            ui.painter().rect_filled(
                Rect::from_x_y_ranges(from.min(x)..=from.max(x), rail.y_range()),
                radius::SMALL,
                palette.tone(Tone::SECONDARY),
            );
        }
        let center = Pos2::new(x, area.center().y);
        ui.painter().circle_filled(
            center,
            if response.hovered() || response.dragged() {
                SLIDER_KNOB_HOVERED
            } else {
                SLIDER_KNOB
            },
            palette.tone(Tone::SOFT),
        );
        ui.painter().circle_stroke(
            center,
            SLIDER_KNOB,
            Stroke::new(1., palette.tone(Tone::BAR)),
        );
        response.on_hover_text(
            "Drag to adjust · double-click to reset. Drag or type the number for precise edits.",
        );
    });
    if *value != before {
        let context = edit_context(ui);
        let name = if label.is_empty() {
            context
        } else if matches!(
            context.as_str(),
            "" | "Basic" | "Tone" | "Presence" | "Tone Curve"
        ) {
            label.to_string()
        } else {
            format!("{context} {label}")
        };
        let shown = format_value(f64::from(*value * scale), decimals, signed);
        step.name(name, shown);
    }
    event
}
/// Steps the Up and Down keys ask of the slider in `row` this frame: +1 or −1 each,
/// ×10 with Shift. Only while the slider is enabled, the pointer is over the row and no text field (a
/// slider's number being typed, a search) has the keyboard, which keeps the keys;
/// Left and Right stay with photo navigation, and scrolling never moves a slider.
fn hovered_nudge(ui: &egui::Ui, row: Rect) -> Option<f32> {
    if !ui.is_enabled()
        || !ui.rect_contains_pointer(row)
        || ui.ctx().text_edit_focused()
        || ui.input(|i| i.pointer.any_down())
    {
        return None;
    }
    let nudge = ui.input_mut(|i| {
        let mut nudge = 0.;
        // Shift first: the plain pattern would also take Shift's presses.
        for (modifiers, size) in [(egui::Modifiers::SHIFT, 10.), (egui::Modifiers::NONE, 1.)] {
            nudge += size
                * (i.count_and_consume_key(modifiers, egui::Key::ArrowUp) as f32
                    - i.count_and_consume_key(modifiers, egui::Key::ArrowDown) as f32);
        }
        nudge
    });
    (nudge != 0.).then_some(nudge)
}
/// Size of a segmented control: the track's height and the label size.
pub(in crate::app) struct SegmentStyle {
    height: f32,
    font: f32,
}
/// The Library/Develop switcher in the top bar.
pub(in crate::app) const TOP_BAR_SEGMENTS: SegmentStyle = SegmentStyle {
    height: 26.,
    font: 13.,
};
/// Toolbar controls, as tall as the toolbar buttons.
const TOOLBAR_SEGMENTS: SegmentStyle = SegmentStyle {
    height: 32.,
    font: 12.,
};
/// Height of compact segmented controls; fields beside them use it too so
/// their edges line up.
pub(in crate::app) const COMPACT_SEGMENT_HEIGHT: f32 = size::CONTROL;
/// Panel and filter controls.
const COMPACT_SEGMENTS: SegmentStyle = SegmentStyle {
    height: COMPACT_SEGMENT_HEIGHT,
    font: 11.,
};
/// Segmented control sized to its labels: equal-width segments, so switching
/// never moves a label. `selected` None shows no active segment.
pub(in crate::app) fn segment_bar<const N: usize>(
    ui: &mut egui::Ui,
    labels: [&str; N],
    selected: Option<usize>,
    style: &SegmentStyle,
) -> [egui::Response; N] {
    let width = segments_width(ui, &labels, style);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, style.height.max(28.)), Sense::hover());
    segment_track(ui, rect, response.id, &labels, selected, style)
        .try_into()
        .expect("one response per label")
}
/// Corner radius for a control of this height, shared by segmented
/// controls and action buttons so they read as one family.
fn control_radius(height: f32) -> f32 {
    (height * 0.27).round()
}
/// Padding each side of a label: roomy when there is space, never below the
/// minimum.
const SEGMENT_PADDING: f32 = space::L;
const SEGMENT_MIN_PADDING: f32 = space::M;
/// What the padding gives way to, at the smaller label size, before the
/// control grows wider than its place (and past the panel holding it).
const SEGMENT_TIGHT_PADDING: f32 = space::XS;
/// The pill's gap to the track.
const SEGMENT_INSET: f32 = space::XXS;
fn label_widths(ui: &egui::Ui, labels: &[&str], font: f32) -> Vec<f32> {
    let white = theme::palette(ui.ctx()).tone(Tone::BRIGHT);
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).into(), egui::FontId::proportional(font), white)
                .size()
                .x
        })
        .collect()
}
/// A roomy track: equal segments as wide as the widest label plus padding.
fn segments_width(ui: &egui::Ui, labels: &[&str], style: &SegmentStyle) -> f32 {
    let widest = label_widths(ui, labels, style.font)
        .into_iter()
        .fold(0., f32::max);
    labels.len() as f32 * (widest + 2. * SEGMENT_PADDING) + 2. * SEGMENT_INSET
}
/// The narrowest track: each segment only as wide as its own label, at the
/// label size one point down, with the tightest padding.
fn tightest_segments_width(ui: &egui::Ui, labels: &[&str], style: &SegmentStyle) -> f32 {
    label_widths(ui, labels, style.font - 1.)
        .into_iter()
        .map(|width| width + 2. * SEGMENT_TIGHT_PADDING)
        .sum::<f32>()
        + 2. * SEGMENT_INSET
}
/// Segment widths and label size for a track `inner` wide: equal widths when
/// every label gets its minimum padding, else each label's own width with the
/// leftover shared out, at a point smaller if even that does not fit, with
/// the padding narrowed as far as the tightest.
fn segment_layout(
    ui: &egui::Ui,
    labels: &[&str],
    style: &SegmentStyle,
    inner: f32,
) -> (Vec<f32>, f32) {
    let count = labels.len().max(1) as f32;
    for font in [style.font, style.font - 1.] {
        let widths = label_widths(ui, labels, font);
        let widest = widths.iter().copied().fold(0., f32::max);
        if inner / count >= widest + 2. * SEGMENT_MIN_PADDING {
            return (vec![inner / count; labels.len()], font);
        }
        let labels_width: f32 = widths.iter().sum();
        let needed = labels_width + count * 2. * SEGMENT_MIN_PADDING;
        if needed <= inner || font < style.font {
            let padding = ((inner - labels_width) / (2. * count))
                .clamp(SEGMENT_TIGHT_PADDING, SEGMENT_MIN_PADDING);
            let extra = (inner - labels_width - count * 2. * padding).max(0.) / count;
            let widths = widths.iter().map(|w| w + 2. * padding + extra).collect();
            return (widths, font);
        }
    }
    unreachable!("the smaller font always returns")
}
/// The shared look of every segmented control: a dark track holding
/// segments sized to their labels, with a raised pill that slides to the
/// selected one while the labels cross-fade, like Claude's view switcher.
fn segment_track(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    labels: &[&str],
    selected: Option<usize>,
    style: &SegmentStyle,
) -> Vec<egui::Response> {
    let palette = theme::palette(ui.ctx());
    let inset = SEGMENT_INSET;
    let radius = control_radius(style.height);
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), style.height));
    // Visible on the dark side panels as on lighter surfaces, so the pill
    // sits inside a track as wide as the control.
    ui.painter().rect(
        track,
        radius,
        palette.tone(Tone::BAR),
        Stroke::new(1., palette.tone(Tone::FIELD)),
        egui::StrokeKind::Inside,
    );
    let (widths, font) = segment_layout(ui, labels, style, track.width() - 2. * inset);
    let starts: Vec<f32> = widths
        .iter()
        .scan(inset, |x, width| {
            let start = *x;
            *x += width;
            Some(start)
        })
        .collect();
    // A fractional position lies between two segments while the pill slides.
    let segment = |position: f32| {
        let last = widths.len().saturating_sub(1);
        let (low, high) = (
            (position.floor() as usize).min(last),
            (position.ceil() as usize).min(last),
        );
        let t = position - position.floor();
        let left = starts[low] + (starts[high] - starts[low]) * t;
        let width = widths[low] + (widths[high] - widths[low]) * t;
        Rect::from_min_size(
            track.min + Vec2::new(left, inset),
            Vec2::new(width, track.height() - 2. * inset),
        )
    };
    // Glide from where the pill was when the selection changed, easing out.
    let target = selected.unwrap_or(0) as f32;
    let linear = ui.ctx().animate_value_with_time(id, target, 0.18);
    let start_id = id.with("start");
    let (last, start) = ui
        .ctx()
        .data(|data| data.get_temp::<(f32, f32)>(start_id))
        .unwrap_or((target, target));
    let start = if last == target { start } else { linear };
    ui.ctx()
        .data_mut(|data| data.insert_temp(start_id, (target, start)));
    let position = if target == start {
        target
    } else {
        let progress = ((linear - start) / (target - start)).clamp(0., 1.);
        start + egui::emath::easing::cubic_out(progress) * (target - start)
    };
    let enabled = ui.is_enabled();
    let responses: Vec<_> = (0..labels.len())
        .map(|index| {
            ui.interact(
                segment(index as f32),
                id.with(("segment", index)),
                Sense::click(),
            )
        })
        .collect();
    for (index, response) in responses.iter().enumerate() {
        if selected != Some(index) && enabled && response.hovered() {
            ui.painter().rect_filled(
                segment(index as f32),
                radius - inset,
                palette.tone(Tone::FIELD),
            );
        }
    }
    if selected.is_some() {
        ui.painter().rect(
            segment(position),
            radius - inset,
            palette.tone(Tone::HOVER),
            Stroke::new(1., palette.tone(Tone::PRESSED)),
            egui::StrokeKind::Inside,
        );
    }
    for (index, label) in labels.iter().enumerate() {
        let rect = segment(index as f32);
        // How much of the pill sits under this label.
        let weight = match selected {
            Some(_) => 1. - (position - index as f32).abs().min(1.),
            None => 0.,
        };
        let color = if enabled {
            palette
                .tone(Tone::SECONDARY)
                .lerp_to_gamma(palette.tone(Tone::BRIGHT), weight)
        } else {
            palette.tone(Tone::PRESSED)
        };
        let text = ui.painter().layout_no_wrap(
            (*label).into(),
            egui::FontId::proportional(font),
            palette.tone(Tone::BRIGHT),
        );
        ui.painter()
            .galley_with_override_text_color(rect.center() - text.size() / 2., text, color);
    }
    responses
        .into_iter()
        .map(|response| {
            if enabled {
                response.on_hover_cursor(egui::CursorIcon::PointingHand)
            } else {
                response
            }
        })
        .collect()
}
/// A primary button sized to its content, in the accent color: an optional
/// icon and the label centred together, with the segmented controls' height
/// and rounding.
pub(in crate::app) fn action_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: Option<Icon>,
    enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let height = TOOLBAR_SEGMENTS.height;
    let (padding, icon_size, gap) = (12., 14., 6.);
    let text = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::proportional(TOOLBAR_SEGMENTS.font),
        palette.tone(Tone::BRIGHT),
    );
    let content = text.size().x + icon.map_or(0., |_| icon_size + gap);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(content + 2. * padding, height),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();
    let (fill, stroke, ink) = if enabled {
        (
            if hovered && !pressed {
                palette.accent_hover()
            } else {
                palette.accent()
            },
            Color32::TRANSPARENT,
            palette.on_accent(),
        )
    } else {
        (
            palette.tone(Tone::CARD),
            palette.tone(Tone::RAISED),
            palette.tone(Tone::OUTLINE_STRONG),
        )
    };
    ui.painter().rect(
        rect,
        control_radius(height),
        fill,
        Stroke::new(1., stroke),
        egui::StrokeKind::Inside,
    );
    // Icon and label form one group centred in the button.
    let mut x = rect.center().x - content / 2.;
    if let Some(icon) = icon {
        icons::paint_at(
            ui.painter(),
            icon,
            Pos2::new(x + icon_size / 2., rect.center().y),
            icon_size,
            ink,
        );
        x += icon_size + gap;
    }
    let y = rect.center().y - text.size().y / 2.;
    ui.painter()
        .galley_with_override_text_color(Pos2::new(x, y), text, ink);
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}
/// Whether a [`segmented`] control of `labels` fits the space left, with
/// its segments and text at their tightest.
pub(in crate::app) fn segments_fit(ui: &egui::Ui, labels: &[&str]) -> bool {
    tightest_segments_width(ui, labels, &COMPACT_SEGMENTS) <= ui.available_width()
}
/// Compact segmented control for panels and filters: at least `width` wide
/// with roomy segments, but never wider than the space left in the panel;
/// there segments shrink to their labels, then the text a point, so labels
/// stay whole with at least 8 px padding.
pub(in crate::app) fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
    width: f32,
) -> bool {
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let preferred = segments_width(ui, &labels, &COMPACT_SEGMENTS);
    let tightest = tightest_segments_width(ui, &labels, &COMPACT_SEGMENTS);
    let width = width.max(preferred).min(ui.available_width().max(tightest));
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, COMPACT_SEGMENTS.height), Sense::hover());
    let selected = options.iter().position(|(option, _)| *option == *value);
    let responses = segment_track(ui, rect, response.id, &labels, selected, &COMPACT_SEGMENTS);
    for (response, (option, _)) in responses.iter().zip(options) {
        if response.clicked() && *value != *option {
            *value = *option;
            return true;
        }
    }
    false
}
/// The height of a panel drop-down, a little taller than a button.
pub(in crate::app) const DROPDOWN_HEIGHT: f32 = size::HEADER;
/// A drop-down in the panels' style: rounded, outlined, a chevron at its end.
/// It styles the rest of `ui`'s row too, so call it inside the row's closure,
/// or in [`keep_style`] when other widgets follow it in that row.
/// Every drop-down in the app is built here: `clippy.toml` disallows egui's
/// own [`egui::ComboBox`] constructors.
pub(in crate::app) fn dropdown(ui: &mut egui::Ui, id_salt: impl egui::AsIdSalt) -> egui::ComboBox {
    let palette = theme::palette(ui.ctx());
    let spacing = ui.spacing_mut();
    spacing.interact_size.y = DROPDOWN_HEIGHT;
    spacing.button_padding = Vec2::new(space::L, space::XS);
    spacing.icon_width = 14.;
    spacing.icon_spacing = space::S;
    let widgets = &mut ui.visuals_mut().widgets;
    for (visuals, fill, outline) in [
        (&mut widgets.inactive, Tone::FIELD, Tone::RAISED),
        (&mut widgets.hovered, Tone::RAISED, Tone::OUTLINE),
        (&mut widgets.active, Tone::HOVER, Tone::OUTLINE),
        (&mut widgets.open, Tone::RAISED, Tone::OUTLINE_STRONG),
    ] {
        visuals.corner_radius = (radius::CARD - 2.).into();
        visuals.weak_bg_fill = palette.tone(fill);
        visuals.bg_fill = palette.tone(fill);
        visuals.bg_stroke = Stroke::new(1., palette.tone(outline));
        visuals.expansion = 0.;
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place that builds an egui combo box, in the panels' style"
    )]
    let combo = egui::ComboBox::from_id_salt(id_salt);
    combo.icon(|ui, rect, visuals, _open| {
        icons::paint_at(
            ui.painter(),
            Icon::ChevronDown,
            rect.center(),
            14.,
            visuals.fg_stroke.color,
        );
    })
}
/// A [`dropdown`] `height` points tall, for a panel whose rows keep to a
/// smaller grid than drop-downs elsewhere (the Crop drawer, a tool drawer).
pub(in crate::app) fn dropdown_of_height(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    height: f32,
) -> egui::ComboBox {
    let combo = dropdown(ui, id_salt);
    let spacing = ui.spacing_mut();
    spacing.interact_size.y = height;
    spacing.button_padding.y = space::XXS;
    combo
}
/// A row as tall as a [`dropdown`], laid out left to right, so the labels,
/// buttons and checkboxes before and after a drop-down sit on its middle.
/// Only the row's own widgets take the height.
pub(in crate::app) fn dropdown_row<R>(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    keep_style(ui, |ui| {
        ui.spacing_mut().interact_size.y = DROPDOWN_HEIGHT;
        ui.horizontal(add).inner
    })
}
/// A one-line text field as tall as a [`dropdown`], its text on the middle,
/// for a field beside one.
pub(in crate::app) fn dropdown_text_edit(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .min_size(Vec2::new(0., DROPDOWN_HEIGHT))
        .vertical_align(egui::Align::Center)
        .margin(design::margin(space::M, space::XS))
}
/// Runs `add`, which draws a [`dropdown`], then puts back `ui`'s style, so
/// the widgets after it in the row keep their own look. Unlike
/// [`egui::Ui::scope`] it draws into `ui` itself, so the drop-down keeps the
/// id it would have had there.
pub(in crate::app) fn keep_style<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let style = ui.style().clone();
    let inner = add(ui);
    ui.set_style(style);
    inner
}
/// Sets a menu's width: `preferred`, or the width its widest
/// [`menu_item`] asked for, so that a long label never runs into its
/// shortcut (#979). Call it first in the menu, in place of
/// [`egui::Ui::set_width`].
pub(in crate::app) fn menu_width(ui: &mut egui::Ui, preferred: f32) {
    let needed = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(menu_width_id(ui)))
        .unwrap_or(0.);
    ui.set_width(preferred.max(needed));
}
/// Where [`menu_item`] records the width its menu needs. A menu is a popup
/// with its own layer, so the layer names it across frames.
fn menu_width_id(ui: &egui::Ui) -> egui::Id {
    egui::Id::new("menu-width").with(ui.layer_id())
}
/// The space between a menu row's label and its shortcut.
const MENU_SHORTCUT_GAP: f32 = space::XL;
/// A full-width menu row: optional check mark, label, right-aligned shortcut.
/// A row too narrow for both records the width it needs, for
/// [`menu_width`] to widen the menu, and discards the frame so it is drawn
/// again at that width before anyone sees the overlap.
pub(in crate::app) fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
    checked: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let label_galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(text::BODY),
        Color32::PLACEHOLDER,
    );
    let shortcut_galley = (!shortcut.is_empty()).then(|| {
        ui.painter().layout_no_wrap(
            shortcut.to_owned(),
            egui::FontId::proportional(text::SMALL),
            palette.tone(if enabled {
                Tone::MUTED
            } else {
                Tone::OUTLINE_STRONG
            }),
        )
    });
    let needed = (MENU_LABEL_INSET
        + label_galley.size().x
        + shortcut_galley
            .as_ref()
            .map_or(0., |g| MENU_SHORTCUT_GAP + g.size().x)
        + MENU_SHORTCUT_INSET)
        .ceil();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    fit_menu_row(ui, rect, needed);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.accent());
    }
    let color = if enabled && response.hovered() {
        palette.on_accent_text(Tone::BRIGHT)
    } else {
        palette.tone(if enabled { Tone::TEXT } else { Tone::DISABLED })
    };
    if checked {
        let c = rect.left_center() + Vec2::new(11., 0.);
        icons::paint_at(ui.painter(), Icon::Check, c, 13., color);
    }
    let y = rect.center().y - label_galley.size().y / 2.;
    ui.painter().galley_with_override_text_color(
        Pos2::new(rect.left() + MENU_LABEL_INSET, y),
        label_galley,
        color,
    );
    if let Some(galley) = shortcut_galley {
        let at = Pos2::new(
            rect.right() - MENU_SHORTCUT_INSET - galley.size().x,
            rect.center().y - galley.size().y / 2.,
        );
        ui.painter().galley(at, galley, Color32::PLACEHOLDER);
    }
    enabled && response.clicked()
}
/// A menu row's label from the row's left edge, past the check mark.
const MENU_LABEL_INSET: f32 = 22.;
/// A menu row's shortcut from the row's right edge.
const MENU_SHORTCUT_INSET: f32 = 10.;
pub(in crate::app) fn menu_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1., theme::palette(ui.ctx()).tone(Tone::OUTLINE)),
    );
}
/// True for a context-menu click: a right click, or Control-click on macOS.
pub(in crate::app) fn context_clicked(response: &egui::Response) -> bool {
    response.secondary_clicked()
        || (cfg!(target_os = "macos")
            && response.clicked()
            && response.ctx.input(|i| i.modifiers.ctrl))
}
/// Like `Response::context_menu`, but Control-click also opens it on macOS.
pub(in crate::app) fn context_menu(response: &egui::Response, add: impl FnOnce(&mut egui::Ui)) {
    let command = if context_clicked(response) {
        Some(egui::SetOpenCommand::Bool(true))
    } else if response.clicked() {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    egui::Popup::menu(response)
        .open_memory(command)
        .at_pointer_fixed()
        .show(add);
}
/// Widens the menu for a row of `rect` that needs `needed`, as [`menu_item`]
/// describes.
fn fit_menu_row(ui: &mut egui::Ui, rect: Rect, needed: f32) {
    if needed > rect.width().ceil() {
        // Counting the width in the menu's bounds lets egui place the
        // popup for it already on the frame it opens, before the redraw.
        ui.expand_to_include_x(rect.left() + needed);
        let id = menu_width_id(ui);
        let recorded = ui.ctx().data(|d| d.get_temp::<f32>(id)).unwrap_or(0.);
        if needed > recorded {
            ui.ctx().data_mut(|d| d.insert_temp(id, needed));
            ui.ctx()
                .request_discard("a menu row is wider than its menu");
        }
    }
}
/// A menu row that opens `add` as a submenu, drawn as [`menu_item`] draws its
/// rows: the label where theirs start, a chevron where their shortcuts end,
/// highlighted while hovered, focused or open.
pub(in crate::app) fn submenu<R>(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    const CHEVRON: f32 = 12.;
    let palette = theme::palette(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(text::BODY),
        Color32::PLACEHOLDER,
    );
    let needed =
        (MENU_LABEL_INSET + galley.size().x + MENU_SHORTCUT_GAP + CHEVRON + MENU_SHORTCUT_INSET)
            .ceil();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    fit_menu_row(ui, rect, needed);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let inner = enabled
        .then(|| egui::containers::menu::SubMenu::new().show(ui, &response, add))
        .flatten();
    let lit = enabled && (response.hovered() || response.has_focus() || inner.is_some());
    if lit {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.accent());
    }
    let color = if lit {
        palette.on_accent_text(Tone::BRIGHT)
    } else {
        palette.tone(if enabled { Tone::TEXT } else { Tone::DISABLED })
    };
    let y = rect.center().y - galley.size().y / 2.;
    ui.painter().galley_with_override_text_color(
        Pos2::new(rect.left() + MENU_LABEL_INSET, y),
        galley,
        color,
    );
    let chevron = Pos2::new(
        rect.right() - MENU_SHORTCUT_INSET - CHEVRON / 2.,
        rect.center().y,
    );
    icons::paint_at(ui.painter(), Icon::ChevronRight, chevron, CHEVRON, color);
    inner.map(|r| r.inner)
}
/// The frame of a modal window such as Preferences or Export.
pub(in crate::app) fn modal_frame(palette: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.tone(Tone::CARD))
        .stroke(Stroke::new(1., palette.tone(Tone::OUTLINE)))
        .corner_radius(radius::DIALOG)
}
/// `response`'s control, called `name` for screen readers: the label shown
/// beside it, or for an icon its hover text. egui names a text field, a
/// number or a drop-down only by a label it draws itself, which the panels'
/// controls don't use.
pub(in crate::app) fn named(response: egui::Response, name: &str) -> egui::Response {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_label(name));
    response
}
/// A drop-down's [`egui::ComboBox::show_ui`] result, its button called `name`
/// for screen readers, as [`named`] calls other controls.
pub(in crate::app) fn named_menu<R>(
    menu: egui::InnerResponse<R>,
    name: &str,
) -> egui::InnerResponse<R> {
    let egui::InnerResponse { inner, response } = menu;
    egui::InnerResponse::new(inner, named(response, name))
}
/// Makes the [`egui::Modal`] whose contents `ui` draws a modal dialog called
/// `title` for screen readers. egui gives a modal's area no role or name; its
/// contents sit under the node of the area's move response, which egui keys
/// on the area's layer as `"move"`, so that node is the dialog's.
pub(in crate::app) fn name_dialog(ui: &egui::Ui, title: &str) {
    ui.ctx()
        .accesskit_node_builder(ui.layer_id().id.with("move"), |node| {
            node.set_role(egui::accesskit::Role::Dialog);
            node.set_label(title);
            node.set_modal();
        });
}
/// The button that confirms a modal window ("Done", "Export").
pub(in crate::app) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(palette.on_accent()))
            .fill(palette.accent())
            // As tall as the plain buttons beside it, which the row's padding sizes.
            .min_size(Vec2::new(84., 0.)),
    )
}
/// A modal window asking before an action: a title, a line of detail and
/// `buttons`, the last one primary. Returns the choice clicked, `dismissed`
/// when the window is closed, or None while it is open. Return confirms
/// nothing unless a button has focus. A `truncated` detail stays on one line,
/// e.g. a path. Longer details scroll independently of the title and buttons.
pub(in crate::app) fn confirm_modal<T: Copy>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    detail: &str,
    truncated: bool,
    buttons: &[(&str, T)],
    dismissed: T,
) -> Option<T> {
    let palette = theme::palette(ctx);
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::new(id))
        .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
        .show(ctx, |ui| {
            name_dialog(ui, title);
            ui.set_width(420.);
            ui.label(
                egui::RichText::new(title)
                    .size(text::TITLE)
                    .color(palette.tone(Tone::STRONG)),
            );
            ui.add_space(space::S);
            let detail = egui::Label::new(
                egui::RichText::new(detail)
                    .size(text::LABEL)
                    .color(palette.tone(Tone::SECONDARY)),
            );
            if truncated {
                ui.add(detail.truncate());
            } else {
                // Reserve room for the frame, heading and actions even on a
                // short viewport. In particular, Synchronize Folder can list
                // hundreds of missing photos here.
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 180.).clamp(0., 360.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add(detail.wrap());
                    });
            }
            ui.add_space(space::XXL);
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = button::DIALOG;
                for (i, (text, value)) in buttons.iter().enumerate() {
                    let clicked = if i + 1 == buttons.len() {
                        primary_button(ui, text).clicked()
                    } else {
                        ui.button(*text).clicked()
                    };
                    if clicked {
                        choice = Some(*value);
                    }
                }
            });
        });
    choice.or(response.should_close().then_some(dismissed))
}
/// A confirmation for something that can't be undone with Undo, such as
/// removing photos from the catalog: Cancel has the focus, so Return or
/// Space alone cancels, as do Escape and a click outside; the action needs a
/// click, or Tab to it first. Keys already held down when it opened (a held
/// Delete repeating, a Return still down) do nothing until they are
/// released. Several actions may follow Cancel, the last the primary one:
/// Some(Some(i)) for `actions[i]`, Some(None) for cancelling, None while it
/// is open. Tab goes from Cancel to each in turn.
pub(in crate::app) fn destructive_choice(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    lines: &[String],
    cancel: &str,
    actions: &[&str],
) -> Option<Option<usize>> {
    destructive_choice_warned(ctx, id, title, None, lines, cancel, actions)
}

/// [`destructive_choice`], with `warning` below the title in the danger
/// color, for what can't be undone.
pub(in crate::app) fn destructive_choice_warned(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    warning: Option<&str>,
    lines: &[String],
    cancel: &str,
    actions: &[&str],
) -> Option<Option<usize>> {
    /// The keys down when the dialog first showed, until all are released.
    #[derive(Clone, Default)]
    struct Opened {
        held: Vec<egui::Key>,
        focused: bool,
    }
    let key = egui::Id::new(id).with("destructive-confirm");
    let down: Vec<egui::Key> = ctx.input(|i| i.keys_down.iter().copied().collect());
    let mut opened = ctx
        .data_mut(|d| d.get_temp::<Opened>(key))
        .unwrap_or_else(|| Opened {
            held: down.clone(),
            focused: false,
        });
    // While a key held from before is down, no key reaches the buttons.
    opened.held.retain(|k| down.contains(k));
    if !opened.held.is_empty() {
        ctx.input_mut(|i| {
            i.events.retain(|e| !matches!(e, egui::Event::Key { .. }));
        });
    }
    let palette = theme::palette(ctx);
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::new(id))
        .frame(modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
        .show(ctx, |ui| {
            name_dialog(ui, title);
            ui.set_width(440.);
            ui.label(
                egui::RichText::new(title)
                    .size(text::TITLE)
                    .color(palette.tone(Tone::STRONG)),
            );
            ui.add_space(space::S);
            if let Some(warning) = warning {
                ui.label(
                    egui::RichText::new(warning)
                        .size(text::LABEL)
                        .strong()
                        .color(palette.danger()),
                );
                ui.add_space(space::S);
            }
            for line in lines {
                ui.label(
                    egui::RichText::new(line)
                        .size(text::LABEL)
                        .color(palette.tone(Tone::SECONDARY)),
                );
            }
            ui.add_space(space::XXL);
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = button::DIALOG;
                let cancelled = ui.button(cancel);
                // Cancel has the focus at first, and keeps it while a key
                // held from before is down (egui moves focus on Tab before
                // anything here sees the key).
                if !opened.focused || !opened.held.is_empty() {
                    cancelled.request_focus();
                    opened.focused = true;
                }
                if cancelled.clicked() {
                    choice = Some(None);
                }
                for (i, action) in actions.iter().enumerate() {
                    let clicked = if i + 1 == actions.len() {
                        primary_button(ui, action).clicked()
                    } else {
                        ui.button(*action).clicked()
                    };
                    if clicked {
                        choice = Some(Some(i));
                    }
                }
            });
        });
    let choice = choice.or(response.should_close().then_some(None));
    ctx.data_mut(|d| match choice {
        Some(_) => d.remove::<Opened>(key),
        None => {
            d.insert_temp(key, opened);
        }
    });
    choice
}
/// "1 photo", "2 photos": `n` with the word for its number.
pub(in crate::app) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
/// A form row as in Lightroom's dialogs: a right-aligned label in a fixed
/// column, then the controls.
pub(in crate::app) fn form_row(
    ui: &mut egui::Ui,
    label: &str,
    contents: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(150., 28.), Sense::hover());
        ui.painter().text(
            rect.right_center() - Vec2::new(12., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(text::BODY),
            theme::palette(ui.ctx()).tone(Tone::SECONDARY),
        );
        contents(ui);
    });
}
/// A row in a tool drawer, as its sliders are: the label right-aligned,
/// ending [`LABEL_GAP`] short of the label column, and `add`'s control
/// starting as far past it. With `top`, the label sits beside a tall
/// control's first line (a [`text_box`]) instead of its middle.
pub(in crate::app) fn drawer_row(
    ui: &mut egui::Ui,
    label: &str,
    top: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    let row = |ui: &mut egui::Ui| {
        let spacing = ui.spacing().item_spacing.x;
        // A text box's first line, with its margin above and below.
        let height = if top {
            size::CONTROL + space::M
        } else {
            size::CONTROL
        };
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(label_column(ui) - LABEL_GAP, height),
            Sense::hover(),
        );
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(text::SMALL),
            theme::palette(ui.ctx()).tone(Tone::SOFT),
        );
        // Only the gap after the label widens: the control keeps its own
        // spacing.
        ui.add_space((LABEL_GAP + LABEL_GAP - spacing).max(0.));
        add(ui);
    };
    // A row as tall as its control, not the room left below it.
    if top {
        ui.horizontal_top(row);
    } else {
        ui.horizontal(row);
    }
}
/// A tool drawer's line of help under a control, starting where the
/// control does.
pub(in crate::app) fn drawer_help(ui: &mut egui::Ui, help: &str) {
    ui.horizontal(|ui| {
        ui.add_space(control_left(ui));
        ui.add(
            egui::Label::new(
                egui::RichText::new(help)
                    .size(text::MICRO)
                    .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
            )
            .wrap(),
        );
    });
}
/// A drawer's note of its own, as wide as the drawer: under its tabs, or a
/// panel's Help without a label. [`drawer_help`] is a control's, in the
/// controls' column.
pub(in crate::app) fn drawer_note(ui: &mut egui::Ui, note: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(note)
                .size(text::MICRO)
                .color(theme::palette(ui.ctx()).tone(Tone::FAINT)),
        )
        .wrap(),
    );
}
/// A path for display, with the home folder as `~`.
pub(in crate::app) fn pretty_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

#[cfg(test)]
mod confirmation_tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        size: Vec2,
        detail: &str,
        events: Vec<egui::Event>,
    ) -> (Option<bool>, egui::accesskit::TreeUpdate) {
        let mut choice = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                choice = confirm_modal(
                    ui.ctx(),
                    "folder-sync-remove",
                    "Remove missing photos?",
                    detail,
                    false,
                    &[("Cancel", false), ("Remove", true)],
                    false,
                );
            },
        );
        output.textures_delta.clear();
        (choice, output.platform_output.accesskit_update.unwrap())
    }

    #[test]
    fn long_removal_confirmation_keeps_title_and_buttons_in_view() {
        for size in [Vec2::new(1280., 720.), Vec2::new(640., 360.)] {
            for count in [1, 30, 100] {
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                let detail = format!(
                    "These photos are removed from the catalog with what they hold:\n{}",
                    (0..count)
                        .map(|i| format!("IMG_{i:04}.NEF: Edited, Keywords, 2 virtual copies"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                // Let the centered modal settle after its first sizing pass.
                for _ in 0..3 {
                    assert_eq!(frame(&ctx, size, &detail, vec![]).0, None);
                }
                let (_, tree) = frame(&ctx, size, &detail, vec![]);
                for label in ["Remove missing photos?", "Cancel", "Remove"] {
                    let node = tree
                        .nodes
                        .iter()
                        .map(|(_, node)| node)
                        // The text and buttons, not the dialog named after the title.
                        .filter(|node| node.role() != egui::accesskit::Role::Dialog)
                        .find(|node| node.label() == Some(label) || node.value() == Some(label))
                        .unwrap_or_else(|| panic!("missing {label}"));
                    let b = node.bounds().expect("placed");
                    assert!(
                        b.x0 >= 0. && b.y0 >= 0. && b.x1 <= size.x as f64 && b.y1 <= size.y as f64,
                        "{label} outside {size:?} with {count} photos: {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn long_confirmation_scrolls_to_the_last_photo_and_buttons_still_work() {
        let size = Vec2::new(640., 360.);
        let detail = (0..100)
            .map(|i| format!("IMG_{i:04}.NEF: Edited, Keywords"))
            .collect::<Vec<_>>()
            .join("\n");
        for (button, expected) in [("Cancel", false), ("Remove", true)] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            for _ in 0..4 {
                assert_eq!(frame(&ctx, size, &detail, vec![]).0, None);
            }
            frame(
                &ctx,
                size,
                &detail,
                vec![egui::Event::PointerMoved(Pos2::new(320., 180.))],
            );
            let events = vec![
                egui::Event::PointerMoved(Pos2::new(320., 180.)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0., -10_000.),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            assert_eq!(frame(&ctx, size, &detail, events).0, None);
            for _ in 0..30 {
                assert_eq!(frame(&ctx, size, &detail, vec![]).0, None);
            }
            let (_, tree) = frame(&ctx, size, &detail, vec![]);
            let last = tree
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.value() == Some("IMG_0099.NEF: Edited, Keywords"))
                .expect("last photo")
                .bounds()
                .expect("placed");
            let action = tree
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(button))
                .expect("action")
                .bounds()
                .expect("placed");
            assert!(last.y0 > 0. && last.y1 < action.y0, "{last:?}");
            let pos = Pos2::new(
                ((action.x0 + action.x1) / 2.) as f32,
                ((action.y0 + action.y1) / 2.) as f32,
            );
            for pressed in [true, false] {
                let choice = frame(
                    &ctx,
                    size,
                    &detail,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                )
                .0;
                assert_eq!(choice, if pressed { None } else { Some(expected) });
            }
        }
    }
}

#[cfg(test)]
mod slider_tests {
    use super::*;

    #[test]
    fn a_slider_from_zero_fills_from_its_left_end_with_no_centre_mark() {
        // Feather, 0–100 with 50 as its default: not a centred slider.
        assert_eq!(
            fill_origin(0., 0.5, false),
            FillOrigin {
                value: 0.,
                tick: Tick::Hidden
            }
        );
        // Exposure, centred on 0.
        assert_eq!(
            fill_origin(-5., 0., false),
            FillOrigin {
                value: 0.,
                tick: Tick::Shown
            }
        );
        // Scale, 50–150% around a neutral 100%, keeps its mark.
        assert_eq!(
            fill_origin(0.5, 1., false),
            FillOrigin {
                value: 1.,
                tick: Tick::Shown
            }
        );
        // Temp keeps its As Shot mark.
        assert_eq!(
            fill_origin(2000., 5500., true),
            FillOrigin {
                value: 5500.,
                tick: Tick::Shown
            }
        );
    }

    /// Frames of Exposure's slider in a 400 × 40 panel: a click on its number
    /// field, then `typed` and Return. Returns the value after.
    fn type_into_exposure(start: f32, typed: &str) -> f32 {
        let ctx = egui::Context::default();
        let mut value = start;
        let field = Pos2::new(400. - SLIDER_VALUE_WIDTH / 2., 12.);
        let frames = [
            vec![egui::Event::PointerMoved(field)],
            vec![egui::Event::PointerButton {
                pos: field,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            vec![egui::Event::PointerButton {
                pos: field,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            vec![egui::Event::Text(typed.into())],
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            vec![],
        ];
        for events in frames {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 40.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            setting_slider(
                                ui,
                                &mut StepName::default(),
                                ParameterId::Exposure,
                                &mut value,
                                0.,
                            );
                        });
                },
            );
            output.textures_delta.clear();
        }
        value
    }

    #[test]
    fn typed_values_take_the_settings_valid_range_beyond_the_rail() {
        // Exposure's rail spans ±5 EV; a recipe may hold ±8.
        assert_eq!(type_into_exposure(0., "6"), 6.);
        assert_eq!(type_into_exposure(0., "-7.5"), -7.5);
        assert_eq!(type_into_exposure(0., "9"), 8.);
        assert_eq!(type_into_exposure(0., "1.25"), 1.25);
    }

    /// Exposure's number field dragged by `dx` points from `start`.
    fn drag_exposure(start: f32, dx: f32) -> f32 {
        let ctx = egui::Context::default();
        let mut value = start;
        let field = Pos2::new(400. - SLIDER_VALUE_WIDTH / 2., 12.);
        let to = field + Vec2::new(dx, 0.);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut frames = vec![
            vec![egui::Event::PointerMoved(field)],
            vec![button(field, true)],
        ];
        for step in 1..=10 {
            frames.push(vec![egui::Event::PointerMoved(
                field + (to - field) * step as f32 / 10.,
            )]);
        }
        frames.push(vec![button(to, false)]);
        for events in frames {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 40.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            setting_slider(
                                ui,
                                &mut StepName::default(),
                                ParameterId::Exposure,
                                &mut value,
                                0.,
                            );
                        });
                },
            );
            output.textures_delta.clear();
        }
        value
    }

    #[test]
    fn dragging_the_number_stays_on_the_rail_and_never_pushes_further_out() {
        assert_eq!(drag_exposure(0., 3000.), 5.);
        assert_eq!(drag_exposure(0., -3000.), -5.);
        // An imported 6.5 EV: dragging further out leaves it; back in moves it.
        assert_eq!(drag_exposure(6.5, 300.), 6.5);
        let back = drag_exposure(6.5, -30.);
        assert!(back < 6.5 && back > 5., "{back}");
    }

    #[test]
    fn stepping_the_number_is_relative_input_typing_is_not() {
        let (rail, typed) = (-5. ..=5., (-8., 8.));
        let relative = |before, entered| {
            number_input(NumberInput::Relative, before, entered, rail.clone(), typed)
        };
        // An arrow key at the rail's end, or on an imported value beyond it, never
        // pushes further out; stepping back in moves it.
        assert_eq!(relative(5., 5.1), 5.);
        assert_eq!(relative(6.5, 6.6), 6.5);
        assert_eq!(relative(6.5, 6.4), 6.4);
        assert_eq!(
            number_input(NumberInput::Typed, 0., 6., rail.clone(), typed),
            6.
        );
        assert_eq!(number_input(NumberInput::Typed, 0., 9., rail, typed), 8.);
    }

    /// Exposure's slider from `start`, with `tabs` Tab presses to move the focus,
    /// then Up pressed `ups` times.
    fn step_exposure(start: f32, tabs: usize, ups: usize) -> f32 {
        let ctx = egui::Context::default();
        let mut value = start;
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let mut frames = vec![vec![]];
        frames.extend((0..tabs).map(|_| vec![key(egui::Key::Tab)]));
        frames.extend((0..ups).map(|_| vec![key(egui::Key::ArrowUp)]));
        frames.push(vec![]);
        for events in frames {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 40.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            setting_slider(
                                ui,
                                &mut StepName::default(),
                                ParameterId::Exposure,
                                &mut value,
                                0.,
                            );
                        });
                },
            );
            output.textures_delta.clear();
        }
        value
    }

    /// Exposure's slider from `start`, stepped up `steps` times by a screen reader's
    /// Increment action on its number.
    fn increment_exposure_assistively(start: f32, steps: usize) -> f32 {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut value = start;
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 40.))),
            events,
            ..Default::default()
        };
        let mut run = |events| {
            let mut output = ctx.run_ui(input(events), |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        setting_slider(
                            ui,
                            &mut StepName::default(),
                            ParameterId::Exposure,
                            &mut value,
                            0.,
                        );
                    });
            });
            output.textures_delta.clear();
            output.platform_output.accesskit_update
        };
        let update = run(vec![]).expect("accessibility is on");
        let (number, _) = update
            .nodes
            .iter()
            .find(|(_, node)| node.role() == egui::accesskit::Role::SpinButton)
            .expect("the number is a spin button");
        let number = *number;
        for _ in 0..steps {
            run(vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Increment,
                    target_tree: egui::accesskit::TreeId::ROOT,
                    target_node: number,
                    data: None,
                },
            )]);
        }
        run(vec![]);
        value
    }

    #[test]
    fn a_screen_reader_steps_the_number_within_the_rail() {
        assert!(increment_exposure_assistively(0., 1) > 0.);
        assert_eq!(increment_exposure_assistively(5., 3), 5.);
        assert_eq!(increment_exposure_assistively(6.5, 3), 6.5);
    }

    #[test]
    fn arrow_keys_on_the_focused_number_step_it_within_the_rail() {
        // Find the Tab press that focuses the number: Up then moves 0 up.
        let tabs = (1..6)
            .find(|&tabs| step_exposure(0., tabs, 1) > 0.)
            .expect("Tab reaches the number field");
        assert_eq!(step_exposure(5., tabs, 3), 5.);
        assert_eq!(step_exposure(6.5, tabs, 3), 6.5);
    }
}

#[cfg(test)]
mod segment_tests {
    use super::*;

    #[test]
    fn segments_narrow_their_padding_rather_than_outgrow_their_place() {
        // The Navigator's zoom levels in a narrow panel once grew past it,
        // where the panel was no longer painted.
        let options = [
            (0, "Fit"),
            (1, "50%"),
            (2, "100%"),
            (3, "200%"),
            (4, "400%"),
        ];
        let ctx = egui::Context::default();
        let mut drawn = None;
        let mut place = 0.;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
            let at_min_padding = label_widths(ui, &labels, COMPACT_SEGMENTS.font - 1.)
                .iter()
                .map(|w| w + 2. * SEGMENT_MIN_PADDING)
                .sum::<f32>()
                + 2. * SEGMENT_INSET;
            // Narrower than the minimum padding allows, wider than the tightest.
            place = at_min_padding - 10.;
            assert!(place >= tightest_segments_width(ui, &labels, &COMPACT_SEGMENTS));
            ui.allocate_ui(Vec2::new(place, 40.), |ui| {
                ui.set_max_width(place);
                let mut value = 0;
                let before = ui.cursor().left();
                segmented(ui, &mut value, &options, place);
                drawn = Some(ui.min_rect().right() - before);
            });
        });
        output.textures_delta.clear();
        let drawn = drawn.expect("drawn");
        assert!(drawn <= place + 0.5, "{drawn} wide in {place}");
    }
}

/// A spinner `size` points across with a hairline stroke: egui's draws three
/// points thick at any size, heavy beside the interface's text.
pub(in crate::app) fn spinner(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_spinner(ui, rect, None);
    response
}
/// [`spinner`] painted into `rect`, in `color` or the interface's grey.
pub(in crate::app) fn paint_spinner(ui: &egui::Ui, rect: Rect, color: Option<Color32>) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx().request_repaint();
    let radius = rect.width().min(rect.height()) / 2. - 1.;
    let time = ui.input(|i| i.time);
    // egui's sweep: a turn a second, the arc growing and shrinking.
    let start = time * std::f64::consts::TAU;
    let end = start + 240f64.to_radians() * time.sin();
    let points: Vec<Pos2> = (0..=24)
        .map(|k| {
            let angle = start + (end - start) * k as f64 / 24.;
            let (sin, cos) = angle.sin_cos();
            rect.center() + radius * Vec2::new(cos as f32, sin as f32)
        })
        .collect();
    let color = color.unwrap_or_else(|| theme::palette(ui.ctx()).tone(Tone::LABEL));
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(1.5, color)));
}

/// The slider grid's label column: captions end 5 pt short of it, slider
/// rails and controls start 5 pt past it.
const LABEL_COLUMN: f32 = 83.;
fn label_column_id() -> egui::Id {
    egui::Id::new("rawmakase-label-column")
}
/// The label column's width where `ui` draws: [`LABEL_COLUMN`], or a
/// narrower one a [`LabelColumn`] set.
pub(in crate::app) fn label_column(ui: &egui::Ui) -> f32 {
    ui.ctx()
        .data(|d| d.get_temp::<f32>(label_column_id()))
        .unwrap_or(LABEL_COLUMN)
}
/// Between the label column and what is beside it: captions end this far
/// short of it, slider rails and controls start this far past it.
pub(in crate::app) const LABEL_GAP: f32 = 5.;
/// Where a row's slider rail or control starts, from the row's left.
pub(in crate::app) fn control_left(ui: &egui::Ui) -> f32 {
    label_column(ui) + LABEL_GAP
}
/// While alive, the slider grid's label column is `width` wide: Develop's
/// tool drawers, whose labels are short, give the controls the room.
pub(in crate::app) struct LabelColumn {
    ctx: egui::Context,
    before: Option<f32>,
}
impl LabelColumn {
    pub(in crate::app) fn enter(ui: &egui::Ui, width: f32) -> Self {
        let ctx = ui.ctx().clone();
        let before = ctx.data_mut(|d| {
            let before = d.get_temp::<f32>(label_column_id());
            d.insert_temp(label_column_id(), width);
            before
        });
        Self { ctx, before }
    }
}
impl Drop for LabelColumn {
    fn drop(&mut self) {
        self.ctx.data_mut(|d| match self.before {
            Some(width) => {
                d.insert_temp(label_column_id(), width);
            }
            None => d.remove::<f32>(label_column_id()),
        });
    }
}

/// The caption band at the top of Develop's tab panel, naming what it shows
/// as the panels' section headers do. Returns the band, for what its right
/// end holds.
pub(in crate::app) fn panel_caption(ui: &mut egui::Ui, name: &str) -> Rect {
    let palette = theme::palette(ui.ctx());
    ui.add_space(inset::SECTION);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), size::HEADER),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, radius::CONTROL, palette.tone(Tone::CARD));
    ui.painter().text(
        Pos2::new(rect.left() + 8., rect.center().y),
        egui::Align2::LEFT_CENTER,
        name.to_uppercase(),
        egui::FontId::proportional(text::SMALL),
        palette.tone(Tone::MUTED),
    );
    rect
}
/// How far a [`text_box`]'s text starts inside it.
pub(in crate::app) const TEXT_BOX_INSET: f32 = space::M;
/// A text box: a rounded, outlined box `rows` lines tall that wraps what is
/// typed, as [`search_field`]'s box looks, lit while it has the keyboard.
/// `hint` shows while it is empty; at most `limit` characters. Returns the
/// text field's response.
pub(in crate::app) fn text_box(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    rows: usize,
    limit: usize,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    // The box goes under the text, its look known once the field is drawn.
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(crate::app::ui::design::margin(TEXT_BOX_INSET, space::S))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .hint_text(hint)
                    .frame(egui::Frame::new())
                    .font(egui::FontId::proportional(text::LABEL))
                    .desired_rows(rows)
                    .char_limit(limit)
                    .desired_width(ui.available_width()),
            )
        });
    let response = inner.inner;
    let focused = response.has_focus();
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::new(
            inner.response.rect,
            6.,
            palette.tone(if focused { Tone::BAR } else { Tone::CARD }),
            Stroke::new(
                1.,
                if focused {
                    palette.accent()
                } else {
                    palette.tone(Tone::RAISED)
                },
            ),
            egui::StrokeKind::Inside,
        ),
    );
    response
}
/// A search field: a rounded box with a magnifier, the text and a button that
/// clears it. Returns the text field's response.
pub(in crate::app) fn search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    search_field_with_id(ui, text, hint, None)
}
/// A [`search_field`] whose text field has id `id`, for code that asks egui
/// about its focus before it is drawn.
pub(in crate::app) fn search_field_with_id(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    id: Option<egui::Id>,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    let field = rect
        .with_min_x(rect.left() + 28.)
        .with_max_x(rect.right() - 26.);
    // The box goes under the text, its look known once the field is drawn.
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(field)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                let field = egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .frame(egui::Frame::new())
                    .font(egui::FontId::proportional(text::LABEL))
                    .desired_width(field.width());
                named(
                    ui.add(match id {
                        Some(id) => field.id(id),
                        None => field,
                    }),
                    hint,
                )
            },
        )
        .inner;
    let focused = response.has_focus();
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::new(
            rect,
            6.,
            palette.tone(if focused { Tone::BAR } else { Tone::CARD }),
            Stroke::new(
                1.,
                if focused {
                    palette.accent()
                } else {
                    palette.tone(Tone::RAISED)
                },
            ),
            egui::StrokeKind::Inside,
        ),
    );
    icons::paint_at(
        ui.painter(),
        Icon::Search,
        Pos2::new(rect.left() + 15., rect.center().y),
        13.,
        palette.tone(Tone::FAINT),
    );
    if !text.is_empty() {
        let clear = Rect::from_center_size(
            Pos2::new(rect.right() - 14., rect.center().y),
            Vec2::splat(20.),
        );
        let hit = ui
            .interact(clear, response.id.with("clear"), Sense::click())
            .on_hover_text("Clear the search")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let tint = palette.tone(if hit.hovered() {
            Tone::TEXT
        } else {
            Tone::MUTED
        });
        icons::paint_at(ui.painter(), Icon::Close, clear.center(), 11., tint);
        if hit.clicked() {
            text.clear();
        }
    }
    response
}

#[cfg(test)]
#[path = "widgets_editor_tests.rs"]
mod editor_tests;

#[cfg(test)]
mod dropdown_layout_tests {
    use super::*;
    use egui::accesskit::{Rect, Role};

    /// In a [`dropdown_row`] the label sits on the drop-down's middle and a
    /// button is as tall as the drop-down; after the row, and after a
    /// drop-down drawn in [`keep_style`], widgets keep their own height.
    #[test]
    fn dropdown_rows_line_up_and_leave_the_style_as_they_found_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut choice = 0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = ui.button("Reference");
            dropdown_row(ui, |ui| {
                ui.label("Deghost");
                dropdown(ui, "row")
                    .selected_text("Low")
                    .show_ui(ui, |ui| ui.selectable_value(&mut choice, 1, "High"));
                let _ = ui.button("In the row");
            });
            let _ = ui.button("After the row");
            ui.horizontal(|ui| {
                keep_style(ui, |ui| {
                    dropdown(ui, "kept")
                        .selected_text("Medium")
                        .show_ui(ui, |ui| ui.selectable_value(&mut choice, 2, "High"));
                });
                let _ = ui.button("After the drop-down");
            });
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("on");
        let node = |role: Role, name: &str| {
            update
                .nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.role() == role && (n.label() == Some(name) || n.value() == Some(name)))
                .and_then(|n| n.bounds())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let middle = |r: Rect| (r.y0 + r.y1) / 2.;
        let height = |r: Rect| r.y1 - r.y0;
        let near = |a: f64, b: f64| (a - b).abs() <= 1.;
        let (label, combo) = (node(Role::Label, "Deghost"), node(Role::ComboBox, "Low"));
        let plain = height(node(Role::Button, "Reference"));
        assert!(near(middle(label), middle(combo)), "{label:?} {combo:?}");
        assert!(near(height(combo), DROPDOWN_HEIGHT.into()), "{combo:?}");
        let in_row = node(Role::Button, "In the row");
        assert!(near(height(in_row), height(combo)), "{in_row:?}");
        for after in ["After the row", "After the drop-down"] {
            let button = node(Role::Button, after);
            assert!(near(height(button), plain), "{after}: {button:?}");
        }
    }
}
