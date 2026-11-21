//! The brush stroke being painted, drawn over the photo as it is painted.
//!
//! The stroke goes into the edit at every step of the drag, but its coverage
//! shows only once a render with it arrives, which, while the mask overlay
//! shows, is a CPU render. This draws the stroke's own coverage in the
//! overlay's color in the meantime, at screen resolution, by the committed
//! stroke's rule ([`LiveStroke`]). Each new segment updates only the pixels it
//! reaches, and only that rectangle is uploaded again. It is a preview over the
//! render and nothing else: the edit, History and XMP never see it.
use crate::develop::masks::{LiveStroke, Space};
use crate::model::masks::BrushStroke;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

/// The longest side of the buffer, in pixels.
const MAX_SIDE: f32 = 2048.;
/// The most buffer pixels across the brush's radius: a large brush is drawn
/// coarser, which its smooth edge hides, so a step costs about the same
/// whatever the brush's size.
const MAX_RADIUS: f32 = 64.;

/// Where the stroke is in the edit: mask, component and stroke.
pub(in crate::app) type StrokeAt = (usize, usize, usize);

/// How the view maps the photo, as the buffer was laid out for it.
#[derive(Clone, Copy, PartialEq)]
struct Layout {
    /// The buffer's place on the screen, in points: the part of the photo in
    /// view.
    rect: Rect,
    size: [usize; 2],
    /// Image-space positions of three of its corners, which change with the
    /// crop, the zoom and the pan.
    corners: [[f32; 2]; 3],
}

pub(in crate::app) struct LiveBrush {
    at: StrokeAt,
    /// The stroke's path as painted, in image space.
    points: Vec<[f32; 2]>,
    line: LiveStroke,
    space: Space,
    /// Points of `line` whose segments are in the buffer.
    applied: usize,
    layout: Option<Layout>,
    /// Each pixel's squared distance to the path (long-edge units).
    nearest: Vec<f32>,
    /// Each pixel's long-edge position, worked out the first time a segment
    /// reaches it; NaN until then.
    positions: Vec<[f32; 2]>,
    image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
    /// The pixels changed since the last upload: left, top, right, bottom.
    dirty: Option<[usize; 4]>,
    /// The drag has ended: dropped once a render shows the whole stroke.
    ended: bool,
    /// The render shown has the stroke as it is now.
    rendered: bool,
}

impl LiveBrush {
    /// Starts showing `stroke`, at `at` in the edit.
    pub(in crate::app) fn new(at: StrokeAt, stroke: &BrushStroke, space: Space) -> Self {
        Self {
            at,
            points: stroke.points.to_vec(),
            line: LiveStroke::new(stroke, space),
            space,
            applied: 0,
            layout: None,
            nearest: Vec::new(),
            positions: Vec::new(),
            image: egui::ColorImage::filled([0, 0], Color32::TRANSPARENT),
            texture: None,
            dirty: None,
            ended: false,
            rendered: false,
        }
    }
    pub(in crate::app) fn at(&self) -> StrokeAt {
        self.at
    }
    pub(in crate::app) fn points(&self) -> &[[f32; 2]] {
        &self.points
    }
    /// Takes the path's new points, `points` being all of it so far.
    pub(in crate::app) fn follow(&mut self, points: &[[f32; 2]]) {
        if points.len() > self.points.len() && points.starts_with(&self.points) {
            for p in &points[self.points.len()..] {
                self.line.push(*p);
            }
            self.points.extend_from_slice(&points[self.points.len()..]);
            self.rendered = false;
        }
    }
    /// A render is shown: whether it has the whole stroke as painted, given
    /// the stroke in its recipe.
    pub(in crate::app) fn render_shown(&mut self, stroke: Option<&BrushStroke>) {
        self.rendered = stroke.is_some_and(|s| *s.points == *self.points);
    }
    /// Whether to let it go: the drag has ended and a render shows the stroke.
    pub(in crate::app) fn done(&self) -> bool {
        self.ended && self.rendered
    }
    /// The drag has ended: only the texture is kept, until a render shows the
    /// stroke.
    pub(in crate::app) fn finish(&mut self) {
        self.ended = true;
        self.nearest = Vec::new();
        self.positions = Vec::new();
        self.image = egui::ColorImage::filled([0, 0], Color32::TRANSPARENT);
    }
    /// A finished stroke's texture and place, while the view is the one it
    /// was laid out for; `None` once the view has changed, as it can't be
    /// laid out again.
    pub(in crate::app) fn shown(
        &self,
        visible: Rect,
        to_image: &impl Fn(Pos2) -> [f32; 2],
    ) -> Option<(egui::TextureId, Rect)> {
        let layout = self.layout?;
        let corners = [
            visible.left_top(),
            visible.right_top(),
            visible.left_bottom(),
        ]
        .map(to_image);
        (layout.rect == visible && layout.corners == corners)
            .then(|| Some((self.texture.as_ref()?.id(), visible)))
            .flatten()
    }
    /// Brings the buffer up to date for the view (`visible`, the part of the
    /// photo on screen, in points; `to_image` and `to_screen` map between the
    /// screen and image space; `radius_on_screen` is a long-edge radius at an
    /// image position, in points) and returns its texture and place.
    pub(in crate::app) fn update(
        &mut self,
        ctx: &egui::Context,
        visible: Rect,
        to_image: &impl Fn(Pos2) -> [f32; 2],
        to_screen: &impl Fn([f32; 2]) -> Pos2,
        radius_on_screen: &impl Fn([f32; 2], f32) -> f32,
        color: Color32,
    ) -> Option<(egui::TextureId, Rect)> {
        if visible.width() < 1. || visible.height() < 1. {
            return None;
        }
        let ppp = ctx.pixels_per_point();
        let scale = (ppp * visible.size()).max_elem().max(1.);
        let brush = radius_on_screen(self.points[0], self.line.radius()).max(1.);
        let k = (ppp * (MAX_SIDE / scale).min(1.)).min(MAX_RADIUS / brush);
        let size = [
            ((visible.width() * k).ceil() as usize).max(1),
            ((visible.height() * k).ceil() as usize).max(1),
        ];
        let layout = Layout {
            rect: visible,
            size,
            corners: [
                visible.left_top(),
                visible.right_top(),
                visible.left_bottom(),
            ]
            .map(to_image),
        };
        if self.layout != Some(layout) {
            // Laid out again for the new view: every segment again.
            self.layout = Some(layout);
            let n = size[0] * size[1];
            self.nearest = vec![f32::INFINITY; n];
            self.positions = vec![[f32::NAN; 2]; n];
            self.image = egui::ColorImage::filled(size, Color32::TRANSPARENT);
            self.texture = None;
            self.applied = 0;
            self.dirty = None;
        }
        let cell = Vec2::new(
            visible.width() / size[0] as f32,
            visible.height() / size[1] as f32,
        );
        let from = self.applied.saturating_sub(1);
        for i in from..self.line.segments() {
            let a = self.points[i];
            let b = *self.points.get(i + 1).unwrap_or(&a);
            // A little past the brush's edge on screen, as the mapping may bend.
            let reach = radius_on_screen(a, self.line.radius())
                .max(radius_on_screen(b, self.line.radius()))
                * 1.1
                + 2.;
            let box_ = Rect::from_two_pos(to_screen(a), to_screen(b)).expand(reach);
            self.paint_segment(i, box_, visible, cell, to_image, color);
        }
        self.applied = self.line.points();
        self.upload(ctx);
        Some((self.texture.as_ref()?.id(), visible))
    }
    /// Lowers the distances of the pixels in `box_` (points) to segment `i`.
    fn paint_segment(
        &mut self,
        i: usize,
        box_: Rect,
        visible: Rect,
        cell: Vec2,
        to_image: &impl Fn(Pos2) -> [f32; 2],
        color: Color32,
    ) {
        let [w, h] = self.image.size;
        let column = |x: f32| ((x - visible.left()) / cell.x).floor().clamp(0., w as f32) as usize;
        let row = |y: f32| ((y - visible.top()) / cell.y).floor().clamp(0., h as f32) as usize;
        let (x0, x1) = (column(box_.left()), (column(box_.right()) + 1).min(w));
        let (y0, y1) = (row(box_.top()), (row(box_.bottom()) + 1).min(h));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let [r, g, b, _] = color.to_array();
        for y in y0..y1 {
            for x in x0..x1 {
                let at = y * w + x;
                if self.positions[at][0].is_nan() {
                    let centre = Pos2::new(
                        visible.left() + (x as f32 + 0.5) * cell.x,
                        visible.top() + (y as f32 + 0.5) * cell.y,
                    );
                    self.positions[at] = self.space.to(to_image(centre));
                }
                let d2 = self.line.to_segment(i, self.positions[at]);
                if d2 < self.nearest[at] {
                    self.nearest[at] = d2;
                    let alpha = self.alpha(d2);
                    self.image.pixels[at] = Color32::from_rgba_unmultiplied(r, g, b, alpha);
                }
            }
        }
        self.dirty = Some(match self.dirty {
            Some([l, t, rr, bb]) => [l.min(x0), t.min(y0), rr.max(x1), bb.max(y1)],
            None => [x0, y0, x1, y1],
        });
    }
    /// The stroke's coverage at squared distance `d2`, or what an erasing
    /// stroke takes away there, as 8-bit alpha.
    pub(in crate::app) fn alpha(&self, d2: f32) -> u8 {
        let v = if self.line.erases() {
            self.line.erased(d2)
        } else {
            self.line.coverage(d2)
        };
        (v * 255.).round() as u8
    }
    /// Uploads what changed: the whole buffer the first time, else only the
    /// rectangle the new segments reached.
    fn upload(&mut self, ctx: &egui::Context) {
        let Some([x0, y0, x1, y1]) = self.dirty.take() else {
            return;
        };
        match &mut self.texture {
            None => {
                self.texture = Some(ctx.load_texture(
                    "live-brush-stroke",
                    self.image.clone(),
                    egui::TextureOptions::LINEAR,
                ));
            }
            Some(texture) => {
                let w = self.image.size[0];
                let pixels = (y0..y1)
                    .flat_map(|y| self.image.pixels[y * w + x0..y * w + x1].iter().copied())
                    .collect();
                let part = egui::ColorImage::new([x1 - x0, y1 - y0], pixels);
                texture.set_partial([x0, y0], part, egui::TextureOptions::LINEAR);
            }
        }
    }
    /// The buffer's coverage (0–1) at pixel `x`, `y`, for tests.
    #[cfg(test)]
    pub(in crate::app) fn coverage_at(&self, x: usize, y: usize) -> f32 {
        let d2 = self.nearest[y * self.image.size[0] + x];
        if self.line.erases() {
            self.line.erased(d2)
        } else {
            self.line.coverage(d2)
        }
    }
    #[cfg(test)]
    pub(in crate::app) fn size(&self) -> [usize; 2] {
        self.image.size
    }
}
