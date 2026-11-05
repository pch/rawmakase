//! The Crop tool's guide overlays, Straighten ruler and aspect swap, as in Lightroom's
//! Crop & Straighten (docs/transform.md#crop-and-straighten).
use crate::model::recipe::Recipe;
use eframe::egui::{self, Pos2, Vec2};

/// Lightroom's crop guide overlays, in the order O cycles them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Guide {
    Grid,
    #[default]
    Thirds,
    Diagonal,
    Triangle,
    GoldenRatio,
    GoldenSpiral,
}
impl Guide {
    pub(in crate::app) const ALL: [Guide; 6] = [
        Guide::Grid,
        Guide::Thirds,
        Guide::Diagonal,
        Guide::Triangle,
        Guide::GoldenRatio,
        Guide::GoldenSpiral,
    ];
    pub(in crate::app) fn name(self) -> &'static str {
        match self {
            Guide::Grid => "Grid",
            Guide::Thirds => "Thirds",
            Guide::Diagonal => "Diagonal",
            Guide::Triangle => "Triangle",
            Guide::GoldenRatio => "Golden Ratio",
            Guide::GoldenSpiral => "Golden Spiral",
        }
    }
    /// Its name in the session file, which stays the same across versions.
    fn key(self) -> &'static str {
        match self {
            Guide::Grid => "grid",
            Guide::Thirds => "thirds",
            Guide::Diagonal => "diagonal",
            Guide::Triangle => "triangle",
            Guide::GoldenRatio => "golden-ratio",
            Guide::GoldenSpiral => "golden-spiral",
        }
    }
    /// How many ways round Shift+O can turn it: the triangle's diagonal runs either way,
    /// and the spiral can close on any corner.
    pub(in crate::app) fn orientations(self) -> u8 {
        match self {
            Guide::Triangle => 2,
            Guide::GoldenSpiral => 4,
            _ => 1,
        }
    }
}

/// When the guides show on the crop: Lightroom's Tools > Crop Guide Overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum GuideShow {
    Always,
    /// With the pointer over the photo, while the crop or the Straighten ruler is
    /// dragged, and for a moment after an overlay is picked.
    #[default]
    Auto,
    Never,
}
impl GuideShow {
    pub(in crate::app) const ALL: [GuideShow; 3] =
        [GuideShow::Always, GuideShow::Auto, GuideShow::Never];
    pub(in crate::app) fn name(self) -> &'static str {
        match self {
            GuideShow::Always => "Always",
            GuideShow::Auto => "Auto",
            GuideShow::Never => "Never",
        }
    }
    fn key(self) -> &'static str {
        match self {
            GuideShow::Always => "always",
            GuideShow::Auto => "auto",
            GuideShow::Never => "never",
        }
    }
}

/// What draws attention to the crop's guides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Attention {
    Away,
    /// The pointer is over the photo, as Lightroom's Auto Show needs.
    Hovered,
    /// The overlay was just picked, so the choice is seen even from the panel.
    JustChanged,
    /// The crop or the Straighten ruler is being dragged.
    Dragging,
}

/// The crop guide overlay chosen, a view preference saved in the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct CropGuides {
    pub(in crate::app) guide: Guide,
    /// Which way round it is, below `guide.orientations()`.
    pub(in crate::app) orientation: u8,
    pub(in crate::app) show: GuideShow,
}
impl CropGuides {
    /// O: the next overlay, the right way round.
    pub(in crate::app) fn cycle(&mut self) {
        let at = Guide::ALL
            .iter()
            .position(|g| *g == self.guide)
            .unwrap_or(0);
        self.guide = Guide::ALL[(at + 1) % Guide::ALL.len()];
        self.orientation = 0;
    }
    /// Shift+O: the overlay's next orientation.
    pub(in crate::app) fn turn(&mut self) {
        self.orientation = (self.orientation + 1) % self.guide.orientations();
    }
    pub(in crate::app) fn visible(&self, attention: Attention) -> bool {
        match self.show {
            GuideShow::Always => true,
            GuideShow::Auto => attention != Attention::Away,
            GuideShow::Never => false,
        }
    }
    pub(in crate::app) fn from_session(
        s: &crate::app::preferences::session::CropGuideLayout,
    ) -> Self {
        let guide = Guide::ALL
            .into_iter()
            .find(|g| g.key() == s.guide)
            .unwrap_or_default();
        let show = GuideShow::ALL
            .into_iter()
            .find(|v| v.key() == s.show)
            .unwrap_or_default();
        Self {
            guide,
            orientation: s.orientation.min(u32::from(guide.orientations() - 1)) as u8,
            show,
        }
    }
    pub(in crate::app) fn to_session(self) -> crate::app::preferences::session::CropGuideLayout {
        crate::app::preferences::session::CropGuideLayout {
            guide: self.guide.key().into(),
            orientation: self.orientation.into(),
            show: self.show.key().into(),
        }
    }
    /// The overlay's lines over a crop `size` points across, from its top-left corner:
    /// each a polyline.
    pub(in crate::app) fn lines(&self, size: Vec2) -> Vec<Vec<Pos2>> {
        let (w, h) = (size.x, size.y);
        let p = Pos2::new;
        let across = |t: f32| vec![p(w * t, 0.), p(w * t, h)];
        let down = |t: f32| vec![p(0., h * t), p(w, h * t)];
        let mut lines = match self.guide {
            Guide::Grid => {
                // Squares about a twelfth of the long edge.
                let step = w.max(h) / 12.;
                let mut lines = Vec::new();
                let mut x = step;
                while x < w - 0.5 {
                    lines.push(vec![p(x, 0.), p(x, h)]);
                    x += step;
                }
                let mut y = step;
                while y < h - 0.5 {
                    lines.push(vec![p(0., y), p(w, y)]);
                    y += step;
                }
                lines
            }
            Guide::Thirds => vec![
                across(1. / 3.),
                across(2. / 3.),
                down(1. / 3.),
                down(2. / 3.),
            ],
            Guide::GoldenRatio => {
                let t = 1. / std::f32::consts::GOLDEN_RATIO;
                vec![across(1. - t), across(t), down(1. - t), down(t)]
            }
            Guide::Diagonal => {
                // At 45° from each corner, as far as the opposite long edge.
                let d = w.min(h);
                vec![
                    vec![p(0., 0.), p(d, d)],
                    vec![p(w, 0.), p(w - d, d)],
                    vec![p(0., h), p(d, h - d)],
                    vec![p(w, h), p(w - d, h - d)],
                ]
            }
            Guide::Triangle => {
                // The diagonal, and from the other two corners the lines that meet it
                // at right angles.
                let foot = |c: Pos2| {
                    let t = (c.x * w + c.y * h) / (w * w + h * h);
                    p(t * w, t * h)
                };
                vec![
                    vec![p(0., 0.), p(w, h)],
                    vec![p(w, 0.), foot(p(w, 0.))],
                    vec![p(0., h), foot(p(0., h))],
                ]
            }
            Guide::GoldenSpiral => golden_spiral(size),
        };
        // Odd orientations mirror across; the spiral's 2 and 3 also mirror down.
        let mirror_x = self.orientation % 2 == 1;
        let mirror_y = self.orientation >= 2;
        for line in &mut lines {
            for q in line.iter_mut() {
                if mirror_x {
                    q.x = w - q.x;
                }
                if mirror_y {
                    q.y = h - q.y;
                }
            }
        }
        lines
    }
}

/// The golden spiral over a crop `size` across: squares cut from golden rectangles,
/// each with a quarter circle, closing on the lower right, stretched to the crop as
/// Lightroom's is.
fn golden_spiral(size: Vec2) -> Vec<Vec<Pos2>> {
    let phi = std::f32::consts::GOLDEN_RATIO;
    let (sx, sy) = (size.x / phi, size.y);
    let at = |x: f32, y: f32| Pos2::new(x * sx, y * sy);
    let mut lines = Vec::new();
    // The rectangle left to cut: left, top, width, height in a φ × 1 frame.
    let (mut x, mut y, mut w, mut h) = (0f32, 0f32, phi, 1f32);
    for i in 0..10 {
        // Square side, its quarter circle's centre and the circle's start and end angles
        // (y down), and the divider left behind.
        let (s, centre, from, divider, rest) = match i % 4 {
            0 => (
                h,
                (x + h, y + h),
                180f32,
                [(x + h, y), (x + h, y + h)],
                (x + h, y, w - h, h),
            ),
            1 => (
                w,
                (x, y + w),
                270.,
                [(x, y + w), (x + w, y + w)],
                (x, y + w, w, h - w),
            ),
            2 => (
                h,
                (x + w - h, y),
                0.,
                [(x + w - h, y), (x + w - h, y + h)],
                (x, y, w - h, h),
            ),
            _ => (
                w,
                (x + w, y + h - w),
                90.,
                [(x, y + h - w), (x + w, y + h - w)],
                (x, y, w, h - w),
            ),
        };
        let arc = (0..=12)
            .map(|k| {
                let a = (from + 90. * k as f32 / 12.).to_radians();
                at(centre.0 + s * a.cos(), centre.1 + s * a.sin())
            })
            .collect();
        lines.push(arc);
        lines.push(vec![
            at(divider[0].0, divider[0].1),
            at(divider[1].0, divider[1].1),
        ]);
        (x, y, w, h) = rest;
    }
    lines
}

/// The Straighten ruler: off, picked in the Crop panel and waiting for a drag, or being
/// drawn on the photo (from the panel, or with Cmd held), in screen points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::app) enum Ruler {
    #[default]
    Off,
    Armed,
    Drawing {
        from: Pos2,
        to: Pos2,
    },
}

/// Shortest Straighten ruler, in points, that sets an angle: a click or a slip of the
/// pointer is not a line.
pub(in crate::app) const MIN_RULER: f32 = 10.;

/// The Straighten angle that makes the ruler drawn from `from` to `to` on the photo as
/// shown level or, when it is nearer upright, plumb; `current` is the angle the photo is
/// shown at. None for a ruler too short to go by. Positions are in screen points, y down.
pub(in crate::app) fn ruler_angle(from: Pos2, to: Pos2, current: f32) -> Option<f32> {
    let d = to - from;
    if d.length() < MIN_RULER {
        return None;
    }
    // The ruler's angle, clockwise from level, folded to within a quarter turn of it.
    let angle = d.y.atan2(d.x).to_degrees();
    let off = (angle + 45.).rem_euclid(90.) - 45.;
    // Straighten turns the photo clockwise, so turning it back by `off` levels the line.
    Some((current - off).clamp(-45., 45.))
}

/// The crop with its width and height swapped (X), as large as fits on the photo and
/// centred where it was. `photo` is the straightened photo's width and height, which
/// the crop's 0–1 coordinates span.
pub(in crate::app) fn swap_orientation(crop: [f32; 4], photo: Vec2) -> [f32; 4] {
    let (cx, cy) = ((crop[0] + crop[2]) / 2., (crop[1] + crop[3]) / 2.);
    // Width and height in pixels, swapped, then shrunk to fit the photo.
    let (w, h) = ((crop[3] - crop[1]) * photo.y, (crop[2] - crop[0]) * photo.x);
    let k = (photo.x / w).min(photo.y / h).min(1.);
    let (w, h) = (w * k / photo.x, h * k / photo.y);
    let cx = cx.clamp(w / 2., 1. - w / 2.);
    let cy = cy.clamp(h / 2., 1. - h / 2.);
    [cx - w / 2., cy - h / 2., cx + w / 2., cy + h / 2.]
}

/// The aspect preset value after X: presets are long side over short in the photo's
/// own orientation, and their reciprocal stands for the other one. The photo's own
/// ratio (negative) becomes the ratio of the swapped photo; Free stays Free.
pub(in crate::app) fn swapped_aspect(aspect: f32, photo: Vec2) -> f32 {
    if aspect > 0. {
        1. / aspect
    } else if aspect < 0. {
        // As `fit_aspect` reads a preset: for a portrait photo it is short over long.
        let ratio = photo.x / photo.y;
        if photo.y > photo.x { ratio } else { 1. / ratio }
    } else {
        aspect
    }
}

/// The aspect the Crop tool's menu names for `crop`, in the menu's values: Original
/// at the photo's own ratio, a preset either way round (X swaps them), or else the
/// crop's exact ratio, shown as Custom. `photo` is the straightened photo's width and
/// height, which the crop's 0–1 coordinates span. Presets are long side over short
/// for a landscape photo and short over long for a portrait one, as
/// [`crate::app::Editor::crop_constraint`] reads them.
pub(in crate::app) fn crop_aspect_shown(crop: [f32; 4], photo: Vec2) -> f32 {
    let photo = photo.x / photo.y;
    let ratio = (crop[2] - crop[0]) / (crop[3] - crop[1]) * photo;
    let aspect = if photo < 1. { 1. / ratio } else { ratio };
    let near = |a: f32, b: f32| (a / b - 1.).abs() < 0.005;
    if near(ratio, photo) {
        return -1.;
    }
    crate::app::develop::panels::inspector::ASPECTS
        .iter()
        .map(|(a, ..)| *a)
        .filter(|a| *a > 0.)
        .flat_map(|a| [a, 1. / a])
        .find(|a| near(aspect, *a))
        .unwrap_or(aspect)
}

impl crate::app::Editor {
    /// The straightened photo's width and height, which the crop's 0–1 coordinates span.
    pub(in crate::app) fn crop_frame(&self) -> Option<Vec2> {
        let im = self.document.full()?;
        let r = Recipe {
            crop: [0., 0., 1., 1.],
            constrain_crop: false,
            ..self.document.edit.recipe().clone()
        };
        let g = crate::develop::Geometry::new(im, &r, 0);
        Some(Vec2::new(g.oriented_width, g.oriented_height))
    }
    /// The ratio every crop gesture keeps: the crop's width over its height in its 0–1
    /// coordinates on the straightened photo ([`Self::crop_frame`]). `None` when the
    /// aspect is unlocked (Free) or no photo is decoded.
    ///
    /// This is the only place a gesture learns whether, and to what, the crop is
    /// constrained: handle drags ask it, and so must any later gesture that resizes or
    /// moves the crop. None reads [`ViewState::aspect`] or the chosen aspect itself,
    /// or infers a ratio from the crop.
    ///
    /// [`ViewState::aspect`]: crate::app::develop::state::ViewState::aspect
    pub(in crate::app) fn crop_constraint(&self) -> Option<f32> {
        if self.view.aspect == 0. {
            return None;
        }
        let photo = self.crop_frame()?;
        let aspect = if self.view.aspect < 0. {
            photo.x / photo.y
        } else if photo.y > photo.x {
            1. / self.view.aspect
        } else {
            self.view.aspect
        };
        Some(aspect * photo.y / photo.x)
    }
    /// X: swaps the crop between portrait and landscape, keeping its aspect.
    pub(in crate::app) fn swap_crop_orientation(&mut self) {
        let Some(photo) = self.crop_frame() else {
            return;
        };
        let r = self.document.edit.recipe_mut();
        r.crop = swap_orientation(r.crop, photo);
        self.view.aspect = swapped_aspect(self.view.aspect, photo);
    }
    /// The Crop tool's keys: X swaps the crop's orientation, A locks or unlocks its
    /// aspect, O cycles the guide overlay
    /// and Shift+O turns it. The caller leaves them alone while a text field has focus.
    pub(in crate::app) fn crop_keys(&mut self, i: &egui::InputState) {
        // The modifiers held with each key press, which a quick release of Shift
        // before the frame does not change.
        for event in &i.events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            if modifiers.command || modifiers.alt || modifiers.ctrl {
                continue;
            }
            match key {
                egui::Key::X if !modifiers.shift => self.swap_crop_orientation(),
                egui::Key::A if !modifiers.shift => self.toggle_aspect_lock(),
                egui::Key::O => {
                    let mut guides = self.view.crop_guides;
                    if modifiers.shift {
                        guides.turn();
                    } else {
                        guides.cycle();
                    }
                    self.set_crop_guides(guides);
                }
                _ => {}
            }
        }
    }
    /// Picks `aspect` from the Crop tool's menu: the crop takes it now, even when an
    /// uncropped photo already opened with it, and uncropped photos open with it, here
    /// and after a relaunch.
    pub(in crate::app) fn choose_aspect(&mut self, aspect: f32) {
        self.view.aspect = aspect;
        self.fit_aspect();
        if aspect != self.view.chosen_aspect {
            self.view.chosen_aspect = aspect;
            let _ = self.save_session();
        }
    }
    /// The padlock (A): unlocks the aspect, as choosing Free does, or locks the crop at
    /// the ratio it has now. A ratio the menu has (Original, or a preset either way
    /// round) is then the chosen aspect, as if picked from the menu. A Custom ratio is
    /// locked on this photo only, with Original as the choice: the session has no name
    /// for it, and an uncropped photo opening locked is then left uncropped rather than
    /// cropped to a ratio from another photo.
    pub(in crate::app) fn toggle_aspect_lock(&mut self) {
        if self.view.chosen_aspect != 0. {
            self.choose_aspect(0.);
            return;
        }
        let Some(photo) = self.crop_frame() else {
            return;
        };
        let shown = crop_aspect_shown(self.document.edit.recipe().crop, photo);
        let chosen = crate::app::develop::panels::inspector::ASPECTS
            .iter()
            .map(|(a, ..)| *a)
            .find(|a| *a == shown || (*a > 0. && 1. / *a == shown))
            .unwrap_or(-1.);
        self.view.aspect = shown;
        self.view.chosen_aspect = chosen;
        let _ = self.save_session();
    }
    /// Changes the guide overlay, saved in the session as a view preference.
    pub(in crate::app) fn set_crop_guides(&mut self, guides: CropGuides) {
        if guides != self.view.crop_guides {
            self.view.crop_guides = guides;
            self.view.crop_guides_changed = Some(std::time::Instant::now());
            let _ = self.save_session();
        }
    }

    /// The Crop panel's Auto: measures the angle Upright's Level would turn the photo by,
    /// off the UI thread; it arrives as [`Event::Straighten`]. Upright's mode is left as
    /// it is.
    ///
    /// [`Event::Straighten`]: crate::app::worker::Event::Straighten
    pub(in crate::app) fn start_auto_straighten(&mut self) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let (generation, _) = self.document.straighten.start();
        let id = self.document.load_id();
        let base = self.document.edit.recipe().clone();
        let analysed = base.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = Ok(crate::develop::upright::straighten_angle(&im, &base));
                let _ = tx.send(crate::app::worker::Event::Straighten {
                    id,
                    generation,
                    analysed: Box::new(base),
                    result,
                });
            },
            // A panic still sends a result, so Auto does not stay disabled.
            move |tx, _| {
                let _ = tx.send(crate::app::worker::Event::Straighten {
                    id,
                    generation,
                    analysed: Box::new(analysed),
                    result: Err("Auto straighten: the analysis failed unexpectedly".to_owned()),
                });
            },
        );
    }
    /// Measures the photo when its Straighten waits for Auto, as after pasting or
    /// undoing to the Crop of a photo whose angle was Auto.
    pub(in crate::app) fn ensure_straighten(&mut self) {
        if self.document.edit.recipe().straighten_pending()
            && !self.document.straighten.is_running()
        {
            self.start_auto_straighten();
        }
    }
    /// Sets the angle Auto straighten found as one History step. One measured before the
    /// photo was turned, flipped or its lens corrections changed no longer fits it, so the
    /// photo is measured again.
    pub(in crate::app) fn auto_straighten_ready(
        &mut self,
        generation: u64,
        analysed: &Recipe,
        result: Result<Option<f32>, String>,
    ) {
        if generation != self.document.straighten.id() {
            return;
        }
        self.document.straighten.finish(generation);
        if analysed.straighten_pending() {
            self.pending_straighten_measured(analysed, result);
            return;
        }
        if crate::app::develop::upright::inputs(analysed)
            != crate::app::develop::upright::inputs(self.document.edit.recipe())
        {
            self.start_auto_straighten();
            return;
        }
        let angle = match result {
            Ok(Some(angle)) => angle,
            Ok(None) => {
                self.messages
                    .warn("Auto straighten found no horizon or verticals to level");
                return;
            }
            Err(e) => {
                self.messages.fail(e);
                return;
            }
        };
        // A drag still under way is recorded first, so undoing it keeps the angle.
        if self.document.edit.history().in_gesture() {
            self.document.edit.finish_gesture();
        }
        self.change_edit(
            Some(crate::app::history::Step::new("Straighten", "Auto")),
            |r| {
                r.set_auto_straighten(Some(angle));
            },
        );
    }
    /// A Straighten Auto copied from another photo, measured on this one. As Upright's
    /// analysis, it is part of the photo rather than an edit: every state in History
    /// still waiting for it, on the photo as measured, gets it, so undoing the Paste
    /// leaves nothing half-applied. One that found nothing leaves the angle as it was.
    fn pending_straighten_measured(
        &mut self,
        analysed: &Recipe,
        result: Result<Option<f32>, String>,
    ) {
        let (measured, failure) = match result {
            Ok(measured) => (measured, None),
            Err(e) => (None, Some(e)),
        };
        let fits = |r: &Recipe| {
            r.straighten_pending()
                && crate::app::develop::upright::inputs(r)
                    == crate::app::develop::upright::inputs(analysed)
        };
        let mut shown = false;
        for (i, r) in self
            .document
            .edit
            .states_mut()
            // A Before copied from the edit before the measurement arrived.
            .chain(self.document.before.as_mut())
            .enumerate()
        {
            if fits(r) {
                r.set_auto_straighten(measured);
                shown |= i == 0;
            }
        }
        // Only the photo as shown speaks in the top bar.
        if shown && measured.is_none() {
            self.messages.warn(failure.unwrap_or_else(|| {
                "Auto straighten found no horizon or verticals to level".into()
            }));
        }
        self.document.edit.save_state_mut().mark_changed();
        self.schedule();
        // Turned or flipped meanwhile: measured again as it is now.
        self.ensure_straighten();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_sets_the_angle_that_levels_or_plumbs_the_line() {
        let at = |x: f32, y: f32| Pos2::new(x, y);
        let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-3);
        // A horizon falling 5° to the right (clockwise) is levelled by turning back 5°,
        // drawn either way.
        let fall = 5f32.to_radians().tan() * 300.;
        assert!(near(ruler_angle(at(0., 0.), at(300., fall), 0.), -5.));
        assert!(near(ruler_angle(at(300., fall), at(0., 0.), 0.), -5.));
        // Rising to the right: turned the other way.
        assert!(near(ruler_angle(at(0., fall), at(300., 0.), 0.), 5.));
        // On top of the angle the photo is shown at.
        assert!(near(ruler_angle(at(0., 0.), at(300., fall), 2.), -3.));
        // Nearly upright lines become plumb, drawn up or down.
        let lean = 4f32.to_radians().tan() * 300.;
        assert!(near(ruler_angle(at(0., 0.), at(lean, 300.), 0.), 4.));
        assert!(near(ruler_angle(at(lean, 300.), at(0., 0.), 0.), 4.));
        assert!(near(ruler_angle(at(lean, 0.), at(0., 300.), 0.), -4.));
        // Level and plumb lines change nothing.
        assert!(near(ruler_angle(at(0., 0.), at(300., 0.), 1.5), 1.5));
        assert!(near(ruler_angle(at(0., 0.), at(0., 300.), 1.5), 1.5));
        // Clicks and slips of the pointer are ignored.
        assert_eq!(ruler_angle(at(5., 5.), at(5., 5.), 0.), None);
        assert_eq!(ruler_angle(at(5., 5.), at(9., 8.), 0.), None);
        // Never beyond Straighten's range.
        assert!(near(ruler_angle(at(0., 290.), at(300., 0.), 40.), 45.));
    }

    #[test]
    fn swapping_keeps_the_aspect_and_stays_on_the_photo() {
        let photo = Vec2::new(6000., 4000.);
        let ratio = |c: [f32; 4]| (c[2] - c[0]) * photo.x / ((c[3] - c[1]) * photo.y);
        for crop in [
            [0., 0., 1., 1.],
            [0.1, 0.2, 0.5, 0.6],
            [0.6, 0.05, 0.98, 0.5],
            [0., 0.223, 1., 0.777],
        ] {
            let swapped = swap_orientation(crop, photo);
            assert!(
                (ratio(swapped) * ratio(crop) - 1.).abs() < 1e-4,
                "{crop:?} -> {swapped:?}"
            );
            assert!(
                swapped[0] >= -1e-6 && swapped[1] >= -1e-6,
                "{crop:?} -> {swapped:?}"
            );
            assert!(
                swapped[2] <= 1. + 1e-6 && swapped[3] <= 1. + 1e-6,
                "{crop:?} -> {swapped:?}"
            );
            // Twice is the same aspect again.
            let back = swap_orientation(swapped, photo);
            assert!((ratio(back) / ratio(crop) - 1.).abs() < 1e-4);
        }
        // A small crop keeps its size and centre.
        let small = swap_orientation([0.4, 0.4, 0.5, 0.5], photo);
        let c = [(small[0] + small[2]) / 2., (small[1] + small[3]) / 2.];
        assert!((c[0] - 0.45).abs() < 1e-6 && (c[1] - 0.45).abs() < 1e-6);
        assert!(((small[2] - small[0]) * photo.x - 0.1 * photo.y).abs() < 1e-2);
    }

    #[test]
    fn swapped_presets_stand_for_the_other_orientation() {
        let landscape = Vec2::new(6000., 4000.);
        let portrait = Vec2::new(4000., 6000.);
        assert_eq!(swapped_aspect(1.25, landscape), 0.8);
        assert_eq!(swapped_aspect(0.8, landscape), 1.25);
        assert_eq!(swapped_aspect(0., landscape), 0.);
        // Original on a 3:2 photo: 2:3, read as a preset is for each orientation.
        assert!((swapped_aspect(-1., landscape) - 2. / 3.).abs() < 1e-6);
        assert!((swapped_aspect(-1., portrait) - 2. / 3.).abs() < 1e-6);
    }

    #[test]
    fn o_cycles_the_overlays_in_lightrooms_order_and_shift_o_turns_them() {
        let mut g = CropGuides::default();
        assert_eq!(g.guide, Guide::Thirds);
        let mut seen = vec![g.guide];
        for _ in 0..6 {
            g.cycle();
            seen.push(g.guide);
        }
        use Guide::*;
        assert_eq!(
            seen,
            [
                Thirds,
                Diagonal,
                Triangle,
                GoldenRatio,
                GoldenSpiral,
                Grid,
                Thirds
            ]
        );
        // Shift+O goes round the overlay's orientations; O starts the next one upright.
        g.guide = GoldenSpiral;
        let turns: Vec<u8> = (0..5)
            .map(|_| {
                g.turn();
                g.orientation
            })
            .collect();
        assert_eq!(turns, [1, 2, 3, 0, 1]);
        g.cycle();
        assert_eq!((g.guide, g.orientation), (Grid, 0));
        g.turn();
        assert_eq!(g.orientation, 0, "the grid has one orientation");
        g.guide = Triangle;
        g.turn();
        g.turn();
        assert_eq!(g.orientation, 0, "the triangle has two");
    }

    #[test]
    fn orientations_mirror_the_overlay_and_stay_on_the_crop() {
        let size = Vec2::new(300., 200.);
        for guide in Guide::ALL {
            let mut shapes = Vec::new();
            for orientation in 0..guide.orientations() {
                let g = CropGuides {
                    guide,
                    orientation,
                    ..Default::default()
                };
                let lines = g.lines(size);
                assert!(!lines.is_empty(), "{guide:?}");
                for q in lines.iter().flatten() {
                    assert!(
                        (-1e-3..=300.001).contains(&q.x) && (-1e-3..=200.001).contains(&q.y),
                        "{guide:?} {orientation}: {q:?}"
                    );
                }
                shapes.push(format!("{lines:?}"));
            }
            shapes.dedup();
            assert_eq!(shapes.len(), guide.orientations() as usize, "{guide:?}");
        }
        // The spiral closes on its pole, where the crop's diagonal meets the line at
        // right angles to it from the first square's corner: (5 + √5) / 10 across and
        // down.
        let spiral = CropGuides {
            guide: Guide::GoldenSpiral,
            ..Default::default()
        }
        .lines(size);
        let end = *spiral[spiral.len() - 2].last().unwrap();
        let pole = (5. + 5f32.sqrt()) / 10.;
        assert!(
            (end.x / 300. - pole).abs() < 0.01 && (end.y / 200. - pole).abs() < 0.01,
            "{end:?}"
        );
    }

    #[test]
    fn guides_show_always_with_the_pointer_on_the_photo_or_never() {
        let mut g = CropGuides::default();
        // Auto: away from the photo they hide; over it, dragging, or just after a new
        // overlay is picked, they show, so picking one in the panel shows it.
        assert!(!g.visible(Attention::Away));
        for shown in [
            Attention::Hovered,
            Attention::Dragging,
            Attention::JustChanged,
        ] {
            assert!(g.visible(shown), "{shown:?}");
        }
        g.show = GuideShow::Always;
        assert!(g.visible(Attention::Away));
        g.show = GuideShow::Never;
        assert!(!g.visible(Attention::Dragging));
    }

    #[test]
    fn guides_round_trip_through_the_session() {
        let g = CropGuides {
            guide: Guide::GoldenSpiral,
            orientation: 3,
            show: GuideShow::Always,
        };
        assert_eq!(CropGuides::from_session(&g.to_session()), g);
        // Names it does not know read as the defaults.
        let unknown = crate::app::preferences::session::CropGuideLayout {
            guide: "aspect-ratios".into(),
            orientation: 7,
            show: "sometimes".into(),
        };
        assert_eq!(CropGuides::from_session(&unknown), CropGuides::default());
    }
}

#[cfg(test)]
#[path = "crop_tool_editor_tests.rs"]
mod editor_tests;
