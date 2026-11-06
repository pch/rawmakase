//! The minimap over a zoomed-in photo, in Develop and the Library's Loupe, in
//! place of Lightroom's Navigator: the whole photo with the part in view outlined,
//! where a click or drag moves the view. Its grip, shown while the pointer is on
//! it, drags it to another corner of the view. The toolbar's zoom slider
//! (`navigator::zoom_slider`) does the Navigator's other work.
use crate::app::Editor;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

/// The minimap's grip, over the top of its photo, and its distance from the view's
/// edges.
const GRIP: f32 = 14.;
const MAP_MARGIN: f32 = 16.;

/// The corner of the view the minimap sits in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}
impl Corner {
    /// Where a block of `size` starts in this corner of `area`.
    fn min(self, area: Rect, size: Vec2) -> Pos2 {
        let left = matches!(self, Self::TopLeft | Self::BottomLeft);
        let top = matches!(self, Self::TopLeft | Self::TopRight);
        Pos2::new(
            if left {
                area.left() + MAP_MARGIN
            } else {
                area.right() - MAP_MARGIN - size.x
            },
            if top {
                area.top() + MAP_MARGIN
            } else {
                area.bottom() - MAP_MARGIN - size.y
            },
        )
    }
    /// The corner of `area` nearest `at`, where a dragged minimap is let go.
    fn nearest(at: Pos2, area: Rect) -> Self {
        let c = area.center();
        match (at.x < c.x, at.y < c.y) {
            (true, true) => Self::TopLeft,
            (false, true) => Self::TopRight,
            (true, false) => Self::BottomLeft,
            (false, false) => Self::BottomRight,
        }
    }
}
/// Where the minimap is, and while its grip is dragged, where the pointer holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::app) struct Minimap {
    pub(in crate::app) corner: Corner,
    held: Option<Vec2>,
}

impl Editor {
    /// Zoomed in on the edit (Develop, or a photo developed in the Library's
    /// Loupe), its minimap; see [`Self::minimap_ui`]. Returns whether the pointer
    /// is on it.
    pub(in crate::app) fn develop_minimap(&mut self, ui: &egui::Ui, area: Rect) -> bool {
        let (Some(photo), Some(shown)) = self.navigator_view() else {
            return false;
        };
        let (on, inspect) = self.minimap_ui(ui, area, photo, shown);
        if let Some(at) = inspect {
            self.inspect(at);
        }
        on
    }
    /// The minimap in a corner of `area`, in place of Lightroom's Navigator: the
    /// whole `photo` with the part `shown` outlined, where a click or drag asks to
    /// look at that point. Its grip, shown while the pointer is on it, moves it to
    /// another corner. Returns whether the pointer is on it, and the point asked
    /// for.
    pub(in crate::app) fn minimap_ui(
        &mut self,
        ui: &egui::Ui,
        area: Rect,
        (texture, size): crate::app::view::navigator::Photo,
        shown: [f32; 4],
    ) -> (bool, Option<[f32; 2]>) {
        const EDGE: f32 = 180.;
        let size = size * (EDGE / size.x.max(size.y));
        // The outline is drawn where the view is going, under the pointer, while
        // the photo glides after it: centred on the target at the size shown.
        let heading = self.view.glide.map(|g| g.target());
        let place = &mut self.view.minimap;
        let grip_id = egui::Id::new("minimap-grip");
        // Where the pointer holds it, through the frame the button is released in.
        let min = match (place.held, ui.ctx().pointer_interact_pos()) {
            (Some(offset), Some(p)) => {
                let lo = area.min + Vec2::splat(MAP_MARGIN);
                let hi = area.max - size - Vec2::splat(MAP_MARGIN);
                (p - offset).clamp(lo, hi.max(lo))
            }
            _ => place.corner.min(area, size),
        };
        let rect = Rect::from_min_size(min, size);
        // The grip shows over the photo's top edge while the pointer is on the map.
        let grip = Rect::from_min_size(rect.min, Vec2::new(size.x, GRIP));
        let response = ui.interact(
            rect,
            egui::Id::new("magnifier-minimap"),
            egui::Sense::click_and_drag(),
        );
        // Over the map's own response, so the grip takes its strip.
        let handle = ui
            .interact(grip, grip_id, egui::Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        if handle.drag_started()
            && let Some(p) = handle.interact_pointer_pos()
        {
            place.held = Some(p - rect.min);
        }
        if handle.drag_stopped() {
            place.corner = Corner::nearest(rect.center(), area);
            place.held = None;
        }
        if handle.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let painter = ui.painter().with_clip_rect(area);
        painter.rect_filled(rect.expand(2.), 4., Color32::from_black_alpha(170));
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        painter.image(texture, rect, uv, Color32::WHITE);
        let [x, y, w, h] = shown;
        let (x, y) = heading.map_or((x, y), |[tx, ty]| {
            (
                (tx - w / 2.).clamp(0., 1. - w),
                (ty - h / 2.).clamp(0., 1. - h),
            )
        });
        let at = |fx: f32, fy: f32| rect.min + Vec2::new(fx, fy) * rect.size();
        painter.rect_stroke(
            Rect::from_min_max(at(x, y), at(x + w, y + h)),
            0.,
            Stroke::new(1.5, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
        let inspect = (response.clicked() || response.dragged())
            .then(|| response.interact_pointer_pos())
            .flatten()
            .map(|p| {
                // As far as the view can go: past the edge of the part in view, a
                // glide would spend its time where the photo no longer moves.
                let at = (p - rect.min) / rect.size();
                [
                    at.x.clamp(w / 2., 1. - w / 2.),
                    at.y.clamp(h / 2., 1. - h / 2.),
                ]
            });
        let on_map = response.hovered() || handle.hovered() || handle.dragged();
        if on_map && !response.dragged() {
            let shade = Color32::from_black_alpha(140);
            painter.rect_filled(
                grip,
                egui::CornerRadius {
                    nw: 2,
                    ne: 2,
                    sw: 0,
                    se: 0,
                },
                shade,
            );
            let pill = Rect::from_center_size(grip.center(), Vec2::new(28., 3.));
            let ink = if handle.hovered() || handle.dragged() {
                240
            } else {
                170
            };
            painter.rect_filled(pill, 1.5, Color32::from_white_alpha(ink));
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        (on_map || response.dragged(), inspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::{CameraImage, Metadata};
    use std::sync::Arc;

    /// Dragging the minimap's grip and letting go leaves it in the corner it was
    /// dropped nearest, not back where it started.
    #[test]
    fn a_minimap_dragged_by_its_grip_stays_in_its_new_corner() {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let image = Arc::new(CameraImage {
            recovered: Default::default(),
            width: 2000,
            height: 1500,
            pixels: vec![[0.2; 3]; 3_000_000],
            metadata: Metadata {
                width: 2000,
                height: 1500,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        });
        editor.document.set_image(image);
        editor.module = crate::app::Module::Develop;
        let gray = |name| {
            Some(
                ctx.load_texture(
                    name,
                    egui::ColorImage::filled([200, 150], Color32::GRAY),
                    egui::TextureOptions::LINEAR,
                )
                .into(),
            )
        };
        editor.preview.texture = gray("photo");
        editor.preview.navigator = gray("navigator");
        editor.view.zoom.on = true;
        editor.view.zoom.level = 1.;
        let mut frame = |events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    editor.viewport_ui(ui);
                },
            )
            .textures_delta
            .clear();
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // The 180 × 135 map in the bottom-right corner; its grip along its top.
        let grip = Pos2::new(800. - 16. - 90., 600. - 16. - 135. + 6.);
        frame(vec![]);
        frame(vec![egui::Event::PointerMoved(grip)]);
        frame(vec![button(grip, true)]);
        let to = Pos2::new(120., 90.);
        for k in 1..=8 {
            frame(vec![egui::Event::PointerMoved(
                grip + (to - grip) * k as f32 / 8.,
            )]);
        }
        frame(vec![button(to, false)]);
        frame(vec![]);
        assert_eq!(editor.view.minimap.corner, Corner::TopLeft);
        assert!(editor.view.minimap.held.is_none());
    }

    /// A minimap let go snaps to the corner it is nearest.
    #[test]
    fn a_dropped_minimap_takes_the_nearest_corner() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 600.));
        assert_eq!(Corner::nearest(Pos2::new(100., 80.), area), Corner::TopLeft);
        assert_eq!(
            Corner::nearest(Pos2::new(900., 80.), area),
            Corner::TopRight
        );
        assert_eq!(
            Corner::nearest(Pos2::new(400., 500.), area),
            Corner::BottomLeft
        );
        assert_eq!(
            Corner::nearest(Pos2::new(600., 400.), area),
            Corner::BottomRight
        );
        let size = Vec2::new(180., 134.);
        assert_eq!(Corner::TopLeft.min(area, size), Pos2::new(16., 16.));
        assert_eq!(Corner::BottomRight.min(area, size), Pos2::new(804., 450.));
    }
}
