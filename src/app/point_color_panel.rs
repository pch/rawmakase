//! The Color Mixer's Point Color tab: the dropper, up to eight swatches, the selected
//! swatch's shifts, Variance, Range and hue, saturation and luminance ranges, and
//! Visualize Range. Each gesture is one History step.
use super::state::{PointColorView, Tool, ViewState};
use super::theme;
use super::widgets::{name_history_step, set_edit_context, slider_with};
use crate::model::point_color::{MAX_SWATCHES, PointColor};
use crate::model::recipe::Recipe;
use crate::{
    app::Module,
    develop::point_color::{PointColors, add_sample},
};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl super::Editor {
    /// Whether Point Color's tab is where the photo is edited: Develop (not Before), a
    /// color photo with the current process, and the Color Mixer on its Point Color
    /// tab.
    pub(super) fn point_color_tab_shown(&self) -> bool {
        let r = self.document.edit.recipe();
        self.module == Module::Develop
            && !self.view.compare.before_only()
            && self.view.mixer_tab == super::state::MixerTab::PointColor
            && r.treatment() == crate::model::recipe::Treatment::Color
    }
    /// Point Color's dropper at (`u`, `v`) of the shown photo: samples the color there
    /// off the UI thread (the render it needs can take a while on a large photo); the
    /// sample arrives as [`Event::PointColorSample`]. Ignored while one is running.
    ///
    /// [`Event::PointColorSample`]: super::worker::Event::PointColorSample
    pub(super) fn start_point_color_sample(&mut self, u: f32, v: f32) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.point_color_pick.is_running() {
            return;
        }
        let (generation, cancel) = self.document.point_color_pick.start();
        let id = self.load.id();
        let sampled = self.document.edit.recipe().clone();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::develop::quality::point_color_pick(&im, &sampled, u, v, &cancel)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("sampling failed unexpectedly")))
            .map_err(|e| format!("Cannot sample a color: {e:#}"));
            let sampled = Box::new(sampled);
            let _ = tx.send(super::worker::Event::PointColorSample {
                id,
                generation,
                sampled,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Adds the sampled swatch as one History step, selects it and puts the dropper
    /// away; or says why not. A sample of a photo edited since is dropped.
    pub(super) fn point_color_sample_ready(
        &mut self,
        sampled: &Recipe,
        result: Result<[f32; 3], String>,
    ) {
        self.document.point_color_pick.invalidate();
        // The dropper was put away meanwhile (another tab or module, or clicked off).
        if !self.view.is(Tool::PointColor) || !self.point_color_tab_shown() {
            return;
        }
        if *sampled != *self.document.edit.recipe() {
            self.status = "The photo changed while sampling; pick the color again".into();
            return;
        }
        let source = match result {
            Ok(source) => source,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        // A drag still under way is recorded first, so undoing it keeps the swatch.
        if self.document.edit.history().in_gesture() {
            self.document.edit.finish_gesture();
        }
        let mut swatches = self.document.edit.recipe().point_colors.clone();
        match add_sample(&mut swatches, source) {
            Ok(i) => {
                // Selected, with the dropper put away, before the render is
                // scheduled: Visualize Range shows the new swatch.
                self.view.point_color.selected = Some(i);
                self.view.tool = Tool::None;
                let step = super::history::Step::new("Point Color", "Add Swatch");
                self.change_edit(Some(step), |r| r.point_colors = swatches);
            }
            Err(refusal) => self.status = refusal.message().into(),
        }
    }
}

/// The tab's contents. `list` is the photo's swatches.
pub(super) fn point_color_panel(
    ui: &mut egui::Ui,
    list: &mut Vec<PointColor>,
    view: &mut ViewState,
) {
    set_edit_context(ui, "Point Color");
    let pc = &mut view.point_color;
    pc.selected = pc.selected.filter(|i| *i < list.len());
    let picking = view.tool == Tool::PointColor;
    if swatch_row(ui, list, &mut view.point_color, picking) {
        view.toggle(Tool::PointColor);
    }
    let pc = &mut view.point_color;
    let Some(i) = pc.selected else {
        hint(ui, "Click the dropper, then a color in the photo");
        return;
    };
    let swatch = &mut list[i];
    comparison(ui, swatch);
    ui.push_id(("point-color", i), |ui| {
        for (c, name) in ["Hue Shift", "Sat Shift", "Lum Shift"]
            .into_iter()
            .enumerate()
        {
            slider_with(ui, name, &mut swatch.shift[c], -1. ..=1., 0., None, None);
        }
        slider_with(
            ui,
            "Variance",
            &mut swatch.variance,
            -1. ..=1.,
            0.,
            None,
            None,
        );
        slider_with(ui, "Range", &mut swatch.range, 0. ..=1., 0.5, None, None);
        disclosure(ui, &mut pc.ranges);
        if pc.ranges {
            ranges(ui, swatch);
        }
        ui.horizontal(|ui| {
            ui.add_space(83.);
            ui.checkbox(&mut pc.visualize, "Visualize Range")
                .on_hover_text("Show the colors this swatch selects; everything else turns gray");
        });
    });
}

/// The dropper and the swatches. Returns whether the dropper was clicked.
fn swatch_row(
    ui: &mut egui::Ui,
    list: &mut Vec<PointColor>,
    pc: &mut PointColorView,
    picking: bool,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
    let dropper = Rect::from_center_size(
        Pos2::new(row.left() + 70., row.center().y),
        Vec2::new(26., 20.),
    );
    let response = ui.interact(dropper, ui.id().with("point-color-dropper"), Sense::click());
    if picking || response.hovered() {
        ui.painter()
            .rect_filled(dropper, 3., palette.gray(if picking { 72 } else { 50 }));
    }
    let strong = picking || response.hovered();
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Eyedropper,
        dropper.center(),
        14.,
        palette.gray(if strong { 235 } else { 170 }),
    );
    let toggle = response
        .on_hover_text("Point Color dropper: click a color in the photo to add a swatch")
        .clicked();
    let step = ((row.right() - row.left() - 88.) / MAX_SWATCHES as f32).min(24.);
    let mut delete = None;
    for slot in 0..MAX_SWATCHES {
        let center = Pos2::new(
            row.left() + 88. + step * (slot as f32 + 0.5),
            row.center().y,
        );
        let Some(swatch) = list.get(slot) else {
            ui.painter()
                .circle_stroke(center, 7., Stroke::new(1., palette.gray(60)));
            continue;
        };
        let rect = Rect::from_center_size(center, Vec2::splat(step));
        let response = ui.interact(rect, ui.id().with(("swatch", slot)), Sense::click());
        ui.painter()
            .circle_filled(center, 8., display(swatch.source_prophoto()));
        if pc.selected == Some(slot) {
            ui.painter()
                .circle_stroke(center, 10.5, Stroke::new(1.5, palette.gray(215)));
        }
        if response.clicked() {
            pc.selected = Some(slot);
        }
        response.context_menu(|ui| {
            if ui.button("Delete Swatch").clicked() {
                delete = Some(Some(slot));
                ui.close();
            }
            if ui.button("Delete All Swatches").clicked() {
                delete = Some(None);
                ui.close();
            }
        });
    }
    match delete {
        Some(Some(slot)) => {
            list.remove(slot);
            pc.selected = (!list.is_empty()).then(|| slot.min(list.len() - 1));
            name_history_step(ui, "Point Color".into(), "Delete Swatch".into());
        }
        Some(None) => {
            list.clear();
            pc.selected = None;
            name_history_step(ui, "Point Color".into(), "Delete All Swatches".into());
        }
        None => {}
    }
    toggle
}

/// The sampled color beside the color the swatch turns it into.
fn comparison(ui: &mut egui::Ui, swatch: &PointColor) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::hover());
    let rect = Rect::from_min_max(
        Pos2::new(row.left() + 88., row.top() + 2.),
        Pos2::new(row.right() - 4., row.bottom() - 2.),
    );
    let source = swatch.source_prophoto();
    let adjusted = PointColors::new(std::slice::from_ref(swatch))
        .map_or(source, |op| op.apply_prophoto(source));
    let (left, right) = rect.split_left_right_at_fraction(0.5);
    ui.painter().rect_filled(left, 0., display(source));
    ui.painter().rect_filled(right, 0., display(adjusted));
    ui.interact(rect, ui.id().with("comparison"), Sense::hover())
        .on_hover_text("The sampled color, and the color the swatch turns it into");
}

/// The Hue, Saturation and Luminance ranges, as four points each.
fn ranges(ui: &mut egui::Ui, swatch: &mut PointColor) {
    let defaults = PointColor::sampled(swatch.source);
    let [h, s, v] = swatch.source;
    let hue = h / 6. * std::f32::consts::TAU;
    let window = crate::develop::point_color::HUE_WINDOW;
    range_points(
        ui,
        "Hue Range",
        &mut swatch.hue_range,
        Hold::InnerSpan,
        defaults.hue_range,
        &|t| hsv(hue + (t - 0.5) * 2. * window, s.max(0.5), v.max(0.5)),
    );
    range_points(
        ui,
        "Sat Range",
        &mut swatch.saturation_range,
        Hold::Value(s),
        defaults.saturation_range,
        &|t| hsv(hue, t, v.max(0.5)),
    );
    range_points(
        ui,
        "Lum Range",
        &mut swatch.luminance_range,
        Hold::Value(crate::develop::srgb_encode(v)),
        defaults.luminance_range,
        &|t| hsv(hue, s, crate::color::srgb_decode(t)),
    );
}

/// What a range's inner points must keep, so Camera Raw accepts the swatch.
#[derive(Clone, Copy)]
enum Hold {
    /// Some span between the inner points (the hue range).
    InnerSpan,
    /// The sampled value between the inner points.
    Value(f32),
}

/// One range: a gradient rail with the outer and inner points as handles. Dragging
/// moves the nearest handle between its neighbours; double-clicking resets it.
fn range_points(
    ui: &mut egui::Ui,
    label: &str,
    points: &mut [f32; 4],
    hold: Hold,
    default: [f32; 4],
    color: &dyn Fn(f32) -> Color32,
) {
    let palette = theme::palette(ui.ctx());
    let before = *points;
    ui.push_id(label, |ui| {
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::hover());
        ui.painter().text(
            Pos2::new(row.left() + 78., row.center().y),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(11.),
            palette.gray(190),
        );
        let area = Rect::from_min_max(Pos2::new(row.left() + 88., row.top()), row.max);
        let rail = Rect::from_min_max(
            Pos2::new(area.left() + 5., area.center().y - 3.),
            Pos2::new(area.right() - 9., area.center().y + 3.),
        );
        let x = |t: f32| egui::lerp(rail.left()..=rail.right(), t);
        let mut mesh = egui::Mesh::default();
        const STEPS: usize = 32;
        for k in 0..STEPS {
            let (a, b) = (k as f32 / STEPS as f32, (k + 1) as f32 / STEPS as f32);
            let first = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(x(a), rail.top()), color(a));
            mesh.colored_vertex(Pos2::new(x(b), rail.top()), color(b));
            mesh.colored_vertex(Pos2::new(x(b), rail.bottom()), color(b));
            mesh.colored_vertex(Pos2::new(x(a), rail.bottom()), color(a));
            mesh.add_triangle(first, first + 1, first + 2);
            mesh.add_triangle(first, first + 2, first + 3);
        }
        ui.painter().add(mesh);
        let response = ui.interact(area, ui.id().with("rail"), Sense::click_and_drag());
        let active = ui.id().with("active");
        if response.double_clicked() {
            *points = default;
        } else if let Some(p) = response.interact_pointer_pos() {
            let t = egui::remap_clamp(p.x, rail.left()..=rail.right(), 0. ..=1.);
            if response.drag_started() || response.clicked() {
                let handle = nearest(points, t);
                ui.data_mut(|d| d.insert_temp(active, handle));
            }
            if (response.dragged() || response.clicked())
                && let Some(handle) = ui.data(|d| d.get_temp::<usize>(active))
            {
                move_point(points, handle, t, hold);
            }
        }
        let shape = [
            Pos2::new(x(points[0]), rail.bottom() + 5.),
            Pos2::new(x(points[1]), rail.top() - 5.),
            Pos2::new(x(points[2]), rail.top() - 5.),
            Pos2::new(x(points[3]), rail.bottom() + 5.),
        ];
        ui.painter()
            .line(shape.to_vec(), Stroke::new(1., palette.gray(215)));
        for (k, t) in points.iter().enumerate() {
            let c = Pos2::new(x(*t), rail.bottom() + 6.);
            let triangle = vec![c, c + Vec2::new(-4., 6.), c + Vec2::new(4., 6.)];
            let inner = k == 1 || k == 2;
            ui.painter().add(egui::Shape::convex_polygon(
                triangle,
                if inner {
                    palette.gray(215)
                } else {
                    palette.gray(40)
                },
                Stroke::new(1., palette.gray(215)),
            ));
        }
        response.on_hover_text("Drag the outer or inner points · double-click to reset");
    });
    if *points != before {
        let shown: Vec<String> = points.iter().map(|p| format!("{:.0}", p * 100.)).collect();
        name_history_step(ui, format!("Point Color {label}"), shown.join(" / "));
    }
}

/// The handle a press at `t` takes: the nearest one; among handles on one spot, the
/// last when the press is right of them, else the first.
fn nearest(points: &[f32; 4], t: f32) -> usize {
    let distance = |k: usize| (points[k] - t).abs();
    let best = (0..4).map(distance).fold(f32::INFINITY, f32::min);
    let tied: Vec<usize> = (0..4).filter(|k| distance(*k) - best < 1e-4).collect();
    if t > points[tied[0]] {
        tied[tied.len() - 1]
    } else {
        tied[0]
    }
}

/// Moves point `k` to `t`, between its neighbours, keeping what `hold` asks of the
/// inner points.
fn move_point(points: &mut [f32; 4], k: usize, t: f32, hold: Hold) {
    let lo = if k == 0 { 0. } else { points[k - 1] };
    let hi = if k == 3 { 1. } else { points[k + 1] };
    let mut t = t.clamp(lo, hi);
    match (hold, k) {
        (Hold::InnerSpan, 1) => t = t.min(points[2] - 0.01),
        (Hold::InnerSpan, 2) => t = t.max(points[1] + 0.01),
        (Hold::Value(v), 1) => t = t.min(v),
        (Hold::Value(v), 2) => t = t.max(v),
        _ => {}
    }
    points[k] = t.clamp(0., 1.);
}

/// A "Ranges" disclosure row.
fn disclosure(ui: &mut egui::Ui, open: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(83.);
        let text = if *open { "▾ Ranges" } else { "▸ Ranges" };
        if ui
            .add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(11.)
                        .color(theme::palette(ui.ctx()).gray(190)),
                )
                .sense(Sense::click()),
            )
            .on_hover_text("Hue, Sat and Lum Range")
            .clicked()
        {
            *open = !*open;
        }
    });
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(88.);
        ui.label(
            egui::RichText::new(text)
                .size(11.)
                .color(theme::palette(ui.ctx()).gray(150)),
        );
    });
}

/// HSV of linear ProPhoto RGB (hue in radians) as a display color.
fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    display(crate::color::hsv::hsv_to_rgb(h, s, v))
}

/// Linear ProPhoto RGB as an sRGB display color.
fn display(p: [f32; 3]) -> Color32 {
    let rgb = crate::develop::mul(crate::camera_profiles::PRO_TO_RGB, p)
        .map(|v| (crate::develop::srgb_encode(v.clamp(0., 1.)) * 255.).round() as u8);
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_handles_keep_their_order_and_the_sampled_value() {
        let mut p = [0., 0.4, 0.7, 1.];
        move_point(&mut p, 1, 0.9, Hold::Value(0.55));
        assert_eq!(p, [0., 0.55, 0.7, 1.]);
        move_point(&mut p, 2, 0.1, Hold::Value(0.55));
        assert_eq!(p, [0., 0.55, 0.55, 1.]);
        move_point(&mut p, 0, 0.8, Hold::Value(0.55));
        assert_eq!(p, [0.55, 0.55, 0.55, 1.]);
        let mut hue = [0., 1. / 3., 2. / 3., 1.];
        move_point(&mut hue, 1, 0.9, Hold::InnerSpan);
        assert!(hue[1] < hue[2]);
        // Stacked handles: a press right of them takes the last, left the first.
        let stacked = [0., 0., 0.5, 1.];
        assert_eq!(nearest(&stacked, 0.02), 1);
        assert_eq!(nearest(&stacked, 0.), 0);
        assert_eq!(nearest(&stacked, 0.7), 2);
    }
}
