use crate::app::icons::{self, Icon};
use crate::app::theme;
use crate::develop::effects::EffectsRendering;
use crate::model::panels::PanelState;
use crate::model::params::{self, LocalParameterId, ParameterId, format_value};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

pub(super) fn toolbar_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12., 32.), Sense::hover());
    ui.painter().line_segment(
        [
            rect.center_top() + Vec2::new(0., 6.),
            rect.center_bottom() - Vec2::new(0., 6.),
        ],
        Stroke::new(1., theme::palette(ui.ctx()).gray(53)),
    );
}
pub(super) fn toolbar_action(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    selected: bool,
    enabled: bool,
    icon: u8,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, 32.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hover = enabled && response.hovered();
    let fill = palette.gray(if selected {
        62
    } else if hover {
        48
    } else {
        29
    });
    ui.painter().rect_filled(rect, 5., fill);
    let color = palette.gray(if !enabled {
        85
    } else if selected || hover {
        235
    } else {
        175
    });
    if !label.is_empty() {
        ui.painter().text(
            rect.center() + Vec2::new(if icon > 0 { 8. } else { 0. }, 0.),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.),
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
pub(super) fn adjustment_section(
    ui: &mut egui::Ui,
    title: &str,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    section(ui, title, true, contents)
}
/// Where the set of collapsed section titles lives in egui memory; the editor
/// seeds it from the session and saves it back when it changes.
pub(super) fn collapsed_sections_id() -> egui::Id {
    egui::Id::new("rawmakase-collapsed-sections")
}
/// Where a control names the edit it just made, for the History panel. The
/// editor takes it at the end of the frame.
pub(super) fn history_step_id() -> egui::Id {
    egui::Id::new("rawmakase-history-step")
}
pub(super) fn name_history_step(ui: &egui::Ui, name: String, value: String) {
    name_frame_step(ui.ctx(), name, value);
}
/// As [`name_history_step`], for an edit made outside a control during the frame.
pub(super) fn name_frame_step(ctx: &egui::Context, name: String, value: String) {
    ctx.data_mut(|d| d.insert_temp(history_step_id(), (name, value)));
}
/// The panel or sub-panel being drawn ("Detail", then "Sharpening"), so a
/// slider's step reads "Sharpening Amount" rather than "Amount".
pub(super) fn set_edit_context(ui: &egui::Ui, title: &str) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("rawmakase-edit-context"), title.to_string()));
}
fn edit_context(ui: &egui::Ui) -> String {
    ui.ctx()
        .data(|d| d.get_temp(egui::Id::new("rawmakase-edit-context")))
        .unwrap_or_default()
}
/// Collapsible Lightroom-style panel header; returns whether reset was clicked.
pub(super) fn section(
    ui: &mut egui::Ui,
    title: &str,
    resettable: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let button = if resettable {
        HeaderButton::Reset
    } else {
        HeaderButton::None
    };
    section_with(ui, title, button, contents)
}
/// The button at the right of a panel header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeaderButton {
    None,
    /// Resets the panel (and names the History step).
    Reset,
    /// Adds an item, as the Snapshots panel's +.
    Add,
}
/// [`section`] with any header button; returns whether it was clicked.
pub(super) fn section_with(
    ui: &mut egui::Ui,
    title: &str,
    button: HeaderButton,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    section_header(ui, title, button, None, contents)
}
/// A resettable Develop panel with Lightroom's on/off switch in its header, over
/// the photo's `Enable*` setting; returns whether reset was clicked.
pub(super) fn switched_section(
    ui: &mut egui::Ui,
    title: &str,
    switch: &mut PanelState,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    section_header(ui, title, HeaderButton::Reset, Some(switch), contents)
}
/// The side of the window a panel is in. Solo Mode is per side, as in Lightroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SectionGroup {
    DevelopLeft,
    DevelopRight,
    LibraryLeft,
    LibraryRight,
}
impl SectionGroup {
    /// A stable name, as saved in the session.
    pub(super) fn key(self) -> &'static str {
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
pub(super) fn solo_sections_id() -> egui::Id {
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
pub(super) struct SectionSide {
    ctx: egui::Context,
}
impl SectionSide {
    pub(super) fn enter(ui: &egui::Ui, group: SectionGroup) -> Self {
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
    contents: impl FnOnce(&mut egui::Ui),
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
    ui.add_space(8.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    let reset_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 16., rect.center().y),
        Vec2::splat(24.),
    );
    let right = if resettable {
        reset_rect.left()
    } else {
        rect.right()
    };
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
        HeaderButton::Add => reset.on_hover_text(format!("New {}", title.trim_end_matches('s'))),
        HeaderButton::None => reset,
    };
    let enabled = switch.as_deref().is_none_or(|s| *s == PanelState::On);
    // A filled header band marks each collapsible panel, as in Lightroom.
    ui.painter().rect_filled(
        rect,
        3.,
        palette.gray(if toggle.hovered() { 60 } else { 51 }),
    );
    ui.painter().rect_stroke(
        rect,
        3.,
        Stroke::new(1., palette.gray(if open { 70 } else { 62 })),
        egui::StrokeKind::Inside,
    );
    let c = Pos2::new(rect.left() + 13., rect.center().y);
    let triangle = if open {
        vec![
            c + Vec2::new(-4., -2.),
            c + Vec2::new(4., -2.),
            c + Vec2::new(0., 3.),
        ]
    } else {
        vec![
            c + Vec2::new(-2., -4.),
            c + Vec2::new(3., 0.),
            c + Vec2::new(-2., 4.),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        triangle,
        palette.gray(if toggle.hovered() { 235 } else { 190 }),
        Stroke::NONE,
    ));
    ui.painter().text(
        Pos2::new(rect.left() + 26., rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.),
        palette.gray(match (enabled, toggle.hovered()) {
            (false, _) => 140,
            (true, true) => 250,
            (true, false) => 235,
        }),
    );
    if resettable {
        let color = palette.gray(if reset.hovered() { 240 } else { 150 });
        let icon = if button == HeaderButton::Add {
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
            if menu_item(ui, "Solo Mode", "", true, solo(ui, group)) {
                toggle_solo(ui, group, key);
                ui.close();
            }
        });
    }
    if toggle.clicked() && !context_clicked(&toggle) {
        open = !open;
        set_open(ui, key, open);
        if open && let Some(group) = group.filter(|g| solo(ui, g)) {
            close_others(ui, group, key);
        }
    }
    if button == HeaderButton::Reset && reset.clicked() {
        name_history_step(ui, format!("Reset {title}"), String::new());
    }
    if open {
        set_edit_context(ui, title);
        // Scope ids per section so equal slider labels (e.g. two "Amount"s) never clash.
        ui.push_id(title, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 8,
                    bottom: 12,
                })
                .show(ui, contents);
        });
    }
    resettable && reset.clicked()
}
/// Lightroom's panel switch: a small track with its knob to the right when on.
fn paint_switch(painter: &egui::Painter, c: Pos2, state: PanelState, hovered: bool) {
    let palette = theme::palette(painter.ctx());
    let track = Rect::from_center_size(c, Vec2::new(20., 10.));
    let on = state == PanelState::On;
    painter.rect_filled(
        track,
        5.,
        palette.gray(match (on, hovered) {
            (true, true) => 175,
            (true, false) => 150,
            (false, true) => 80,
            (false, false) => 34,
        }),
    );
    painter.rect_stroke(
        track,
        5.,
        Stroke::new(1., palette.gray(if on { 120 } else { 95 })),
        egui::StrokeKind::Inside,
    );
    let knob = if on {
        track.right_center() - Vec2::new(5., 0.)
    } else {
        track.left_center() + Vec2::new(5., 0.)
    };
    painter.circle_filled(knob, 3.5, palette.gray(if on { 235 } else { 140 }));
}
#[derive(Clone, Default)]
struct CurveInteraction {
    selected: Option<usize>,
    dragging: Option<usize>,
}
/// Lightroom's curve backdrop: mid-gray field, the image histogram behind the
/// curve, a quarter grid and a dark frame. `channel` 0 is RGB, 1–3 are R, G, B.
fn curve_backdrop(ui: &egui::Ui, rect: Rect, histogram: &[[u32; 256]; 3], channel: usize) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, 0., palette.gray(82));
    let bins: Vec<f32> = (0..256)
        .map(|i| match channel {
            1..=3 => histogram[channel - 1][i] as f32,
            _ => histogram.iter().map(|h| h[i] as f32).sum::<f32>(),
        })
        .collect();
    // Ignore the extreme bins when scaling so clipped pixels don't flatten the rest.
    let max = bins[1..255].iter().copied().fold(1., f32::max);
    let fill = match channel {
        1 => Color32::from_rgb(112, 62, 60),
        2 => Color32::from_rgb(62, 102, 66),
        3 => Color32::from_rgb(62, 78, 118),
        _ => palette.gray(58),
    };
    if bins.iter().any(|v| *v > 0.) {
        let mut mesh = egui::Mesh::default();
        for (i, v) in bins.iter().enumerate() {
            let x = rect.left() + i as f32 / 255. * rect.width();
            let h = ((v / max).sqrt()).min(1.) * rect.height() * 0.92;
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(x, rect.bottom()), fill);
            mesh.colored_vertex(Pos2::new(x, rect.bottom() - h), fill);
            if i > 0 {
                mesh.add_triangle(base - 2, base - 1, base);
                mesh.add_triangle(base - 1, base, base + 1);
            }
        }
        painter.add(mesh);
    }
    for i in 1..4 {
        let t = i as f32 / 4.;
        let x = rect.left() + t * rect.width();
        let y = rect.bottom() - t * rect.height();
        let grid = Stroke::new(1., Color32::from_rgba_unmultiplied(160, 160, 160, 60));
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
        Stroke::new(1., Color32::from_rgba_unmultiplied(200, 200, 200, 45)),
    );
    painter.rect_stroke(
        rect,
        0.,
        Stroke::new(1., palette.gray(15)),
        egui::StrokeKind::Outside,
    );
}
fn curve_color(palette: &theme::Palette, channel: usize) -> Color32 {
    match channel {
        1 => Color32::from_rgb(240, 110, 100),
        2 => Color32::from_rgb(120, 215, 125),
        3 => Color32::from_rgb(120, 160, 245),
        _ => palette.gray(255),
    }
}
/// Read-out under a curve: the input and output values at the pointer.
fn curve_readout(ui: &mut egui::Ui, value: Option<[f32; 2]>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
    if let Some([x, y]) = value {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{:.0} / {:.0}", x * 255., y * 255.),
            egui::FontId::proportional(11.),
            theme::palette(ui.ctx()).gray(170),
        );
    }
}
/// Parametric curve: dragging up or down in the graph changes the region
/// under the pointer, and the three handles below move the region splits.
/// The parametric curve; `targeted` is the region a Targeted Adjustment Tool drag is
/// moving, which shows as a hovered one does.
pub(super) fn parametric_curve_ui(
    ui: &mut egui::Ui,
    effects: &mut crate::model::effects::Effects,
    histogram: &[[u32; 256]; 3],
    targeted: Option<usize>,
) {
    let palette = theme::palette(ui.ctx());
    let size = ui.available_width();
    let (outer, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let rect = outer.shrink(4.);
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
        .add(egui::Shape::line(pts, Stroke::new(2., palette.gray(255))));
    let name = hovered_region.map(|i| ["Shadows", "Darks", "Lights", "Highlights"][i]);
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
        palette.gray(60),
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
        let color = palette.gray(if r.hovered() || r.dragged() { 240 } else { 175 });
        let top = Pos2::new(x, track.top() + 3.);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![top, top + Vec2::new(5., 9.), top + Vec2::new(-5., 9.)],
            color,
            Stroke::NONE,
        ));
    }
}
pub(super) fn tone_curve_ui(
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
    let rect = outer.shrink(4.);
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
    let pts = (0..=256)
        .map(|i| {
            let x = i as f32 / 256.;
            screen([x, plot.evaluate(x)])
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        pts,
        Stroke::new(2., curve_color(&palette, channel)),
    ));
    for (i, p) in curve.points.iter().enumerate() {
        let p = screen(*p);
        let active = state.selected == Some(i) || hit == Some(i);
        ui.painter().circle_filled(
            p,
            if active { 5.5 } else { 4.5 },
            if state.selected == Some(i) {
                palette.gray(25)
            } else {
                palette.gray(255)
            },
        );
        ui.painter().circle_stroke(
            p,
            if active { 5.5 } else { 4.5 },
            Stroke::new(
                1.5,
                if state.selected == Some(i) {
                    palette.gray(255)
                } else {
                    palette.gray(20)
                },
            ),
        );
    }
    let readout = state.dragging.map(|i| curve.points[i]).or_else(|| {
        pointer.filter(|p| rect.contains(*p)).map(|p| {
            let [x, _] = value(p);
            [x, plot.evaluate(x)]
        })
    });
    response.on_hover_cursor(if state.dragging.is_some() {
        egui::CursorIcon::Grabbing
    } else if hit.is_some() {
        egui::CursorIcon::Grab
    } else {
        egui::CursorIcon::Crosshair
    });
    curve_readout(ui, readout);
    if let Some(i) = state.selected {
        let mut point = curve.points[i].map(|v| v * 255.);
        let changed = ui
            .horizontal(|ui| {
                ui.label("Input");
                let x = ui
                    .add(
                        egui::DragValue::new(&mut point[0])
                            .range(0. ..=255.)
                            .update_while_editing(false)
                            .speed(1.)
                            .fixed_decimals(0),
                    )
                    .changed();
                ui.label("Output");
                let y = ui
                    .add(
                        egui::DragValue::new(&mut point[1])
                            .range(0. ..=255.)
                            .update_while_editing(false)
                            .speed(1.)
                            .fixed_decimals(0),
                    )
                    .changed();
                x || y
            })
            .inner;
        if changed {
            curve.move_point(i, point.map(|v| v / 255.));
        }
    }
    ui.label(
        egui::RichText::new("Click to add · drag to shape · right-click to remove")
            .size(10.)
            .color(palette.gray(125)),
    );
    ui.ctx().data_mut(|d| d.insert_temp(id, state));
}
/// Width of a slider's number field at the right of its row.
const SLIDER_VALUE_WIDTH: f32 = 52.;
/// Lightroom-style slider. Normalized ranges within ±1 display as integers
/// (×100), matching Lightroom's numbers while the recipe keeps its units.
pub(super) fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
) {
    slider_with(ui, label, value, range, default, None, None);
}
/// What happened to a slider this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SliderEvent {
    None,
    /// Double-clicked back to its default.
    Reset,
}
/// `display` overrides the shown scale and decimals, e.g. Sharpening's 0–150.
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
/// zero, a coloured one like Temp, or one with a neutral point like Levels' Midtone
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
pub(super) fn slider_with(
    ui: &mut egui::Ui,
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
    slider_styled(ui, label, value, range, default, style)
}
/// How a slider looks and moves beyond its label, range and default.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SliderStyle {
    /// Shown scale and decimals, e.g. Sharpening's 0–150; picked from the range
    /// when `None`.
    pub display: Option<(f32, usize)>,
    /// A coloured rail, from its left end to its right.
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
}
/// The Temp slider's rail, blue to yellow.
pub(super) const TEMPERATURE_GRADIENT: (Color32, Color32) = (
    Color32::from_rgb(74, 123, 182),
    Color32::from_rgb(194, 169, 93),
);
/// The Tint slider's rail, green to magenta.
pub(super) const TINT_GRADIENT: (Color32, Color32) = (
    Color32::from_rgb(91, 156, 112),
    Color32::from_rgb(165, 105, 158),
);
/// The slider for a develop setting, as its descriptor describes it.
pub(super) fn setting_slider(
    ui: &mut egui::Ui,
    id: ParameterId,
    value: &mut f32,
    default: f32,
) -> SliderEvent {
    let gradient = match id {
        ParameterId::Temperature => Some(TEMPERATURE_GRADIENT),
        ParameterId::Tint | ParameterId::ShadowTint => Some(TINT_GRADIENT),
        _ => None,
    };
    descriptor_slider(ui, id.descriptor(), value, default, gradient)
}
/// The slider for a mask's adjustment, as its descriptor describes it.
pub(super) fn local_setting_slider(
    ui: &mut egui::Ui,
    id: LocalParameterId,
    value: &mut f32,
    default: f32,
) -> SliderEvent {
    let gradient = match id {
        LocalParameterId::Temperature => Some(TEMPERATURE_GRADIENT),
        LocalParameterId::Tint => Some(TINT_GRADIENT),
        _ => None,
    };
    descriptor_slider(ui, id.descriptor(), value, default, gradient)
}
fn descriptor_slider<Id>(
    ui: &mut egui::Ui,
    d: &params::Descriptor<Id>,
    value: &mut f32,
    default: f32,
    gradient: Option<(Color32, Color32)>,
) -> SliderEvent {
    let style = SliderStyle {
        display: Some((d.display.scale, d.display.decimals)),
        gradient,
        reciprocal: matches!(d.tick, params::Tick::Mireds(_)),
        drag_step: d.drag_step,
        hue_rail: false,
        typed: Some((*d.valid.start(), *d.valid.end())),
    };
    slider_styled(ui, d.label, value, d.interactive.clone(), default, style)
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
pub(super) fn slider_styled(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    style: SliderStyle,
) -> SliderEvent {
    let palette = theme::palette(ui.ctx());
    let SliderStyle {
        display,
        gradient,
        reciprocal,
        drag_step: step,
        hue_rail,
        typed,
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
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
        let label_rect = Rect::from_min_max(row.min, Pos2::new(row.left() + 83., row.bottom()));
        let label_response = ui.interact(label_rect, ui.id().with("label"), Sense::click());
        ui.painter().text(
            label_rect.right_center() - Vec2::new(5., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(11.),
            palette.gray(190),
        );
        if label_response.double_clicked() {
            *value = default.clamp(start, end);
            event = SliderEvent::Reset;
        }
        let value_rect = Rect::from_min_max(
            Pos2::new(row.right() - SLIDER_VALUE_WIDTH, row.top()),
            row.max,
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
            .scope_builder(egui::UiBuilder::new().max_rect(value_rect), |ui| {
                ui.set_clip_rect(value_rect.intersect(ui.clip_rect()));
                let spacing = ui.spacing_mut();
                spacing.interact_size.x = SLIDER_VALUE_WIDTH;
                spacing.button_padding.x = 4.;
                ui.place(
                    value_rect,
                    // An imported value beyond the range shows as it is, rather
                    // than being clamped by drawing it.
                    egui::DragValue::new(&mut displayed)
                        .range(typed_low * scale..=typed_high * scale)
                        .clamp_existing_to_range(false)
                        .speed(span * scale / 500.)
                        .custom_formatter(move |v, _| format_value(v, decimals, signed))
                        .custom_parser(|s| s.trim().trim_start_matches('+').parse().ok()),
                )
            })
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
        let area = Rect::from_min_max(
            Pos2::new(label_rect.right(), row.top()),
            Pos2::new(value_rect.left() - 4., row.bottom()),
        );
        let response = ui.interact(area, ui.id().with("rail"), Sense::click_and_drag());
        let rail = Rect::from_min_max(
            Pos2::new(area.left() + 5., area.center().y - 1.),
            Pos2::new(area.right() - 5., area.center().y + 1.),
        );
        if hue_rail {
            let mut mesh = egui::Mesh::default();
            for sector in 0..6 {
                let left = egui::lerp(rail.left()..=rail.right(), sector as f32 / 6.);
                let right = egui::lerp(rail.left()..=rail.right(), (sector + 1) as f32 / 6.);
                let color = |h| {
                    let c = crate::develop::color::hue_rgb(h);
                    Color32::from_rgb(
                        (c[0] * 180.) as u8,
                        (c[1] * 180.) as u8,
                        (c[2] * 180.) as u8,
                    )
                };
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
            ui.painter().rect_filled(rail, 1., palette.gray(83));
        }
        // A tick where a centred slider rests, or a coloured one's default (Temp's As
        // Shot); a slider from zero up, like Feather, fills from its left end.
        let neutral = to_rail(origin.value, rail);
        if origin.tick == Tick::Shown {
            ui.painter().line_segment(
                [
                    Pos2::new(neutral, area.center().y - 4.),
                    Pos2::new(neutral, area.center().y + 4.),
                ],
                Stroke::new(1., palette.gray(115)),
            );
        }
        if response.double_clicked() {
            *value = default.clamp(start, end);
            event = SliderEvent::Reset;
        } else if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            let v = from_rail(p.x, rail);
            let v = step.map_or(v, |step| (v / step).round() * step);
            *value = v.clamp(start, end);
        } else if let Some(nudge) = hovered_nudge(ui, row) {
            // Lightroom's keys over a hovered slider: Up and Down move it by its
            // smallest shown step, ten with Shift.
            let unit = step.unwrap_or(10f32.powi(-(decimals as i32)) / scale);
            *value = crate::model::params::nudged(*value, nudge * unit, start..=end);
        }
        let x = to_rail(*value, rail);
        if gradient.is_none() {
            ui.painter().line_segment(
                [
                    Pos2::new(neutral, area.center().y),
                    Pos2::new(x, area.center().y),
                ],
                Stroke::new(2., palette.gray(153)),
            );
        }
        let center = Pos2::new(x, area.center().y);
        ui.painter().circle_filled(
            center,
            if response.hovered() || response.dragged() {
                4.5
            } else {
                3.5
            },
            palette.gray(205),
        );
        ui.painter()
            .circle_stroke(center, 3.5, Stroke::new(1., palette.gray(26)));
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
        name_history_step(ui, name, shown);
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
pub(super) struct SegmentStyle {
    height: f32,
    font: f32,
}
/// The Library/Develop switcher in the top bar.
pub(super) const TOP_BAR_SEGMENTS: SegmentStyle = SegmentStyle {
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
pub(super) const COMPACT_SEGMENT_HEIGHT: f32 = 22.;
/// Panel and filter controls.
const COMPACT_SEGMENTS: SegmentStyle = SegmentStyle {
    height: COMPACT_SEGMENT_HEIGHT,
    font: 11.,
};
/// Segmented control sized to its labels: equal-width segments, so switching
/// never moves a label. `selected` None shows no active segment.
pub(super) fn segment_bar<const N: usize>(
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
const SEGMENT_PADDING: f32 = 12.;
const SEGMENT_MIN_PADDING: f32 = 8.;
/// What the padding gives way to, at the smaller label size, before the
/// control grows wider than its place (and past the panel holding it).
const SEGMENT_TIGHT_PADDING: f32 = 3.;
/// The pill's gap to the track.
const SEGMENT_INSET: f32 = 2.;
fn label_widths(ui: &egui::Ui, labels: &[&str], font: f32) -> Vec<f32> {
    let white = theme::palette(ui.ctx()).gray(255);
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
    ui.painter().rect_filled(track, radius, palette.gray(21));
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
            ui.painter()
                .rect_filled(segment(index as f32), radius - inset, palette.gray(30));
        }
    }
    if selected.is_some() {
        ui.painter().rect(
            segment(position),
            radius - inset,
            palette.gray(44),
            Stroke::new(1., palette.gray(58)),
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
            palette.gray(140).lerp_to_gamma(palette.gray(245), weight)
        } else {
            palette.gray(70)
        };
        let text = ui.painter().layout_no_wrap(
            (*label).into(),
            egui::FontId::proportional(font),
            palette.gray(255),
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
/// What an [`action_button`] does: the main action of its area, or any other.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ButtonKind {
    Primary,
    Secondary,
}
/// A toolbar button sized to its content: an optional icon and the label
/// centred together, with the segmented controls' height and rounding.
pub(super) fn action_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: Option<Icon>,
    kind: ButtonKind,
    enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let height = TOOLBAR_SEGMENTS.height;
    let (padding, icon_size, gap) = (12., 14., 6.);
    let text = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::proportional(TOOLBAR_SEGMENTS.font),
        palette.gray(255),
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
    let (fill, stroke, ink) = match (kind, enabled) {
        (_, false) => (palette.gray(34), palette.gray(44), palette.gray(85)),
        (ButtonKind::Primary, true) => (
            if hovered && !pressed {
                palette.accent_hover()
            } else {
                palette.accent()
            },
            Color32::TRANSPARENT,
            palette.on_accent(),
        ),
        (ButtonKind::Secondary, true) => (
            palette.gray(if pressed {
                36
            } else if hovered {
                50
            } else {
                42
            }),
            palette.gray(60),
            palette.gray(225),
        ),
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
/// Compact segmented control for panels and filters: at least `width` wide
/// with roomy segments, but never wider than the space left in the panel;
/// there segments shrink to their labels, then the text a point, so labels
/// stay whole with at least 8 px padding.
pub(super) fn segmented<T: PartialEq + Copy>(
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
/// A full-width menu row: optional check mark, label, right-aligned shortcut.
pub(super) fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
    checked: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 3., palette.accent());
    }
    let color = if enabled && response.hovered() {
        palette.on_accent_text(250)
    } else {
        palette.gray(if enabled { 215 } else { 100 })
    };
    if checked {
        let c = rect.left_center() + Vec2::new(11., 0.);
        icons::paint_at(ui.painter(), Icon::Check, c, 13., color);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(22., 0.),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.),
        color,
    );
    if !shortcut.is_empty() {
        ui.painter().text(
            rect.right_center() - Vec2::new(10., 0.),
            egui::Align2::RIGHT_CENTER,
            shortcut,
            egui::FontId::proportional(11.),
            palette.gray(if enabled { 140 } else { 90 }),
        );
    }
    enabled && response.clicked()
}
pub(super) fn menu_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1., theme::palette(ui.ctx()).gray(55)),
    );
}
/// True for a context-menu click: a right click, or Control-click on macOS.
pub(super) fn context_clicked(response: &egui::Response) -> bool {
    response.secondary_clicked()
        || (cfg!(target_os = "macos")
            && response.clicked()
            && response.ctx.input(|i| i.modifiers.ctrl))
}
/// Like `Response::context_menu`, but Control-click also opens it on macOS.
pub(super) fn context_menu(response: &egui::Response, add: impl FnOnce(&mut egui::Ui)) {
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
/// Makes egui submenu buttons match `menu_item` rows: same height, text
/// inset and hover color.
pub(super) fn submenu_style(ui: &mut egui::Ui) {
    let palette = theme::palette(ui.ctx());
    let spacing = ui.spacing_mut();
    spacing.item_spacing.y = 0.;
    spacing.button_padding = Vec2::new(22., 4.);
    spacing.interact_size.y = 24.;
    let visuals = ui.visuals_mut();
    for widget in [
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.open,
        &mut visuals.widgets.active,
    ] {
        widget.weak_bg_fill = palette.accent();
        widget.bg_fill = palette.accent();
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1., palette.on_accent_text(250));
    }
    visuals.widgets.inactive.fg_stroke = Stroke::new(1., palette.gray(215));
}
/// The frame of a modal window such as Preferences or Export.
pub(super) fn modal_frame(palette: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.gray(33))
        .stroke(Stroke::new(1., palette.gray(52)))
        .corner_radius(10.)
}
/// The button that confirms a modal window ("Done", "Export").
pub(super) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(palette.on_accent()))
            .fill(palette.accent())
            .min_size(Vec2::new(84., 30.)),
    )
}
/// A modal window asking before an action: a title, a line of detail and
/// `buttons`, the last one primary. Returns the choice clicked, `dismissed`
/// when the window is closed, or None while it is open. Return confirms
/// nothing unless a button has focus. A `truncated` detail stays on one line,
/// e.g. a path.
pub(super) fn confirm_modal<T: Copy>(
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
        .frame(modal_frame(&palette).inner_margin(24))
        .show(ctx, |ui| {
            ui.set_width(420.);
            ui.label(
                egui::RichText::new(title)
                    .size(15.)
                    .color(palette.gray(236)),
            );
            ui.add_space(6.);
            let detail = egui::Label::new(
                egui::RichText::new(detail)
                    .size(12.)
                    .color(palette.gray(150)),
            );
            ui.add(if truncated { detail.truncate() } else { detail });
            ui.add_space(20.);
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = Vec2::new(14., 6.);
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
/// "1 photo", "2 photos": `n` with the word for its number.
pub(super) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
/// A form row as in Lightroom's dialogs: a right-aligned label in a fixed
/// column, then the controls.
pub(super) fn form_row(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(150., 28.), Sense::hover());
        ui.painter().text(
            rect.right_center() - Vec2::new(12., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(13.),
            theme::palette(ui.ctx()).gray(150),
        );
        contents(ui);
    });
}
/// A path for display, with the home folder as `~`.
pub(super) fn pretty_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
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
                            setting_slider(ui, ParameterId::Exposure, &mut value, 0.);
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
                            setting_slider(ui, ParameterId::Exposure, &mut value, 0.);
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
                            setting_slider(ui, ParameterId::Exposure, &mut value, 0.);
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
                        setting_slider(ui, ParameterId::Exposure, &mut value, 0.);
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
