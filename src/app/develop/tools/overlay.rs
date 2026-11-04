//! What the active tool draws over the photo (pins, spot circles, the brush cursor,
//! gradient handles), and which tool gets the pointer.
use crate::app::Editor;
use crate::app::develop::state::Tool;
use crate::app::ui::design::radius;
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, Vec2};

impl Editor {
    /// Draws the active tool's overlay and handles its pointer input. Returns whether
    /// the tool owns clicks and drags on the photo, so the viewport does not pan or
    /// zoom with them. `rect` is where the whole photo is drawn, inside `area`.
    pub(in crate::app) fn tool_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
        area: Rect,
    ) -> bool {
        // Holding Space pans instead, as in Lightroom.
        let taken = ui.input(|i| i.key_down(egui::Key::Space)) || self.view.compare.shows_before();
        // A mask drag the hand, Before or another tool takes over ends where it is,
        // so a later drag does not go on painting or moving it.
        if taken || !self.view.is(Tool::Mask) {
            self.view.masking.end_drag();
        }
        if taken {
            // A targeted drag the hand or Before takes over ends where it is.
            if self.view.targeted.is_some() && !ui.input(|i| i.pointer.primary_down()) {
                self.end_targeted_drag();
            }
            return false;
        }
        match self.view.tool {
            Tool::None | Tool::Crop | Tool::WhiteBalance | Tool::Defringe | Tool::PointColor => {
                false
            }
            Tool::Remove => {
                self.retouch_overlay(ui, response, rect);
                true
            }
            Tool::RedEye => {
                self.red_eye_overlay(ui, response, rect);
                true
            }
            Tool::Mask => self.mask_overlay(ui, response, rect),
            Tool::Guided => {
                self.guided_overlay(ui, response, rect, area);
                true
            }
            Tool::Targeted(target) => {
                self.targeted_overlay(ui, response, rect, target);
                true
            }
        }
    }
}

/// A Lightroom pin: a small grey disc, filled darker when selected.
pub(in crate::app) fn pin(painter: &Painter, at: Pos2, selected: bool) {
    painter.circle_filled(at, 6., Color32::from_black_alpha(140));
    painter.circle_filled(
        at,
        4.5,
        if selected {
            Color32::from_gray(30)
        } else {
            Color32::from_gray(225)
        },
    );
    painter.circle_stroke(at, 4.5, Stroke::new(1.2, Color32::WHITE));
}
/// An ellipse with screen radii `radii`, rotated by `angle` radians, as a white line
/// with a dark outline so it shows on any photo.
pub(in crate::app) fn ellipse(
    painter: &Painter,
    center: Pos2,
    radii: Vec2,
    angle: f32,
    width: f32,
) {
    let (s, c) = angle.sin_cos();
    let points: Vec<Pos2> = (0..72)
        .map(|i| {
            let t = i as f32 / 72. * std::f32::consts::TAU;
            let (x, y) = (t.cos() * radii.x, t.sin() * radii.y);
            center + Vec2::new(c * x - s * y, s * x + c * y)
        })
        .collect();
    painter.add(egui::Shape::closed_line(
        points.clone(),
        Stroke::new(width + 1.5, Color32::from_black_alpha(110)),
    ));
    painter.add(egui::Shape::closed_line(
        points,
        Stroke::new(width, Color32::from_white_alpha(230)),
    ));
}
/// The brush cursor: the outer circle is the brush size, the inner one where the
/// feather starts.
pub(in crate::app) fn brush_cursor(painter: &Painter, at: Pos2, radius: f32, feather: f32) {
    ellipse(painter, at, Vec2::splat(radius), 0., 1.);
    if feather > 0.01 {
        ellipse(painter, at, Vec2::splat(radius * (1. - feather)), 0., 0.7);
    }
    painter.circle_filled(at, 1.5, Color32::WHITE);
}
/// A brush stroke along `points` (screen space) as Lightroom draws one: the outline of
/// everything within `radius` of the path, round at its ends and corners.
pub(in crate::app) fn stroke_outline(painter: &Painter, points: &[Pos2], radius: f32, width: f32) {
    let edges = crate::app::develop::tools::stroke_outline::outline(points, radius, 2.);
    // A brush too thin to outline shows as its path.
    if edges.is_empty() {
        match points {
            [] => {}
            [dot] => ellipse(painter, *dot, Vec2::splat(radius.max(1.5)), 0., width),
            _ => path(painter, points.to_vec(), Color32::from_white_alpha(230)),
        }
        return;
    }
    for [a, b] in &edges {
        painter.line_segment(
            [*a, *b],
            Stroke::new(width + 1.5, Color32::from_black_alpha(110)),
        );
    }
    for [a, b] in edges {
        painter.line_segment([a, b], Stroke::new(width, Color32::from_white_alpha(230)));
    }
}
/// A path drawn as a thin outlined line.
pub(in crate::app) fn path(painter: &Painter, points: Vec<Pos2>, color: Color32) {
    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(2.5, Color32::from_black_alpha(110)),
    ));
    painter.add(egui::Shape::line(points, Stroke::new(1., color)));
}
/// A square handle, as on Lightroom's gradients.
pub(in crate::app) fn handle(painter: &Painter, at: Pos2, active: bool) {
    let r = Rect::from_center_size(at, Vec2::splat(if active { 9. } else { 7. }));
    painter.rect_filled(r.expand(1.), radius::SMALL, Color32::from_black_alpha(140));
    painter.rect_filled(r, radius::SMALL, Color32::WHITE);
}
