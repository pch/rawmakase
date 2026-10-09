use crate::model::recipe::Recipe;
use crate::{camera_data::CameraImage, model::transform::Transform};

/// The crop as rendered, `[left, top, right, bottom]`: with Constrain Crop, the crop
/// constrained to the photo, which is what Lightroom stores; otherwise the recipe's.
pub fn rendered_crop(r: &Recipe, m: &crate::camera_data::Metadata) -> [f32; 4] {
    if r.constrain_crop {
        Geometry::for_metadata(m, &r.as_rendered()).crop()
    } else {
        r.crop
    }
}
/// The Transform sliders as the homography the renderer samples through.
pub(crate) trait TransformHomography {
    /// Homography from output to source coordinates, both centred, y down, in units
    /// of the long edge. The forward (source-to-output) matrix was fitted to Camera Raw
    /// 18.6 renders (docs/transform.md): Rotate applies first, then Vertical and
    /// Horizontal together, then Aspect, Scale and the offsets. With q = (h, v) and
    /// s = |q|, Vertical and Horizontal are [[I + e(s) q qᵀ / s², 0], [−qᵀ, 1]] with
    /// e(s) = 0.0391 s² + 0.9251 s³ − 0.2827 s⁴; Aspect scales y by 2^(0.137 a) and x by
    /// the inverse; offsets move by 0.811 of the image size, positive Y upward.
    fn inverse(&self, width: f32, height: f32) -> [[f32; 3]; 3];
}
impl TransformHomography for Transform {
    fn inverse(&self, width: f32, height: f32) -> [[f32; 3]; 3] {
        let (qx, qy) = (self.horizontal, self.vertical);
        let s = qx.hypot(qy);
        // e(s) / s²
        let e = 0.0391 + 0.9251 * s - 0.2827 * s * s;
        let perspective = [
            [1. + e * qx * qx, e * qx * qy, 0.],
            [e * qx * qy, 1. + e * qy * qy, 0.],
            [-qx, -qy, 1.],
        ];
        let (sr, cr) = self.rotate.to_radians().sin_cos();
        let rotate = [[cr, -sr, 0.], [sr, cr, 0.], [0., 0., 1.]];
        let a = 2f32.powf(0.137 * self.aspect);
        let scale = [
            [self.scale / a, 0., 0.],
            [0., self.scale * a, 0.],
            [0., 0., 1.],
        ];
        let offset = [
            [1., 0., self.offset_x * 0.811 * width],
            [0., 1., -self.offset_y * 0.811 * height],
            [0., 0., 1.],
        ];
        let forward = mat(offset, mat(scale, mat(perspective, rotate)));
        crate::color::inverse(forward)
    }
}
/// Lightroom's manual lens Distortion, measured on Camera Raw 18.7 renders of the
/// synthetic chart: an output position at radius r (1 at the frame's corners) samples
/// the photo at radius r·(1 + k·(1 − r²)), with k = 0.4 × amount for positive amounts
/// and 0.5 × amount for negative ones. Corners stay put; positive amounts pull the
/// edges' middles in from outside the photo, which shows white, as Lightroom shows it
/// without Constrain Crop. It applies in the frame as recorded, after Upright, the
/// Transform sliders and the crop take an output position back to it, and before the
/// lens profile (docs/lens-corrections.md#manual-distortion).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ManualDistortion {
    k: f32,
    /// Half the frame's width and height over its half diagonal.
    axes: [f32; 2],
}
impl ManualDistortion {
    /// For Lightroom's amount (−1 to 1) on a `width` by `height` frame; `None` at 0.
    fn new(amount: f32, width: f32, height: f32) -> Option<Self> {
        if amount == 0. {
            return None;
        }
        let diagonal = width.hypot(height);
        Some(Self {
            k: amount * if amount > 0. { 0.4 } else { 0.5 },
            axes: [width / diagonal, height / diagonal],
        })
    }
    /// The source position (0–1 of the frame) that output position (`x`, `y`) samples.
    fn source(&self, x: f32, y: f32) -> [f32; 2] {
        let rho = self.radius_squared(x, y).sqrt();
        let g = if rho > 1e-6 {
            self.source_radius(rho) / rho
        } else {
            1. + self.k
        };
        [0.5 + (x - 0.5) * g, 0.5 + (y - 0.5) * g]
    }
    /// The output position that samples source position (`x`, `y`): the inverse of
    /// [`Self::source`], its radius found by bisection on the increasing radial map.
    fn output(&self, x: f32, y: f32) -> [f32; 2] {
        let target = self.radius_squared(x, y).sqrt();
        if target < 1e-6 {
            return [x, y];
        }
        // The map's slope is at least min(1, 1 + k) up to `turn`, and 1 beyond it, where
        // it lies below rho by less than `turn`.
        let (mut lo, mut hi) = (
            0f32,
            target / (1. + self.k.min(0.)) + self.turn().unwrap_or(0.),
        );
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.source_radius(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let scale = 0.5 * (lo + hi) / target;
        [0.5 + (x - 0.5) * scale, 0.5 + (y - 0.5) * scale]
    }
    /// Source radius for output radius `rho` (1 at the frame's corners): Camera Raw's
    /// rho·(1 + k(1 − rho²)) up to where it stops increasing (for positive amounts, just
    /// beyond the corners), and a slope of 1 past that, so positions outside the frame
    /// map one to one and fall outside the photo.
    fn source_radius(&self, rho: f32) -> f32 {
        let f = |r: f32| r * (1. + self.k * (1. - r * r));
        match self.turn() {
            Some(turn) if rho > turn => f(turn) + rho - turn,
            _ => f(rho),
        }
    }
    /// Where the radial map stops increasing, for positive amounts.
    fn turn(&self) -> Option<f32> {
        (self.k > 0.).then(|| ((1. + self.k) / (3. * self.k)).sqrt())
    }
    fn radius_squared(&self, x: f32, y: f32) -> f32 {
        let dx = (x - 0.5) * 2. * self.axes[0];
        let dy = (y - 0.5) * 2. * self.axes[1];
        dx * dx + dy * dy
    }
}

const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
fn mat(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}
/// Inverse map from output normalized coordinates to un-oriented decoded pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub oriented_width: f32,
    pub oriented_height: f32,
    crop: [f32; 4],
    angle: f32,
    zoom: f32,
    turns: u8,
    flip_x: bool,
    flip_y: bool,
    source_width: u32,
    source_height: u32,
    inset: [f32; 4],
    /// Output-to-source homography of Upright and the Transform sliders, when not the
    /// identity, in 0–1 coordinates of the photo as recorded (after flips and turns).
    transform: Option<[[f32; 3]; 3]>,
    /// Its inverse, source to output.
    forward: Option<[[f32; 3]; 3]>,
    /// Lightroom's manual Distortion, after the homography on the way to the source.
    manual: Option<ManualDistortion>,
}
impl Geometry {
    pub fn new(im: &CameraImage, r: &Recipe, max_edge: u32) -> Self {
        Self::with_frame(
            crate::model::image_frame::ImageFrame::new(im),
            [im.width, im.height],
            r,
            max_edge,
        )
    }
    /// The geometry from metadata alone, for positions and the crop as rendered before
    /// the photo is decoded.
    pub fn for_metadata(m: &crate::camera_data::Metadata, r: &Recipe) -> Self {
        Self::with_frame(
            crate::model::image_frame::ImageFrame::for_metadata(m),
            [m.width.max(1), m.height.max(1)],
            r,
            0,
        )
    }
    /// For a photo decoded at `size` pixels with `frame`.
    fn with_frame(
        frame: crate::model::image_frame::ImageFrame,
        size: [u32; 2],
        r: &Recipe,
        max_edge: u32,
    ) -> Self {
        let turns = (frame.turns + r.rotation) % 4;
        let [w, h] = frame.size();
        let (ow, oh) = if r.rotation % 2 == 1 { (h, w) } else { (w, h) };
        let angle = r.straighten.to_radians();
        let (s, c) = angle.sin_cos();
        let zoom = (c.abs() + s.abs() * oh / ow).max(c.abs() + s.abs() * ow / oh);
        let [width, height] = size;
        let (frame_width, frame_height) = (
            width as f32 * frame.inset[2],
            height as f32 * frame.inset[3],
        );
        let transform = Self::homography(r, frame_width, frame_height);
        // Off with the Lens Corrections panel, for callers that pass the stored recipe
        // (the white balance picker) rather than the rendered one.
        let lens_panel = r.panels.state(crate::model::panels::Panel::LensCorrections);
        let manual = (lens_panel == crate::model::panels::PanelState::On)
            .then(|| ManualDistortion::new(r.lens_manual_distortion, frame_width, frame_height))
            .flatten();
        let mut g = Self {
            width: 0,
            height: 0,
            oriented_width: ow,
            oriented_height: oh,
            crop: r.crop,
            angle,
            zoom,
            turns,
            flip_x: r.flip_x,
            flip_y: r.flip_y,
            source_width: width,
            source_height: height,
            inset: frame.inset,
            transform,
            forward: transform.map(crate::color::inverse),
            manual,
        };
        if r.constrain_crop && (g.transform.is_some() || g.manual.is_some()) {
            g.crop = g.constrained_crop();
        }
        let w = ow * (g.crop[2] - g.crop[0]);
        let h = oh * (g.crop[3] - g.crop[1]);
        let factor = if max_edge > 0 {
            (max_edge as f32 / w.max(h)).min(1.)
        } else {
            1.
        };
        g.width = (w * factor).round().max(1.) as u32;
        g.height = (h * factor).round().max(1.) as u32;
        g
    }
    /// The crop Constrain Crop renders for this geometry's crop. The viewport builds a
    /// geometry several times a frame, so the last one found is remembered.
    fn constrained_crop(&self) -> [f32; 4] {
        thread_local! {
            static LAST: std::cell::Cell<Option<(Geometry, [f32; 4])>> =
                const { std::cell::Cell::new(None) };
        }
        if let Some((key, crop)) = LAST.get()
            && key == *self
        {
            return crop;
        }
        let crop = super::crop_constraint::largest_covered(self.crop, self).unwrap_or(self.crop);
        LAST.set(Some((*self, crop)));
        crop
    }
    /// The crop as rendered (0–1 of the straightened photo): the recipe's, or with
    /// Constrain Crop the part of it that has a source pixel everywhere.
    pub fn crop(&self) -> [f32; 4] {
        self.crop
    }
    /// Upright followed by the Transform sliders, output to source, in 0–1 coordinates
    /// of the photo as recorded (`width` by `height`). Camera Raw applies both in that
    /// frame, before rotating or flipping the photo, so on a photo turned to portrait
    /// Vertical keystones across the screen (docs/transform.md).
    fn homography(r: &Recipe, width: f32, height: f32) -> Option<[[f32; 3]; 3]> {
        let upright = r.upright.correction();
        if upright.is_none() && r.transform.is_identity() {
            return None;
        }
        let mut h = IDENTITY;
        if !r.transform.is_identity() {
            let (sx, sy) = (width / width.max(height), height / width.max(height));
            // 0–1 coordinates to the sliders' centred, long-edge units.
            let centred = [[sx, 0., -0.5 * sx], [0., sy, -0.5 * sy], [0., 0., 1.]];
            h = mat(
                crate::color::inverse(centred),
                mat(r.transform.inverse(sx, sy), centred),
            );
        }
        if let Some(u) = upright {
            h = mat(crate::color::inverse(u), h);
        }
        Some(h)
    }
    /// Whether a transformed or manually distorted output position has no source pixel;
    /// Lightroom shows white there.
    /// Pixels beyond the camera's default crop count as outside, as in Lightroom.
    pub fn outside(&self, x: f32, y: f32) -> bool {
        let (w, h) = (self.source_width as f32, self.source_height as f32);
        let [left, top, width, height] = self.inset;
        (self.transform.is_some() || self.manual.is_some())
            && (x < left * w - 0.5
                || y < top * h - 0.5
                || x > (left + width) * w - 0.5
                || y > (top + height) * h - 0.5)
    }
    /// The fields `source` reads, laid out for `gpu/local.wgsl` (`S_CROP` to the end of
    /// `S_HOMOGRAPHY`): crop, oriented size, zoom, sine and cosine, turns, flips, inset,
    /// then whether there is a transform and its homography.
    pub(crate) fn gpu_params(&self) -> [f32; 26] {
        let (s, c) = self.angle.sin_cos();
        let h = self.transform.unwrap_or([[0.; 3]; 3]);
        let mut out = [0.; 26];
        out[..4].copy_from_slice(&self.crop);
        out[4..12].copy_from_slice(&[
            self.oriented_width,
            self.oriented_height,
            self.zoom,
            s,
            c,
            self.turns as f32,
            self.flip_x as u8 as f32,
            self.flip_y as u8 as f32,
        ]);
        out[12..16].copy_from_slice(&self.inset);
        out[16] = self.transform.is_some() as u8 as f32;
        out[17..].copy_from_slice(h.as_flattened());
        out
    }
    /// Manual Distortion as `gpu/local.wgsl` reads it (`S_MANUAL`): k, 0 when off, and
    /// the frame's axes.
    pub(crate) fn gpu_manual(&self) -> [f32; 3] {
        self.manual.map_or([0.; 3], |m| [m.k, m.axes[0], m.axes[1]])
    }
    pub fn source(&self, u: f32, v: f32) -> [f32; 2] {
        let [x, y] = self.recorded(
            self.crop[0] + u * (self.crop[2] - self.crop[0]),
            self.crop[1] + v * (self.crop[3] - self.crop[1]),
        );
        [
            (self.inset[0] + x * self.inset[2]) * self.source_width as f32 - 0.5,
            (self.inset[1] + y * self.inset[3]) * self.source_height as f32 - 0.5,
        ]
    }
    /// The position in the frame as recorded (0–1, inside the camera's default crop) of
    /// crop-space position (`x`, `y`), 0–1 over the straightened, uncropped photo.
    fn recorded(&self, x: f32, y: f32) -> [f32; 2] {
        let [x, y] = self.unwarped(x, y);
        self.manual.map_or([x, y], |m| m.source(x, y))
    }
    /// The position in Upright's frame (0–1 of the frame as recorded, after lens
    /// corrections and before Upright and the Transform sliders) shown at view position
    /// (`u`, `v`): where Guided Upright's guides are kept.
    pub fn upright_frame(&self, u: f32, v: f32) -> [f32; 2] {
        self.unwarped(
            self.crop[0] + u * (self.crop[2] - self.crop[0]),
            self.crop[1] + v * (self.crop[3] - self.crop[1]),
        )
    }
    /// View position of position `p` in Upright's frame: the inverse of
    /// [`Self::upright_frame`].
    pub fn from_upright_frame(&self, p: [f32; 2]) -> [f32; 2] {
        self.frame_to_view(p[0], p[1])
    }
    /// [`Self::recorded`] up to Upright's frame, before manual Distortion.
    fn unwarped(&self, x: f32, y: f32) -> [f32; 2] {
        let x = (x - 0.5) * self.oriented_width / self.zoom;
        let y = (y - 0.5) * self.oriented_height / self.zoom;
        let (s, c) = self.angle.sin_cos();
        let nx = (c * x + s * y) / self.oriented_width + 0.5;
        let ny = (-s * x + c * y) / self.oriented_height + 0.5;
        let (mut x, mut y) = (nx, ny);
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        let (mut x, mut y) = match self.turns {
            1 => (y, 1. - x),
            2 => (1. - x, 1. - y),
            3 => (1. - y, x),
            _ => (x, y),
        };
        if let Some(h) = &self.transform {
            let w = h[2][0] * x + h[2][1] * y + h[2][2];
            // Points behind the virtual camera have no source; send them off-image.
            let w = if w > 1e-6 { w } else { 1e-6 };
            (x, y) = (
                (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
                (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
            );
        }
        [x, y]
    }
    /// Output position (0–1 over the view) of decoded sample coordinates (`x`, `y`):
    /// the inverse of [`Self::source`].
    pub fn view(&self, x: f32, y: f32) -> [f32; 2] {
        let x = ((x + 0.5) / self.source_width as f32 - self.inset[0]) / self.inset[2];
        let y = ((y + 0.5) / self.source_height as f32 - self.inset[1]) / self.inset[3];
        let [x, y] = self.manual.map_or([x, y], |m| m.output(x, y));
        self.frame_to_view(x, y)
    }
    /// View position of position (`x`, `y`) in Upright's frame.
    fn frame_to_view(&self, x: f32, y: f32) -> [f32; 2] {
        let (x, y) = match &self.forward {
            Some(f) => {
                let w = f[2][0] * x + f[2][1] * y + f[2][2];
                let w = if w.abs() > 1e-6 { w } else { 1e-6 };
                (
                    (f[0][0] * x + f[0][1] * y + f[0][2]) / w,
                    (f[1][0] * x + f[1][1] * y + f[1][2]) / w,
                )
            }
            None => (x, y),
        };
        let [mut x, mut y] = crate::model::image_frame::turn((4 - self.turns) % 4, x, y);
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        let big_x = (x - 0.5) * self.oriented_width;
        let big_y = (y - 0.5) * self.oriented_height;
        let (s, c) = self.angle.sin_cos();
        let x0 = c * big_x - s * big_y;
        let y0 = s * big_x + c * big_y;
        let xc = x0 * self.zoom / self.oriented_width + 0.5;
        let yc = y0 * self.zoom / self.oriented_height + 0.5;
        [
            (xc - self.crop[0]) / (self.crop[2] - self.crop[0]),
            (yc - self.crop[1]) / (self.crop[3] - self.crop[1]),
        ]
    }
}

/// Margin, in fractions of the frame, that Constrain Crop keeps from its edges, for
/// rounding and for manual Distortion bending a crop's edges between the positions
/// checked.
const CONSTRAIN_MARGIN: f32 = 1e-4;
impl super::crop_constraint::Covers for Geometry {
    fn covers(&self, x: f32, y: f32) -> bool {
        let inside = CONSTRAIN_MARGIN..=1. - CONSTRAIN_MARGIN;
        let [x, y] = self.recorded(x, y);
        inside.contains(&x) && inside.contains(&y)
    }
    /// Straightening, Upright and the Transform map straight lines to straight lines,
    /// and the frame they map back to is a rectangle.
    fn straight_edges(&self) -> bool {
        self.manual.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::transform::display_axes;

    #[test]
    fn sliders_show_along_the_displayed_axes_as_in_lightroom() {
        // Lightroom's Vertical −70 on a photo the camera turned 90° left (LibRaw flip
        // 5, three quarter turns) is stored as PerspectiveHorizontal +70.
        let stored = Transform {
            horizontal: 0.7,
            ..Default::default()
        };
        let shown = stored.displayed(display_axes(3, false, false));
        assert!(
            (shown.vertical + 0.7).abs() < 1e-6 && shown.horizontal.abs() < 1e-6,
            "{shown:?}"
        );
    }

    #[test]
    fn displayed_sliders_are_the_same_homography_on_the_displayed_photo() {
        let stored = Transform {
            vertical: 0.3,
            horizontal: -0.2,
            rotate: 4.,
            aspect: 0.4,
            scale: 1.1,
            offset_x: 0.2,
            offset_y: -0.1,
        };
        let (w, h) = (1., 2. / 3.);
        for turns in 0..4 {
            for (flip_x, flip_y) in [(false, false), (true, false), (false, true)] {
                let m = display_axes(turns, flip_x, flip_y);
                let shown = stored.displayed(m);
                assert_eq!(shown.recorded(m), stored);
                let swapped = m[0][0] == 0.;
                let (dw, dh) = if swapped { (h, w) } else { (w, h) };
                // Displayed centred coordinates to recorded ones.
                let to = [[m[0][0], m[0][1], 0.], [m[1][0], m[1][1], 0.], [0., 0., 1.]];
                let expected = mat(crate::color::inverse(to), mat(stored.inverse(w, h), to));
                let got = shown.inverse(dw, dh);
                for i in 0..3 {
                    for j in 0..3 {
                        let (a, b) = (got[i][j] / got[2][2], expected[i][j] / expected[2][2]);
                        assert!(
                            (a - b).abs() < 1e-5,
                            "{turns} {flip_x} {flip_y}: {got:?} {expected:?}"
                        );
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod manual_distortion_tests {
    use super::*;
    fn photo() -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
            metadata: crate::camera_data::Metadata {
                width: 300,
                height: 200,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    fn distorted(amount: f32) -> Recipe {
        Recipe {
            lens_manual_distortion: amount,
            ..Default::default()
        }
    }
    /// Camera Raw 18.7 on the synthetic chart: corners stay, the centre is scaled by
    /// 1 + 0.4 × amount (positive) or 1 + 0.5 × amount (negative), and positive amounts
    /// bring white in at the edges' middles.
    #[test]
    fn manual_distortion_matches_camera_raws_radial_map() {
        let im = photo();
        let plain = Geometry::new(&im, &Recipe::default(), 0);
        for (amount, centre) in [(0.5, 1.2), (-0.5, 0.75), (1., 1.4), (-1., 0.5)] {
            let g = Geometry::new(&im, &distorted(amount), 0);
            for corner in [[0., 0.], [1., 0.], [0., 1.], [1., 1.]] {
                let (a, b) = (
                    g.source(corner[0], corner[1]),
                    plain.source(corner[0], corner[1]),
                );
                assert!(
                    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3,
                    "{a:?} {b:?}"
                );
            }
            // Near the centre the radius scales by the centre ratio.
            let [cx, _] = plain.source(0.5, 0.5);
            let [x, _] = g.source(0.51, 0.5);
            let [px, _] = plain.source(0.51, 0.5);
            let ratio = (x - cx) / (px - cx);
            assert!((ratio - centre).abs() < 2e-3, "{amount}: {ratio}");
            let [ex, ey] = g.source(0., 0.5);
            assert_eq!(g.outside(ex, ey), amount > 0., "{amount}: {ex}");
            // Positions well outside the frame (spots and masks may sit there) map back
            // too: past the corners the map continues one to one.
            for p in [[0.2, 0.3], [0.5, 0.5], [0.9, 0.1], [1.8, 1.5], [-0.9, 2.5]] {
                let [x, y] = g.source(p[0], p[1]);
                let back = g.view(x, y);
                assert!(
                    (back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3,
                    "{amount} {p:?}: {back:?}"
                );
            }
        }
        // Off at 0.
        assert!(Geometry::new(&im, &distorted(0.), 0).manual.is_none());
        let mut off = distorted(0.5);
        off.panels.set(
            crate::model::panels::Panel::LensCorrections,
            crate::model::panels::PanelState::Off,
        );
        assert!(Geometry::new(&im, &off, 0).manual.is_none());
    }
    /// It applies in the frame as recorded, after the Transform takes an output position
    /// back to that frame, as Camera Raw's renders with Scale, Offset and Vertical show.
    #[test]
    fn manual_distortion_applies_after_the_transform_towards_the_source() {
        let im = photo();
        let mut transformed = Recipe::default();
        transformed.transform.scale = 0.8;
        transformed.transform.offset_x = 0.2;
        transformed.transform.vertical = 0.3;
        let both = Recipe {
            lens_manual_distortion: 0.5,
            ..transformed.clone()
        };
        let (t, g) = (
            Geometry::new(&im, &transformed, 0),
            Geometry::new(&im, &both, 0),
        );
        let manual = ManualDistortion::new(0.5, 300., 200.).unwrap();
        for p in [[0.2, 0.3], [0.6, 0.5], [0.9, 0.8]] {
            let [x, y] = t.source(p[0], p[1]);
            let [mx, my] = manual.source((x + 0.5) / 300., (y + 0.5) / 200.);
            let got = g.source(p[0], p[1]);
            assert!(
                (got[0] - (mx * 300. - 0.5)).abs() < 1e-3
                    && (got[1] - (my * 200. - 0.5)).abs() < 1e-3,
                "{got:?}"
            );
        }
    }
}
#[cfg(test)]
mod constrain_crop_tests {
    use super::*;
    use crate::model::transform::{Upright, UprightMode};
    fn photo() -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
            metadata: crate::camera_data::Metadata {
                width: 300,
                height: 200,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    /// Output positions, on a grid over the rendered crop, that render white.
    fn white(g: &Geometry) -> usize {
        let n = 120;
        (0..=n)
            .flat_map(|i| (0..=n).map(move |j| (i, j)))
            .filter(|&(i, j)| {
                let [x, y] = g.source(i as f32 / n as f32, j as f32 / n as f32);
                g.outside(x, y)
            })
            .count()
    }
    fn cases() -> Vec<(&'static str, Recipe)> {
        let with = |edit: &dyn Fn(&mut Recipe)| {
            let mut r = Recipe::default();
            edit(&mut r);
            r
        };
        let (s, c) = 4f32.to_radians().sin_cos();
        vec![
            ("vertical +100", with(&|r| r.transform.vertical = 1.)),
            ("vertical -100", with(&|r| r.transform.vertical = -1.)),
            (
                "horizontal and vertical",
                with(&|r| {
                    r.transform.horizontal = 0.6;
                    r.transform.vertical = -0.4;
                }),
            ),
            ("rotate 10", with(&|r| r.transform.rotate = 10.)),
            ("rotate -10", with(&|r| r.transform.rotate = -10.)),
            ("scale 50", with(&|r| r.transform.scale = 0.5)),
            ("offset x 100", with(&|r| r.transform.offset_x = 1.)),
            (
                "aspect and offset y",
                with(&|r| {
                    r.transform.aspect = 1.;
                    r.transform.offset_y = -0.3;
                }),
            ),
            ("distortion +100", with(&|r| r.lens_manual_distortion = 1.)),
            (
                "distortion with transform",
                with(&|r| {
                    r.lens_manual_distortion = 0.5;
                    r.transform.vertical = 0.3;
                    r.transform.scale = 0.9;
                }),
            ),
            (
                "upright",
                with(&|r| {
                    r.upright = Upright {
                        mode: UprightMode::Level,
                        corrections: vec![
                            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                            [1., 0., 0., 0., 1., 0., 0., 0., 1.],
                            // A 4° turn about the centre and a slight keystone.
                            [
                                c,
                                -s,
                                0.5 - 0.5 * c + 0.5 * s,
                                s,
                                c,
                                0.5 - 0.5 * s - 0.5 * c,
                                0.05,
                                0.,
                                0.97,
                            ],
                        ],
                        ..Default::default()
                    }
                }),
            ),
            (
                "straighten with vertical",
                with(&|r| {
                    r.straighten = 20.;
                    r.transform.vertical = 0.5;
                }),
            ),
            (
                "turned and flipped",
                with(&|r| {
                    r.rotation = 1;
                    r.flip_x = true;
                    r.transform.horizontal = 0.5;
                    r.transform.rotate = 5.;
                }),
            ),
            (
                "user crop",
                with(&|r| {
                    r.crop = [0.05, 0.1, 0.55, 0.9];
                    r.transform.vertical = 0.5;
                }),
            ),
            (
                "user crop at the edge",
                with(&|r| {
                    r.crop = [0.7, 0., 1., 0.3];
                    r.straighten = -12.;
                    r.transform.rotate = -8.;
                    r.transform.scale = 0.8;
                }),
            ),
        ]
    }
    #[test]
    fn constrain_crop_leaves_no_white_and_keeps_the_crops_aspect() {
        let im = photo();
        for (name, r) in cases() {
            let free = Geometry::new(&im, &r, 0);
            assert!(white(&free) > 0, "{name}: nothing to constrain");
            let constrained = Recipe {
                constrain_crop: true,
                ..r.clone()
            };
            let g = Geometry::new(&im, &constrained, 0);
            assert_eq!(white(&g), 0, "{name}: {:?}", g.crop());
            let [l, t, r2, b] = g.crop();
            let c = r.crop;
            // Inside the user's crop, at its aspect, and a valid crop.
            assert!(
                l >= c[0] - 1e-6 && t >= c[1] - 1e-6 && r2 <= c[2] + 1e-6 && b <= c[3] + 1e-6,
                "{name}: {:?} outside {c:?}",
                g.crop()
            );
            let aspect = (r2 - l) / (b - t);
            let original = (c[2] - c[0]) / (c[3] - c[1]);
            assert!(
                (aspect / original - 1.).abs() < 1e-3,
                "{name}: {aspect} {original}"
            );
            assert!(r2 - l >= 0.01 && b - t >= 0.01, "{name}: {:?}", g.crop());
            assert!(constrained.validate().is_ok());
            // The output size follows the crop as rendered.
            let w = g.oriented_width * (r2 - l);
            assert_eq!(g.width, w.round() as u32, "{name}");
            // A recipe that stores the constrained crop renders it unchanged.
            let stored = Recipe {
                crop: g.crop(),
                ..constrained.clone()
            };
            assert_eq!(Geometry::new(&im, &stored, 0).crop(), g.crop(), "{name}");
        }
    }
    /// At Scale 50 only the middle half of the frame has a source pixel: the crop
    /// becomes that half, the largest one at the photo's aspect.
    #[test]
    fn the_rendered_crop_is_the_constrained_one_only_with_constrain_crop() {
        let mut r = Recipe::default();
        r.transform.vertical = 1.;
        let m = photo().metadata;
        assert_eq!(rendered_crop(&r, &m), r.crop);
        r.constrain_crop = true;
        let constrained = rendered_crop(&r, &m);
        assert_eq!(constrained, Geometry::for_metadata(&m, &r).crop());
        assert_ne!(constrained, r.crop);
    }
    #[test]
    fn constrain_crop_takes_the_largest_crop_that_fits() {
        let mut r = Recipe {
            constrain_crop: true,
            ..Default::default()
        };
        r.transform.scale = 0.5;
        let crop = Geometry::new(&photo(), &r, 0).crop();
        for (v, e) in crop.iter().zip([0.25, 0.25, 0.75, 0.75]) {
            assert!((v - e).abs() < 1e-3, "{crop:?}");
        }
        // Offset right, the crop moves with the photo instead of shrinking around the
        // centre.
        r.transform.scale = 0.8;
        r.transform.offset_x = 0.1;
        let [l, _, right, _] = Geometry::new(&photo(), &r, 0).crop();
        assert!((right - l - 0.8).abs() < 2e-3, "{l} {right}");
        assert!((l - (0.1 + 0.0811)).abs() < 2e-3, "{l}");
    }
    /// Where the covered area is far from the crop's centre, the search still finds the
    /// largest crop: a dense search over centres finds a side of 0.1038 here.
    #[test]
    fn constrain_crop_finds_the_largest_crop_away_from_the_centre() {
        let mut r = Recipe {
            constrain_crop: true,
            ..Default::default()
        };
        r.transform = Transform {
            horizontal: -0.1522,
            vertical: -0.5199,
            rotate: -2.83,
            aspect: 0.,
            scale: 0.5258,
            offset_x: 0.2627,
            offset_y: 0.8124,
        };
        let g = Geometry::new(&photo(), &r, 0);
        let [l, _, right, _] = g.crop();
        assert!(right - l > 0.1035, "{:?}", g.crop());
        assert_eq!(white(&g), 0);
    }
    /// A crop wholly in the white becomes the largest crop at its aspect that the photo
    /// covers, rather than staying white.
    #[test]
    fn constrain_crop_moves_a_crop_that_is_all_white() {
        let mut r = Recipe {
            constrain_crop: true,
            crop: [0., 0.4, 0.15, 0.6],
            ..Default::default()
        };
        r.transform.offset_x = 1.;
        let free = Geometry::new(
            &photo(),
            &Recipe {
                constrain_crop: false,
                ..r.clone()
            },
            0,
        );
        assert_eq!(white(&free), 121 * 121);
        let g = Geometry::new(&photo(), &r, 0);
        assert_eq!(white(&g), 0, "{:?}", g.crop());
        let [l, t, right, b] = g.crop();
        assert!(
            ((right - l) / (b - t) - 0.75).abs() < 1e-3,
            "{:?}",
            g.crop()
        );
        // The photo covers the right 0.189 of the frame.
        assert!((right - l - 0.189).abs() < 2e-3, "{:?}", g.crop());
    }
    /// Switching the Transform panel off bypasses its Constrain Crop too, so manual
    /// Distortion alone renders the stored crop.
    #[test]
    fn transform_panel_off_bypasses_constrain_crop() {
        use crate::model::panels::{Panel, PanelState};
        let mut r = Recipe {
            constrain_crop: true,
            lens_manual_distortion: 0.5,
            ..Default::default()
        };
        assert_ne!(Geometry::new(&photo(), &r, 0).crop(), r.crop);
        r.panels.set(Panel::Transform, PanelState::Off);
        let shown = r.as_rendered();
        assert_eq!(Geometry::new(&photo(), &shown, 0).crop(), r.crop);
    }
    #[test]
    fn constrain_crop_keeps_crops_with_nothing_white() {
        let im = photo();
        let mut r = Recipe {
            constrain_crop: true,
            crop: [0.3, 0.3, 0.6, 0.5],
            straighten: 30.,
            ..Default::default()
        };
        // Straighten alone zooms to leave no white.
        assert_eq!(Geometry::new(&im, &r, 0).crop(), r.crop);
        r.transform.vertical = 0.3;
        assert_eq!(Geometry::new(&im, &r, 0).crop(), r.crop);
        // Off, the crop is rendered as it is.
        r.transform.vertical = 1.;
        r.crop = [0., 0., 1., 1.];
        r.constrain_crop = false;
        assert_eq!(Geometry::new(&im, &r, 0).crop(), r.crop);
    }
}
