//! Lightroom's Before/After views in Develop: Before alone, full frame (\), or beside
//! the edit left/right (Y) or top/bottom (Alt+Y), or split across one photo (Shift+Y).
//!
//! Before belongs to the open photo and is not saved with the edit, as in Lightroom:
//! the photo's starting settings until a History step or snapshot is copied to it, or
//! the edit is. It never takes edits; panels and tools act on the edit (After), and
//! copying Before to After or swapping them is one History step. Both sides share zoom
//! and pan, and Before is framed with the edit's crop and orientation so the two line
//! up. Before renders in its own lane of the renderer, once per view: editing After
//! does not render it again.
//!
//! Reference View (see `reference`) is laid out as a side-by-side view, with the
//! reference photo in Before's place, rendered in the same lane.
use crate::app::develop::state::{Picture, TextureMode};
use crate::app::ui::design::{radius, space, text};
use crate::app::{Editor, history::Step};
use crate::rendered::ClipOverlay;
use crate::{camera_data::CameraImage, develop::Geometry, model::recipe::Recipe};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::{path::PathBuf, sync::Arc};

/// How Develop shows Before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Compare {
    /// The edit alone (Lightroom's Loupe).
    #[default]
    Off,
    /// Before alone, full frame.
    BeforeOnly,
    /// The camera's embedded JPEG in place of the photo (see `camera_thumbnail`).
    CameraThumbnail,
    /// Before and After side by side, each the whole photo.
    SideBySide(Axis),
    /// One photo, Before on one side of a line and After on the other.
    Split(Axis),
    /// Lightroom's Reference View: another photo, the reference, beside the edit.
    Reference(Axis),
}
/// Which way the two sides are laid out: Before left or on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Axis {
    LeftRight,
    TopBottom,
}

/// The views the Before/After menu lists, with their names.
pub(in crate::app) const VIEWS: [(Compare, &str); 5] = [
    (Compare::BeforeOnly, "Before Only"),
    (Compare::SideBySide(Axis::LeftRight), "Left / Right"),
    (Compare::SideBySide(Axis::TopBottom), "Top / Bottom"),
    (Compare::Split(Axis::LeftRight), "Split Left / Right"),
    (Compare::Split(Axis::TopBottom), "Split Top / Bottom"),
];

impl Compare {
    /// Before is on screen, alone or with the edit.
    pub(in crate::app) fn shows_before(self) -> bool {
        matches!(
            self,
            Self::BeforeOnly | Self::SideBySide(_) | Self::Split(_)
        )
    }
    /// Reference View.
    pub(in crate::app) fn reference(self) -> bool {
        matches!(self, Self::Reference(_))
    }
    /// Before alone: the panels are disabled, as nothing they change is shown.
    pub(in crate::app) fn before_only(self) -> bool {
        matches!(self, Self::BeforeOnly | Self::CameraThumbnail)
    }
    /// The photo is rendered with Before's settings, alone. Not the camera thumbnail,
    /// which shows no render: under it, and outside Develop, the photo is the edit.
    pub(in crate::app) fn renders_before_alone(self) -> bool {
        self == Self::BeforeOnly
    }
    /// Before, or the reference photo, beside the edit, rendered separately.
    pub(in crate::app) fn two_up(self) -> bool {
        matches!(
            self,
            Self::SideBySide(_) | Self::Split(_) | Self::Reference(_)
        )
    }
    /// `view`, or back to the edit alone when `view` is already shown: what its key
    /// does, as Y, Alt+Y, Shift+Y and \ do in Lightroom.
    pub(in crate::app) fn toggled(self, view: Self) -> Self {
        if self == view { Self::Off } else { view }
    }
}

/// Where a side's photo is laid out (`area`, which zoom and pan place it in) and the
/// part of the screen it shows in (`clip`): the same in side-by-side views, half of
/// the area in split ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Pane {
    pub(in crate::app) area: Rect,
    pub(in crate::app) clip: Rect,
}
/// The viewport's panes for a view: After always, Before when it shows beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Panes {
    pub(in crate::app) after: Pane,
    pub(in crate::app) before: Option<Pane>,
}
/// Space between the two sides of a side-by-side view.
const GAP: f32 = space::S;

/// Lays out `area` for `compare`.
pub(in crate::app) fn panes(compare: Compare, area: Rect) -> Panes {
    let halves = |axis: Axis, gap: f32| {
        let c = area.center();
        match axis {
            Axis::LeftRight => (
                Rect::from_min_max(area.min, Pos2::new(c.x - gap / 2., area.bottom())),
                Rect::from_min_max(Pos2::new(c.x + gap / 2., area.top()), area.max),
            ),
            Axis::TopBottom => (
                Rect::from_min_max(area.min, Pos2::new(area.right(), c.y - gap / 2.)),
                Rect::from_min_max(Pos2::new(area.left(), c.y + gap / 2.), area.max),
            ),
        }
    };
    let whole = Pane { area, clip: area };
    match compare {
        Compare::Off | Compare::BeforeOnly | Compare::CameraThumbnail => Panes {
            after: whole,
            before: None,
        },
        Compare::SideBySide(axis) | Compare::Reference(axis) => {
            let (before, after) = halves(axis, GAP);
            let pane = |r| Pane { area: r, clip: r };
            Panes {
                after: pane(after),
                before: Some(pane(before)),
            }
        }
        Compare::Split(axis) => {
            let (before, after) = halves(axis, 0.);
            Panes {
                after: Pane { area, clip: after },
                before: Some(Pane { area, clip: before }),
            }
        }
    }
}

/// Where Before's photo sits, given where After's does: moved with its pane, so the
/// same part of the photo is under the same place in each. A Before of another size
/// (its own Transform) is placed in its pane on its own.
pub(in crate::app) fn before_rect(after: Rect, panes: &Panes, same_size: bool) -> Option<Rect> {
    let before = panes.before?;
    same_size.then(|| after.translate(before.area.min - panes.after.area.min))
}
/// The point on After's photo that `pos` on Before's shows, so a click on either side
/// zooms to the same place.
pub(in crate::app) fn to_after(pos: Pos2, before: Rect, after: Rect) -> Pos2 {
    after.min + (pos - before.min) / before.size() * after.size()
}

/// What Before was last rendered with: the same job is not rendered again.
#[derive(Clone)]
struct BeforeJob {
    image: Arc<CameraImage>,
    recipe: Recipe,
    /// The AI Denoise source the render captured: a newer result renders again.
    ai: Option<crate::develop::AiSource>,
    region: Option<[u32; 4]>,
    max_edge: u32,
    monitor: Option<PathBuf>,
    clipping: ClipOverlay,
}
/// The photo the side beside the edit renders, its settings, the 100% region and
/// the long edge of its whole photo.
pub(in crate::app) struct SecondSide<'a> {
    pub(in crate::app) image: &'a Arc<CameraImage>,
    pub(in crate::app) recipe: &'a Recipe,
    pub(in crate::app) region: Option<[u32; 4]>,
    pub(in crate::app) max_edge: u32,
}
impl PartialEq for BeforeJob {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image)
            && self.recipe == other.recipe
            && self.ai == other.ai
            && self.region == other.region
            && self.max_edge == other.max_edge
            && self.monitor == other.monitor
            && self.clipping == other.clipping
    }
}

/// Before's render beside the edit: its own task and textures, like the edit's.
#[derive(Default)]
pub(in crate::app) struct BeforePreview {
    pub(in crate::app) task: crate::app::task::Task,
    /// The last whole-photo render, and a 100% region drawn over it.
    pub(in crate::app) texture: Option<Picture>,
    pub(in crate::app) region: Option<Picture>,
    pub(in crate::app) mode: TextureMode,
    pending_mode: TextureMode,
    /// The job asked for last, rendered or on its way.
    submitted: Option<BeforeJob>,
}
impl BeforePreview {
    /// Drops what Before showed, e.g. when it leaves the screen or the photo changes.
    pub(in crate::app) fn clear(&mut self) {
        self.task.invalidate();
        self.texture = None;
        self.region = None;
        self.mode = TextureMode::Whole;
        self.submitted = None;
    }
    /// The render asked for last is a 100% region.
    #[cfg(test)]
    pub(in crate::app) fn zoomed(&self) -> bool {
        matches!(self.pending_mode, TextureMode::Region(_))
    }
    /// Asks for its job again on the next schedule, e.g. after its textures were lost.
    pub(in crate::app) fn forget_job(&mut self) {
        self.submitted = None;
    }
    /// The textures it draws.
    pub(in crate::app) fn pictures(&mut self) -> [&mut Option<Picture>; 2] {
        [&mut self.texture, &mut self.region]
    }
}

/// What a Before/After command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Transfer {
    /// Before's settings become the edit's, as one History step.
    BeforeToAfter,
    /// The edit's settings become Before's.
    AfterToBefore,
    /// Each takes the other's settings; the edit's change is one History step.
    Swap,
}

/// Before takes the edit's crop and orientation, so both sides show the same part of
/// the photo, as the full-frame Before always has.
fn framed_like(mut before: Recipe, edit: &Recipe) -> Recipe {
    before.crop = edit.crop;
    before.rotation = edit.rotation;
    before.flip_x = edit.flip_x;
    before.flip_y = edit.flip_y;
    before.straighten = edit.straighten;
    // Whether that angle is Auto's goes with it.
    before.straighten_auto = edit.straighten_auto;
    before
}

impl Editor {
    /// The settings Before shows: those copied to it, else the photo's starting
    /// settings (its raw defaults), framed as the edit on screen is (a hovered
    /// preset's crop included).
    pub(in crate::app) fn before_settings(&self) -> Recipe {
        let shown = self
            .presets
            .preview
            .as_ref()
            .unwrap_or(self.document.edit.recipe());
        self.before_framed_by(shown)
    }
    /// Before's settings framed as `edit` is.
    fn before_framed_by(&self, edit: &Recipe) -> Recipe {
        let before = self
            .document
            .before
            .clone()
            .unwrap_or_else(|| self.photo_defaults().map(|d| d.recipe).unwrap_or_default());
        framed_like(before, edit)
    }
    /// Shows `view`, closing any tool when Before goes beside the edit: tools work on
    /// the edit alone, as in Lightroom.
    pub(in crate::app) fn set_compare(&mut self, view: Compare) {
        if view.two_up() {
            self.view.tool = crate::app::develop::state::Tool::None;
        }
        // Before and the reference share a lane: neither shows the other's render.
        if view.reference() != self.view.compare.reference() {
            self.preview.before.clear();
        }
        self.view.compare = view;
        if view.reference() {
            self.load_reference();
        }
        if view == Compare::CameraThumbnail {
            self.start_camera_thumbnail();
        }
    }
    /// A tool opened while Before is beside the edit goes back to the edit alone.
    pub(in crate::app) fn leave_compare_for_tools(&mut self) {
        if self.view.compare.two_up() && self.view.tool != crate::app::develop::state::Tool::None {
            self.view.compare = Compare::Off;
        }
    }
    /// Copy or swap settings between Before and After.
    pub(in crate::app) fn transfer(&mut self, transfer: Transfer) {
        // Framed as the edit itself, never by a preset only hovered.
        let before = self.before_framed_by(self.document.edit.recipe());
        let after = self.document.edit.recipe().clone();
        let (step, edit) = match transfer {
            Transfer::AfterToBefore => {
                self.document.before = Some(after);
                ("", None)
            }
            Transfer::BeforeToAfter => ("Copy Before Settings to After", Some(before)),
            Transfer::Swap => {
                self.document.before = Some(after);
                ("Swap Before and After Settings", Some(before))
            }
        };
        if let Some(edit) = edit.filter(|e| *e != *self.document.edit.recipe()) {
            self.document.edit.name_next_step(Step::new(step, ""));
            self.document.edit.replace(edit);
            self.ensure_analysis();
        }
        self.schedule();
    }
    /// History's Copy History Step Settings to Before: the state with `applied` steps.
    pub(in crate::app) fn before_from_history(&mut self, applied: usize) {
        if let Some(state) = self
            .document
            .edit
            .history()
            .state(applied, self.document.edit.recipe())
        {
            self.set_before(state);
        }
    }
    /// Before shows `settings` from now on, for this photo.
    pub(in crate::app) fn set_before(&mut self, settings: Recipe) {
        self.document.before = Some(settings);
        self.schedule();
    }
    /// Before as rendered: switched-off panels left out, as the edit's.
    fn before_settings_rendered(&self) -> Recipe {
        let r = self.before_settings();
        if r.panels.all_on() {
            r
        } else {
            r.as_rendered().into_owned()
        }
    }
    /// What the side beside the edit renders: Before, or the reference photo, with
    /// the zoom it is shown at.
    fn second_side(&self) -> Option<(Arc<CameraImage>, Recipe, crate::app::view::navigator::Zoom)> {
        if !self.view.compare.two_up() {
            return None;
        }
        let (image, recipe, zoom) = if self.view.compare.reference() {
            let side = self.reference_side()?;
            (side.image, side.recipe, self.reference.zoom)
        } else {
            let image = self.document.full()?.clone();
            (image, self.before_settings(), self.view.zoom)
        };
        // Switched-off panels left out, as the edit's.
        let recipe = if recipe.panels.all_on() {
            recipe
        } else {
            recipe.as_rendered().into_owned()
        };
        Some((image, recipe, zoom))
    }
    /// Renders Before (or the reference) beside the edit when its view changed;
    /// edits to After alone do not render it again. Leaving the side-by-side views
    /// frees its textures.
    pub(in crate::app) fn schedule_before(&mut self) {
        let Some((image, recipe, zoom)) = self.second_side() else {
            let before = &self.preview.before;
            if !self.view.compare.reference()
                && (before.submitted.is_some()
                    || before.texture.is_some()
                    || before.region.is_some())
            {
                self.preview.before.clear();
            }
            return;
        };
        let geometry = Geometry::new(&image, &recipe, 0);
        let viewport = self.view.viewport;
        let job = BeforeJob {
            region: zoom.render_region(
                viewport,
                geometry.width,
                geometry.height,
                self.preview
                    .before
                    .submitted
                    .as_ref()
                    .and_then(|j| j.region),
            ),
            max_edge: crate::app::develop::workflow::render_edges(&zoom, viewport, &geometry)
                .max_edge,
            ai: self.document.ai_source(&image, &recipe.ai_denoise),
            image,
            recipe,
            monitor: self.view.display_profile(),
            clipping: self.view.clipping.overlay(),
        };
        if self.preview.before.submitted.as_ref() == Some(&job) {
            return;
        }
        let before = &mut self.preview.before;
        let (id, cancel) = before.task.start();
        before.pending_mode = job.region.map_or(TextureMode::Whole, TextureMode::Region);
        before.submitted = Some(job.clone());
        let drawn = self.preview.presented();
        self.renderer.submit(crate::app::worker::RenderJob {
            id,
            pane: crate::app::worker::Pane::Before,
            image: job.image,
            max_edge: job.max_edge,
            cancel,
            recipe: job.recipe,
            region: job.region,
            monitor: job.monitor,
            clipping: job.clipping,
            navigator: false,
            thumbnail: false,
            samples: false,
            overlay: crate::app::worker::Overlay::None,
            ai: job.ai,
            drawn,
        });
    }
    /// A Before render arrived.
    pub(in crate::app) fn before_rendered(
        &mut self,
        ctx: &egui::Context,
        preview: crate::app::worker::Preview,
        stage: crate::app::worker::RenderStage,
        task: u64,
    ) {
        let before = &mut self.preview.before;
        let region = matches!(before.pending_mode, TextureMode::Region(_));
        let slot = if region {
            &mut before.region
        } else {
            &mut before.texture
        };
        match preview {
            crate::app::worker::Preview::Pixels {
                image, display_rgb, ..
            } => {
                let size = [image.width as usize, image.height as usize];
                let image = egui::ColorImage::from_rgb(size, &display_rgb);
                Picture::upload(slot, ctx, "before", image);
            }
            crate::app::worker::Preview::Texture { id, size, .. } => {
                *slot = Some(Picture::presented(id, size));
            }
        }
        before.mode = before.pending_mode;
        if stage != crate::app::worker::RenderStage::Draft {
            before.task.finish(task);
        }
    }
    /// Draws Before in its pane beside the edit, whose photo sits at `after` in
    /// `panes.after`, and labels both sides. Returns where Before's photo is drawn.
    pub(in crate::app) fn before_pane_ui(
        &mut self,
        ui: &egui::Ui,
        panes: &Panes,
        after: Rect,
    ) -> Option<Rect> {
        let pane = panes.before?;
        if self.view.compare.reference() {
            return self.reference_pane_ui(ui, panes, pane);
        }
        let geometry = self.document.full().map(|im| {
            (
                Geometry::new(im, &self.before_settings_rendered(), 0),
                Geometry::new(im, &self.effective_recipe(), 0),
            )
        });
        let same_size = geometry
            .as_ref()
            .is_none_or(|(b, a)| (b.width, b.height) == (a.width, a.height));
        let rect = before_rect(after, panes, same_size).unwrap_or_else(|| {
            let size = geometry.as_ref().map_or(after.size(), |(b, _)| {
                Vec2::new(b.width as f32, b.height as f32)
            });
            let ppp = ui.ctx().pixels_per_point();
            self.view.zoom.photo_rect(pane.area, size, ppp)
        });
        if let Some((g, _)) = &geometry {
            self.paint_second_side(ui, pane, rect, g);
        }
        if matches!(self.view.compare, Compare::Split(_)) {
            let line = match self.view.compare {
                Compare::Split(Axis::TopBottom) => {
                    [pane.clip.left_bottom(), pane.clip.right_bottom()]
                }
                _ => [pane.clip.right_top(), pane.clip.right_bottom()],
            };
            ui.painter()
                .with_clip_rect(pane.area)
                .line_segment(line, egui::Stroke::new(1., Color32::from_white_alpha(200)));
        }
        badge(ui, pane.clip, "Before");
        badge(ui, panes.after.clip, "After");
        Some(rect)
    }
    /// Draws the render of the side beside the edit, the whole photo at `rect` and a
    /// 100% region over it, or a spinner while it has none.
    /// Draws the side beside the edit, Before or the reference photo, at `rect` in
    /// `pane`: its SDR rendition, and its HDR master over it when that is due.
    pub(in crate::app) fn paint_second_side(
        &mut self,
        ui: &egui::Ui,
        pane: Pane,
        rect: Rect,
        g: &Geometry,
    ) {
        let painter = ui.painter().with_clip_rect(pane.clip);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        let before = &self.preview.before;
        match &before.texture {
            Some(texture) => {
                painter.image(texture.id(), rect, uv, Color32::WHITE);
            }
            // Waiting for its first render; a 100% region alone is drawn below.
            None if before.region.is_none() => {
                let at = Rect::from_center_size(pane.clip.center(), Vec2::splat(18.));
                crate::app::ui::widgets::paint_spinner(ui, at, None);
            }
            None => {}
        }
        if let (TextureMode::Region(region), Some(texture)) = (before.mode, &before.region) {
            painter.image(texture.id(), region_on(rect, g, region), uv, Color32::WHITE);
        }
        let due = self.hdr_second_side_due();
        self.hdr_preview_ui(&painter, rect, g, due, true);
    }
    /// What the side beside the edit last asked its SDR rendition for.
    pub(in crate::app) fn second_side_submitted(&self) -> Option<SecondSide<'_>> {
        let job = self.preview.before.submitted.as_ref()?;
        Some(SecondSide {
            image: &job.image,
            recipe: &job.recipe,
            region: job.region,
            max_edge: job.max_edge,
        })
    }
    /// An edit is about to render: a Before render still running would hold it up
    /// on the renderer's one thread, so it is cancelled and asked for again behind it.
    pub(in crate::app) fn yield_before(&mut self) {
        let before = &mut self.preview.before;
        if before.task.is_running() {
            before.task.invalidate();
            before.submitted = None;
        }
    }
}

/// Where the 100% region `[x, y, w, h]` of a photo with geometry `g` sits on screen,
/// with the whole photo at `rect`.
pub(in crate::app) fn region_on(rect: Rect, g: &Geometry, [x, y, w, h]: [u32; 4]) -> Rect {
    let at = |px: u32, py: u32| {
        Pos2::new(
            rect.left() + px as f32 / g.width as f32 * rect.width(),
            rect.top() + py as f32 / g.height as f32 * rect.height(),
        )
    };
    Rect::from_min_max(at(x, y), at(x + w, y + h))
}

/// "Before" or "After" in the top-left corner of a side, as Lightroom labels them.
pub(in crate::app) fn badge(ui: &egui::Ui, side: Rect, text: &str) {
    let painter = ui.painter().with_clip_rect(side);
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(text::LABEL),
        Color32::WHITE,
    );
    let width = (galley.size().x + 20.).max(62.);
    let rect = Rect::from_min_size(side.left_top() + Vec2::splat(12.), Vec2::new(width, 25.));
    painter.rect_filled(rect, radius::CONTROL, Color32::from_black_alpha(190));
    painter.galley(rect.center() - galley.size() / 2., galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::navigator::Zoom;

    fn area() -> Rect {
        Rect::from_min_size(Pos2::new(10., 20.), Vec2::new(1000., 600.))
    }

    #[test]
    fn keys_toggle_their_view_and_move_between_views() {
        let lr = Compare::SideBySide(Axis::LeftRight);
        let tb = Compare::SideBySide(Axis::TopBottom);
        let split = Compare::Split(Axis::LeftRight);
        // Y twice: Left / Right, then the edit alone again.
        assert_eq!(Compare::Off.toggled(lr), lr);
        assert_eq!(lr.toggled(lr), Compare::Off);
        // Alt+Y and Shift+Y switch straight from one view to another.
        assert_eq!(lr.toggled(tb), tb);
        assert_eq!(tb.toggled(split), split);
        assert_eq!(split.toggled(Compare::BeforeOnly), Compare::BeforeOnly);
        assert!(lr.two_up() && split.two_up());
        assert!(!Compare::BeforeOnly.two_up() && Compare::BeforeOnly.shows_before());
        assert!(!Compare::Off.shows_before());
    }

    #[test]
    fn side_by_side_panes_share_the_area_and_splits_share_the_photo() {
        let a = area();
        let p = panes(Compare::SideBySide(Axis::LeftRight), a);
        let before = p.before.unwrap();
        assert_eq!(before.area.size(), p.after.area.size());
        assert!(before.area.right() < p.after.area.left());
        assert_eq!(before.area.left(), a.left());
        assert_eq!(p.after.area.right(), a.right());
        let p = panes(Compare::SideBySide(Axis::TopBottom), a);
        assert!(p.before.unwrap().area.bottom() < p.after.area.top());
        // Split: one photo laid out over the whole area, each side showing half.
        let p = panes(Compare::Split(Axis::LeftRight), a);
        let before = p.before.unwrap();
        assert_eq!((before.area, p.after.area), (a, a));
        assert_eq!(before.clip.right(), p.after.clip.left());
        assert_eq!(before.clip.width() + p.after.clip.width(), a.width());
        assert_eq!(panes(Compare::BeforeOnly, a).before, None);
        // Reference View: the reference where Before is, left or on top.
        for axis in [Axis::LeftRight, Axis::TopBottom] {
            let reference = Compare::Reference(axis);
            assert_eq!(panes(reference, a), panes(Compare::SideBySide(axis), a));
            assert!(reference.two_up() && reference.reference() && !reference.shows_before());
        }
    }

    #[test]
    fn zoom_and_pan_show_the_same_part_of_the_photo_on_both_sides() {
        let photo = Vec2::new(6000., 4000.);
        for zoom in [
            Zoom::default(),
            Zoom {
                on: true,
                level: 1.,
                pan: [0.3, 0.7],
            },
            Zoom {
                on: true,
                level: 0.25,
                pan: [0.9, 0.1],
            },
        ] {
            for view in [
                Compare::SideBySide(Axis::LeftRight),
                Compare::SideBySide(Axis::TopBottom),
                Compare::Split(Axis::TopBottom),
            ] {
                let p = panes(view, area());
                let after = zoom.photo_rect(p.after.area, photo, 2.);
                let before = before_rect(after, &p, true).unwrap();
                // Before, laid out in its own pane, lands where it is drawn.
                assert_eq!(before, zoom.photo_rect(p.before.unwrap().area, photo, 2.));
                // The middle of each pane shows the same point of the photo.
                let point = |rect: Rect, pane: Rect| (pane.center() - rect.min) / rect.size();
                let a = point(after, p.after.area);
                let b = point(before, p.before.unwrap().area);
                assert!((a - b).length() < 1e-5, "{view:?} {zoom:?}");
                // A click on Before zooms where the same point is on After.
                let click = before.min + before.size() * Vec2::new(0.25, 0.6);
                let on_after = to_after(click, before, after);
                assert!(
                    ((on_after - after.min) / after.size() - Vec2::new(0.25, 0.6)).length() < 1e-5
                );
            }
        }
    }

    fn editor() -> Editor {
        let ctx = egui::Context::default();
        Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        )
    }
    fn photo() -> Arc<CameraImage> {
        let (w, h) = (60, 40);
        Arc::new(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| [0.1 + (i % w) as f32 / 200.; 3])
                .collect(),
            metadata: crate::camera_data::Metadata {
                width: w,
                height: h,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        })
    }

    #[test]
    fn copying_before_to_after_and_swapping_are_history_steps() {
        let mut e = editor();
        let start = e.document.edit.recipe().clone();
        e.document.edit.setup_mut().exposure = 1.;
        e.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
        e.commit_edit(start, None);
        let edited = e.document.edit.recipe().clone();
        // After's settings to Before: the edit and its History are left as they are.
        e.document.before = Some(Recipe {
            contrast: 0.3,
            ..Default::default()
        });
        e.transfer(Transfer::AfterToBefore);
        assert_eq!(e.document.before.as_ref(), Some(&edited));
        assert_eq!(*e.document.edit.recipe(), edited);
        assert_eq!(e.document.edit.history().steps().0.len(), 1);
        // Before's settings to After: one step, keeping the edit's crop, undone by Undo.
        e.document.before = Some(Recipe {
            contrast: 0.3,
            ..Default::default()
        });
        let before = e.document.edit.recipe().clone();
        e.transfer(Transfer::BeforeToAfter);
        e.commit_edit(before, None);
        assert_eq!(e.document.edit.recipe().contrast, 0.3);
        assert_eq!(e.document.edit.recipe().exposure, 0.);
        assert_eq!(e.document.edit.recipe().crop, edited.crop);
        let (steps, _) = e.document.edit.history().steps();
        assert_eq!(steps.last().unwrap().name, "Copy Before Settings to After");
        e.undo();
        assert_eq!(*e.document.edit.recipe(), edited);
        // Swap: each side takes the other's settings, the edit's change as one step.
        let before = e.document.edit.recipe().clone();
        e.transfer(Transfer::Swap);
        e.commit_edit(before, None);
        assert_eq!(e.document.edit.recipe().contrast, 0.3);
        assert_eq!(e.document.before.as_ref(), Some(&edited));
        let (steps, _) = e.document.edit.history().steps();
        assert_eq!(steps.last().unwrap().name, "Swap Before and After Settings");
        assert_eq!(e.before_settings().exposure, 1.);
        // Copying settings that are already the edit's records nothing.
        let before = e.document.edit.recipe().clone();
        e.transfer(Transfer::AfterToBefore);
        e.transfer(Transfer::BeforeToAfter);
        e.commit_edit(before, None);
        // A preset only hovered lends Before its framing on screen, not to the edit.
        e.presets.preview = Some(Recipe {
            crop: [0.3, 0.3, 0.7, 0.7],
            ..Default::default()
        });
        e.transfer(Transfer::Swap);
        assert_eq!(e.document.edit.recipe().crop, edited.crop);
        e.presets.preview = None;
        e.transfer(Transfer::Swap);
        // Exposure and the swap; the undone copy went with the swap.
        assert_eq!(e.document.edit.history().steps().0.len(), 2);
    }

    #[test]
    fn before_starts_as_the_photo_was_and_can_be_set_from_history() {
        let mut e = editor();
        // No settings copied to it: the photo's starting settings, framed as the edit.
        let start = e.document.edit.recipe().clone();
        for value in [0.5, 1.] {
            let before = e.document.edit.recipe().clone();
            e.document.edit.setup_mut().exposure = value;
            e.commit_edit(before, None);
        }
        e.document.edit.setup_mut().set_auto_straighten(Some(2.));
        assert_eq!(e.before_settings().exposure, start.exposure);
        assert_eq!(e.before_settings().straighten, 2.);
        // Framed as the edit, Before's angle is Auto's as the edit's is, so copying
        // Before to After keeps it Auto.
        assert_eq!(
            e.before_settings().auto_straighten(),
            e.document.edit.recipe().auto_straighten()
        );
        // Copy History Step Settings to Before, from the first step.
        let edit = e.document.edit.recipe().clone();
        e.before_from_history(1);
        assert_eq!(e.before_settings().exposure, 0.5);
        // The edit and History stay where they were.
        assert_eq!(*e.document.edit.recipe(), edit);
        assert_eq!(e.document.edit.history().steps().1, 2);
        // Before belongs to the open photo.
        e.document.reset(None);
        assert!(e.document.before.is_none());
    }

    #[test]
    fn edits_reach_after_only_and_do_not_render_before_again() {
        let mut e = editor();
        e.document.set_image(photo());
        e.view.viewport = Vec2::new(120., 80.);
        e.set_compare(Compare::SideBySide(Axis::LeftRight));
        e.schedule();
        // A Before render still running gives way to the edit's, and is asked for
        // again behind it.
        assert!(e.preview.before.task.is_running());
        let running = e.preview.before.task.id();
        e.document.edit.setup_mut().exposure = 0.5;
        e.schedule();
        assert!(e.preview.before.task.id() > running);
        assert!(e.preview.before.submitted.is_some());
        // Once Before has rendered, edits leave it alone.
        let rendered = e.preview.before.task.id();
        e.preview.before.task.finish(rendered);
        let shown = e.before_settings();
        // Edits change After alone; Before is neither changed nor rendered again.
        e.document.edit.setup_mut().exposure = 1.;
        e.document.edit.setup_mut().contrast = 0.4;
        e.schedule();
        assert_eq!(e.before_settings(), shown);
        assert_eq!(e.preview.before.task.id(), rendered);
        // A crop changes what both sides frame: Before follows it, a hovered
        // preset's too.
        e.presets.preview = Some(Recipe {
            crop: [0.1, 0.3, 0.6, 0.9],
            ..Default::default()
        });
        assert_eq!(e.before_settings().crop, [0.1, 0.3, 0.6, 0.9]);
        e.presets.preview = None;
        e.document.edit.setup_mut().crop = [0.2, 0.2, 0.8, 0.8];
        e.schedule();
        assert_eq!(e.before_settings().crop, e.document.edit.recipe().crop);
        assert_eq!(e.before_settings().exposure, shown.exposure);
        assert!(e.preview.before.task.id() > rendered);
        // So does zooming in, which renders a 100% region of each.
        let reframed = e.preview.before.task.id();
        e.preview.before.task.finish(reframed);
        e.set_zoom(1.);
        assert!(e.preview.before.task.id() > reframed);
        assert!(matches!(
            e.preview.before.pending_mode,
            TextureMode::Region(_)
        ));
        // Back to the edit alone: Before's render is dropped, a lone 100% region too.
        e.preview.before.texture = None;
        e.preview.before.region = Some(Picture::presented(egui::TextureId::Managed(1), [1, 1]));
        e.preview.before.forget_job();
        e.set_compare(Compare::Off);
        e.schedule();
        assert!(e.preview.before.submitted.is_none());
        assert!(e.preview.before.region.is_none());
    }

    #[test]
    fn tools_work_on_the_edit_alone() {
        use crate::app::develop::state::Tool;
        let mut e = editor();
        e.view.tool = Tool::Mask;
        e.set_compare(Compare::Split(Axis::LeftRight));
        assert_eq!(e.view.tool, Tool::None);
        // Opening a tool goes back to the edit alone, as in Lightroom.
        e.view.toggle(Tool::Remove);
        e.leave_compare_for_tools();
        assert_eq!(e.view.compare, Compare::Off);
        // Before alone keeps the tool, as it always has.
        e.set_compare(Compare::BeforeOnly);
        e.leave_compare_for_tools();
        assert_eq!(e.view.compare, Compare::BeforeOnly);
    }

    #[test]
    fn y_keys_choose_the_view() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let press = |modifiers: egui::Modifiers, e: &mut Editor| {
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::Y,
                        physical_key: Some(egui::Key::Y),
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                    egui::Event::Key {
                        key: egui::Key::Y,
                        physical_key: Some(egui::Key::Y),
                        pressed: false,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
            output.textures_delta.clear();
            e.view.compare
        };
        let none = egui::Modifiers::NONE;
        assert_eq!(press(none, &mut e), Compare::SideBySide(Axis::LeftRight));
        assert_eq!(
            press(egui::Modifiers::ALT, &mut e),
            Compare::SideBySide(Axis::TopBottom)
        );
        assert_eq!(
            press(egui::Modifiers::SHIFT, &mut e),
            Compare::Split(Axis::LeftRight)
        );
        assert_eq!(press(egui::Modifiers::SHIFT, &mut e), Compare::Off);
        // Redo's Ctrl+Y elsewhere is not a view.
        assert_eq!(press(egui::Modifiers::COMMAND, &mut e), Compare::Off);
    }
}

#[cfg(test)]
#[path = "before_after_editor_tests.rs"]
mod editor_tests;
