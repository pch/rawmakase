//! Image space, where retouching and masks keep their positions, and its mapping to
//! and from the rendered view.
//!
//! Image space is the photo as the camera oriented it, within the camera's default
//! crop, normalised to 0–1 on both axes, before lens correction, Transform, crop,
//! straightening and the user's rotation and flips. A spot stays on the dust particle
//! whatever those settings do.
use super::Geometry;
use crate::camera_data::CameraImage;
use crate::develop::recipe::RenderedRecipe;
use crate::model::image_frame::ImageFrame;
use crate::model::recipe::Recipe;

/// Lens distortion as a map of positions: from a corrected position (what
/// `Geometry::source` returns) to where it samples the decoded image, for green.
#[derive(Clone, Copy)]
pub(crate) struct LensMap<'a> {
    pub(crate) lens: &'a crate::optics::LensCorrection,
    /// Measured lateral chromatic aberration replacing the lens data's own.
    pub(crate) chromatic: Option<&'a [crate::optics::Radial; 2]>,
    pub(crate) center: [f32; 2],
    pub(crate) half: f32,
    pub(crate) fill: f32,
    /// Lightroom's profile Distortion amount (1 = 100%).
    pub(crate) amount: f32,
}
impl<'a> LensMap<'a> {
    pub(crate) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let chromatic = crate::lens::auto_ca::measured(im).filter(|_| r.lens_ca);
        let lens = match r.lens_correction(&im.metadata) {
            Some(lens) => lens,
            // Measured chromatic aberration and manual Vignetting are applied while
            // sampling, as lens corrections are, also without lens data.
            None => {
                if chromatic.is_none() && r.manual_vignette().is_none() {
                    return None;
                }
                &crate::optics::NO_CORRECTION
            }
        };
        let (w, h) = (im.width as f32, im.height as f32);
        Some(Self {
            lens,
            chromatic,
            center: [w * 0.5, h * 0.5],
            half: (w * w + h * h).sqrt() * 0.5,
            fill: lens.fill_scale_with(r.lens_distortion),
            amount: r.lens_distortion,
        })
    }
    /// Offset from the centre, scaled by the fill, and the per-channel radial scale.
    pub(crate) fn scales(&self, x: f32, y: f32) -> ([f32; 2], [f32; 3]) {
        let dx = (x + 0.5 - self.center[0]) * self.fill;
        let dy = (y + 0.5 - self.center[1]) * self.fill;
        let r = (dx * dx + dy * dy).sqrt() / self.half;
        let scale = self.lens.radial_scale_ca(r, self.amount, self.chromatic);
        ([dx, dy], scale)
    }
    pub(crate) fn forward(&self, x: f32, y: f32) -> [f32; 2] {
        let ([dx, dy], scale) = self.scales(x, y);
        [
            self.center[0] + dx * scale[1] - 0.5,
            self.center[1] + dy * scale[1] - 0.5,
        ]
    }
    /// The corrected position that samples decoded position (`x`, `y`): the radius is
    /// found with a few Newton steps, the direction is kept.
    pub(crate) fn inverse(&self, x: f32, y: f32) -> [f32; 2] {
        let qx = x + 0.5 - self.center[0];
        let qy = y + 0.5 - self.center[1];
        let target = (qx * qx + qy * qy).sqrt();
        if target < 1e-6 {
            return [x, y];
        }
        let f = |rho: f32| rho * self.lens.radial_scale_with(rho / self.half, self.amount)[1];
        let mut rho = target;
        for _ in 0..8 {
            let h = self.half * 1e-3;
            let slope = (f(rho + h) - f(rho - h)) / (2. * h);
            if slope.abs() < 1e-6 {
                break;
            }
            let step = (f(rho) - target) / slope;
            rho = (rho - step).max(0.);
            if step.abs() < 1e-4 {
                break;
            }
        }
        let k = rho / target / self.fill;
        [self.center[0] + qx * k - 0.5, self.center[1] + qy * k - 0.5]
    }
}

/// The mapping between rendered output coordinates (0–1 over the cropped view) and
/// image space, for one image and recipe.
pub struct ViewMapping<'a> {
    pub geometry: Geometry,
    frame: ImageFrame,
    lens: Option<LensMap<'a>>,
}
impl<'a> ViewMapping<'a> {
    pub fn new(im: &'a CameraImage, r: &Recipe) -> Self {
        Self {
            geometry: Geometry::new(im, r, 0),
            frame: ImageFrame::new(im),
            lens: LensMap::new(im, r),
        }
    }
    pub fn frame(&self) -> &ImageFrame {
        &self.frame
    }
    /// Image-space position shown at view position (`u`, `v`).
    pub fn to_image(&self, u: f32, v: f32) -> [f32; 2] {
        let [x, y] = self.geometry.source(u, v);
        let [x, y] = self.lens.map_or([x, y], |l| l.forward(x, y));
        self.frame.to_image(x, y)
    }
    /// View position where image-space position `p` is shown.
    pub fn to_view(&self, p: [f32; 2]) -> [f32; 2] {
        let [x, y] = self.frame.to_source(p);
        let [x, y] = self.lens.map_or([x, y], |l| l.inverse(x, y));
        self.geometry.view(x, y)
    }
    /// View-space length of an image-space long-edge fraction `r` near `p`, as
    /// fractions of the view width and height.
    pub fn view_radius(&self, p: [f32; 2], r: f32) -> [f32; 2] {
        let (rx, ry) = crate::model::retouch::radii(r, self.frame.aspect());
        let c = self.to_view(p);
        let a = self.to_view([p[0] + rx, p[1]]);
        let b = self.to_view([p[0], p[1] + ry]);
        let d = |q: [f32; 2]| [q[0] - c[0], q[1] - c[1]];
        let (a, b) = (d(a), d(b));
        // The longer of the two mapped axes on each view axis, so rotation keeps size.
        [a[0].hypot(b[0]), a[1].hypot(b[1])]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(flip: i32) -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
            metadata: crate::camera_data::Metadata {
                width: 300,
                height: 200,
                crop_left: 10,
                crop_top: 6,
                crop_width: 280,
                crop_height: 190,
                flip,
                wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    #[test]
    fn view_mapping_round_trips_with_crop_rotation_and_transform() {
        let im = image(6);
        let mut r = Recipe {
            crop: [0.1, 0.05, 0.8, 0.9],
            straighten: 7.,
            rotation: 1,
            flip_x: true,
            ..Default::default()
        };
        r.transform.vertical = 0.3;
        r.transform.rotate = 2.;
        r.upright.mode = crate::model::transform::UprightMode::Vertical;
        r.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5];
        r.upright.corrections[4] = [1.1, 0.02, -0.05, 0.03, 1.05, -0.02, 0.2, 0.01, 1.];
        let map = ViewMapping::new(&im, &r);
        for p in [[0.3, 0.4], [0.5, 0.5], [0.7, 0.2]] {
            let view = map.to_view(p);
            let back = map.to_image(view[0], view[1]);
            assert!(
                (p[0] - back[0]).abs() < 1e-4 && (p[1] - back[1]).abs() < 1e-4,
                "{p:?} {view:?} {back:?}"
            );
        }
    }
    #[test]
    fn upright_applies_in_the_recorded_frame() {
        // A photo the camera turned to portrait: Lightroom's Upright correction is in
        // the frame the camera recorded, so a shift along its x samples 10% further
        // along the decoded image's width.
        let im = image(6);
        let plain = Recipe::default();
        let mut r = plain.clone();
        r.upright.mode = crate::model::transform::UprightMode::Auto;
        r.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 2];
        r.upright.corrections[1] = [1., 0., 0.1, 0., 1., 0., 0., 0., 1.];
        let (a, b) = (Geometry::new(&im, &plain, 0), Geometry::new(&im, &r, 0));
        for [u, v] in [[0.3, 0.4], [0.8, 0.1]] {
            let [ax, ay] = a.source(u, v);
            let [bx, by] = b.source(u, v);
            assert!(
                (ax - bx - 28.).abs() < 1e-3 && (ay - by).abs() < 1e-3,
                "{u} {v}"
            );
        }
        // The camera's default crop starts 10 pixels in: the margin shows white.
        assert!(b.outside(5., 100.) && !b.outside(15., 100.));
    }
    #[test]
    fn lens_inverse_undoes_forward() {
        let lens = crate::optics::LensCorrection {
            distortion: Some(crate::optics::Radial {
                knots: vec![0., 0.5, 1.],
                values: vec![1., 1.02, 1.08],
            }),
            ..Default::default()
        };
        let map = LensMap {
            lens: &lens,
            chromatic: None,
            center: [150., 100.],
            half: 180.,
            fill: 0.95,
            amount: 1.,
        };
        for (x, y) in [(10., 20.), (150., 100.), (290., 190.), (40., 170.)] {
            let [fx, fy] = map.forward(x, y);
            let [bx, by] = map.inverse(fx, fy);
            assert!((bx - x).abs() < 1e-2 && (by - y).abs() < 1e-2, "{x} {y}");
        }
    }
}
