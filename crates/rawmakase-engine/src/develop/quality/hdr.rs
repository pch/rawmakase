//! The HDR master of an edit with HDR on (docs/records/hdr-implementation-plan.md,
//! stage 3).
use super::*;
use rawmakase_cancel::Cancel;

/// The HDR master of an edit with HDR on, on the CPU: the whole photo within
/// `max_edge`. Every other render of such an edit is its SDR rendition
/// (`Recipe::resolved`); the GPU never renders an HDR master.
///
/// Sharpening and resizing work on the master's sRGB-curve-encoded values, the
/// encoding SDR renders sharpen and resize in, extended above white and unclipped;
/// the detail is that of ProPhoto luminance. The HDR limit applies after both.
pub fn render_hdr(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    master(im, r, ai, Part::Whole(max_edge), cancel)
}

/// The part `[x, y, w, h]` of the HDR master at 100%, in the pixels of the photo as
/// its geometry lays it out (as a 100% region of the SDR rendition): what
/// [`render_hdr`] renders there at full size, sharpening included.
pub fn render_hdr_region(
    im: &CameraImage,
    r: &ValidRecipe,
    region: [u32; 4],
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    master(im, r, None, Part::Region(region), cancel)
}

/// Which part of the master to render: the whole photo within a long edge (0 for
/// full size), or a region at 100%.
#[derive(Clone, Copy)]
enum Part {
    Whole(u32),
    Region([u32; 4]),
}
impl Part {
    /// The region `self` renders of a photo `g` lays out, checked to lie within it.
    fn region(self, g: &Geometry) -> Result<[u32; 4]> {
        let [x, y, w, h] = match self {
            Part::Whole(_) => [0, 0, g.width, g.height],
            Part::Region(region) => region,
        };
        ensure!(
            w > 0
                && h > 0
                && x.checked_add(w).is_some_and(|v| v <= g.width)
                && y.checked_add(h).is_some_and(|v| v <= g.height),
            "Invalid viewport region"
        );
        Ok([x, y, w, h])
    }
    fn max_edge(self) -> u32 {
        match self {
            Part::Whole(edge) => edge,
            Part::Region(_) => 0,
        }
    }
}

fn master(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    part: Part,
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    use crate::color::{srgb_decode, srgb_encode};
    ensure!(r.hdr.is_active(), "This edit has HDR off");
    crate::model::hdr::check_for(r, &im.metadata)?;
    let max_edge = part.max_edge();
    let mut recipe = r.resolved_hdr(&im.metadata);
    // An HDR file renders through the same stages as a raw, from the pseudo-scene of
    // its light, through its profile with a linear curve; an edit without a profile
    // has the Embedded profile, as a rendered file has no camera matrix to default
    // to. Its Exposure is a gain of that light, applied before, as Camera Raw
    // renders it.
    let scene;
    let (im, ai) = if im.metadata.hdr_source.is_some() {
        scene = hdr_file_scene(im, recipe.exposure.exp2())?;
        let recipe = recipe.to_mut();
        recipe.exposure = 0.;
        let profile = match recipe.profile.as_deref() {
            Some(p) => p.for_hdr_light(),
            None => crate::camera_profiles::CameraProfile::embedded(&im.metadata).for_hdr_light(),
        };
        recipe.profile = Some(std::sync::Arc::new(profile));
        (&scene, None)
    } else {
        (im, ai)
    };
    ensure!(
        recipe
            .profile
            .as_ref()
            .is_some_and(|p| p.hdr_tone().is_ok()),
        "This photo's camera profile does not render in HDR"
    );
    let geometry = Geometry::new(im, &recipe, 0);
    let [x, y, w, h] = part.region(&geometry)?;
    let resolved = Resolved { recipe, geometry };
    let source = resolved.source(im, ai, cancel, None)?;
    let r = resolved.recipe.as_ref();
    let g = resolved.geometry;
    // A region renders with the margin sharpening reads around it, as SDR regions do,
    // and is cropped to it after. Radii are the photo's: a reduced decode sizes them by
    // its photo scale, as SDR renders do.
    let scale = im.photo_scale();
    let sigma = Sharpener::new(r).sigma * scale;
    let halo = if r.sharpening > 0. {
        (3. * sigma).ceil() as u32
    } else {
        0
    };
    let left = x.saturating_sub(halo);
    let top = y.saturating_sub(halo);
    let base = [
        left,
        top,
        (x + w + halo).min(g.width) - left,
        (y + h + halo).min(g.height) - top,
    ];
    let (toned, tonal_recipe) = local_stage(&source.image, &source, r, scale, cancel, None)?;
    let srgb = crate::color::OutputSpace::Srgb;
    let (mut out, _) =
        develop::render_base(&toned, &tonal_recipe, &g, base, 0., cancel, None, srgb)?;
    check_cancel(cancel)?;
    let peak = r.hdr.peak();
    let encode = |p: &mut [f32; 3]| *p = p.map(|v| srgb_encode(v.max(0.)));
    let resized =
        if r.sharpening > 0. || output_size(g.width, g.height, max_edge) != (g.width, g.height) {
            out.pixels.par_iter_mut().for_each(encode);
            sharpen_unbounded(
                &mut out,
                r,
                sigma,
                crate::color::OutputSpace::ProPhoto,
                cancel,
            )?;
            let out = super::render::crop(out, [x - left, y - top, w, h]);
            let mut out = resize_unbounded(out, max_edge);
            out.pixels
                .par_iter_mut()
                .for_each(|p| *p = p.map(|v| srgb_decode(v.max(0.))));
            out
        } else {
            super::render::crop(out, [x - left, y - top, w, h])
        };
    check_cancel(cancel)?;
    Ok(crate::rendered::HdrImage {
        width: resized.width,
        height: resized.height,
        pixels: resized
            .pixels
            .into_iter()
            .map(|p| p.map(|v| crate::model::hdr::roll_off(v, peak)))
            .collect(),
        peak,
    })
}

/// An HDR file as the scene its HDR master renders (docs/records/hdr-stage4-design.md):
/// its light through [`crate::develop::rendered::scene_from_light`], which the master
/// at the file's own settings (`Recipe::for_rendered`, the Embedded profile with a
/// linear curve) renders as the light again. `im` is the decoded photo or its preview reduction, which hold
/// the light; a working image doesn't, and a fact in its metadata alone renders
/// nothing. The scene carries no light of its own, so no stage can reach it. `gain`
/// scales the light first.
fn hdr_file_scene(im: &CameraImage, gain: f32) -> Result<CameraImage> {
    use anyhow::Context;
    let light = im.hdr().context("This HDR file's light was not decoded")?;
    Ok(CameraImage {
        width: light.width(),
        height: light.height(),
        recovered: Default::default(),
        pixels: light
            .pixels()
            .par_iter()
            .map(|p| crate::develop::rendered::scene_from_light(p.map(|v| v * gain)))
            .collect(),
        metadata: im.metadata.clone(),
        fast: im.fast,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Metadata;
    use crate::camera_profiles::CameraProfile;
    use crate::model::recipe::Recipe;

    /// A `w` × `h` HDR file whose light is `light(x, y)`, paired with its SDR
    /// rendition as the decoder pairs them.
    fn hdr_file(w: u32, h: u32, light: impl Fn(u32, u32) -> [f32; 3]) -> CameraImage {
        use crate::camera_data::{HdrLight, Source};
        let pixels: Vec<[f32; 3]> = (0..w * h).map(|i| light(i % w, i / w)).collect();
        let light = HdrLight::new(
            w,
            h,
            pixels.clone(),
            crate::color::hdr_output::HdrTransfer::Pq,
        )
        .unwrap();
        let metadata = Metadata {
            width: w,
            height: h,
            raw_width: w,
            raw_height: h,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            source: Source::Rendered,
            ..Default::default()
        };
        CameraImage {
            width: w,
            height: h,
            recovered: Default::default(),
            pixels: pixels.iter().map(|p| p.map(|v| v.clamp(0., 1.))).collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
        .with_hdr(std::sync::Arc::new(light))
        .unwrap()
    }

    /// An HDR file's master at its own settings is its light, above white and coloured
    /// alike, within the pseudo-scene's precision (0.0001 stop); a negative channel
    /// renders as 0, as an HDR master never renders one. Exposure is a gain; Rotate
    /// turns it as the SDR rendition turns.
    #[test]
    fn an_hdr_files_master_is_its_light() -> Result<()> {
        let light = |x: u32, y: u32| match (x, y) {
            (0, 0) => [6., 6., 6.],
            (1, 0) => [-0.2, 0.4, 0.9],
            _ => [0.1 * x as f32, 0.05 * y as f32, 0.3],
        };
        let im = hdr_file(6, 4, light);
        let r = Recipe::for_new_rendered(&im.metadata);
        assert!(r.hdr.enabled, "an HDR file starts as an HDR edit");
        assert_eq!(r.hdr.limit, 3., "6 is past limit 2's knee");
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        assert_eq!((out.width, out.height), (6, 4));
        for (i, p) in out.pixels.iter().enumerate() {
            let want = light(i as u32 % 6, i as u32 / 6).map(|v| v.max(0.));
            assert!(
                (0..3).all(|c| (p[c] - want[c]).abs() <= 2e-4 * want[c].max(0.05)),
                "{i}: {p:?} against {want:?}"
            );
        }
        let mut brighter = r.clone();
        brighter.exposure = 1.;
        brighter.hdr.limit = 4.;
        let out = render_hdr(&im, &brighter.checked()?, None, 0, Cancel::NEVER)?;
        assert!(
            (out.pixels[0][0] / 12. - 1.).abs() < 2e-4,
            "{:?}",
            out.pixels[0]
        );
        let mut turned = r;
        turned.rotation = 1;
        let out = render_hdr(&im, &turned.checked()?, None, 0, Cancel::NEVER)?;
        let sdr = crate::develop::render(&im, &turned.checked()?, 0, Cancel::NEVER)?;
        assert_eq!((out.width, out.height), (sdr.width, sdr.height));
        assert_eq!((out.width, out.height), (4, 6));
        // The bright corner goes where the SDR rendition's goes.
        let brightest = |pixels: &[[f32; 3]]| {
            (0..pixels.len())
                .max_by(|a, b| pixels[*a][1].total_cmp(&pixels[*b][1]))
                .unwrap()
        };
        assert_eq!(brightest(&out.pixels), brightest(&sdr.pixels));
        Ok(())
    }

    /// Saturated light renders as itself too, within the precision of the colour
    /// matrices the master's stages convert through, as a raw's do (f32, and published
    /// pairs inverse to about 1e-5): 1e-5 of the pixel's brightest channel. An edit
    /// without a profile renders as one with the Embedded profile.
    #[test]
    fn saturated_light_and_an_edit_without_a_profile_render_as_the_light() -> Result<()> {
        let colours = [
            [40., 1e-4, 1e-4],
            [0., 0., 40.],
            [3., 0.02, 0.4],
            [0.001, 0.3, 0.0002],
            [12., 11., 0.],
            [0.2, 0.2, 0.2],
        ];
        let im = hdr_file(6, 1, |x, _| colours[x as usize]);
        let r = Recipe::for_new_rendered(&im.metadata);
        let mut none = r.clone();
        none.profile = None;
        for r in [r, none] {
            let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
            for (p, want) in out.pixels.iter().zip(colours) {
                let top = want.iter().copied().fold(0f32, f32::max);
                assert!(
                    (0..3).all(|c| (p[c] - want[c]).abs() <= 2e-4 * want[c] + 1e-5 * top),
                    "{p:?} against {want:?}"
                );
            }
        }
        Ok(())
    }

    /// The fact that a photo is an HDR file renders nothing without its light: a
    /// working copy of the image has none.
    #[test]
    // The clone is what is tested: a working copy has no light.
    #[allow(clippy::redundant_clone)]
    fn an_hdr_files_working_copy_renders_no_master() -> Result<()> {
        let im = hdr_file(2, 2, |_, _| [2.; 3]);
        let r = Recipe::for_new_rendered(&im.metadata);
        let copy = im.clone();
        assert!(copy.metadata.hdr_source.is_some() && copy.hdr().is_none());
        assert!(render_hdr(&copy, &r.checked()?, None, 0, Cancel::NEVER).is_err());
        Ok(())
    }

    /// Grays at the scene values `levels` (white-balanced camera values, which the
    /// camera matrix profile renders neutral), one per column of a strip 8 pixels
    /// high.
    fn grays(levels: &[f32]) -> CameraImage {
        patches(&levels.iter().map(|v| [*v; 3]).collect::<Vec<_>>())
    }
    /// The scene colours `colors` (white-balanced camera values), one per 8-pixel
    /// column of a strip 8 pixels high.
    fn patches(colors: &[[f32; 3]]) -> CameraImage {
        let (w, h) = (colors.len() as u32 * 8, 8);
        let metadata = Metadata {
            width: w,
            height: h,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        CameraImage {
            width: w,
            height: h,
            recovered: Default::default(),
            pixels: (0..w * h).map(|i| colors[(i % w / 8) as usize]).collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }
    fn hdr_recipe(im: &CameraImage) -> Recipe {
        let mut r = Recipe {
            profile: CameraProfile::camera_matrix_default(&im.metadata).map(std::sync::Arc::new),
            sharpening: 0.,
            ..Default::default()
        };
        r.hdr.enabled = true;
        r.hdr.limit = 8.;
        r
    }
    fn centre(out: &crate::rendered::HdrImage, column: usize) -> f32 {
        let p = out.pixels[4 * out.width as usize + column * 8 + 4];
        crate::develop::scene_tone::luminance(p)
    }

    /// The default HDR render of a gray ramp through the ACR3 curve is Camera Raw
    /// 18.7's (measured at HDR limit 8; scene values after the S3 toe), and the SDR
    /// renders of the same edit stay SDR.
    #[test]
    fn an_hdr_master_extends_above_white_as_camera_raw_does() -> anyhow::Result<()> {
        let levels = [0.1887, 0.4789, 1.2006, 4.064];
        let measured = [0.408, 1.004, 3.571, 21.277];
        let im = grays(&levels);
        let r = hdr_recipe(&im);
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        for (i, expected) in measured.iter().enumerate() {
            let got = centre(&out, i);
            assert!(
                (got / expected).log2().abs() < 0.05,
                "{i}: {got} {expected}"
            );
        }
        let sdr = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        assert!(sdr.pixels.iter().flatten().all(|v| (0. ..=1.).contains(v)));
        let mut off = r;
        off.hdr.enabled = false;
        let plain = crate::develop::render(&im, &off.checked()?, 0, Cancel::NEVER)?;
        assert_eq!(
            sdr.pixels, plain.pixels,
            "the SDR rendition is the edit with HDR off"
        );
        Ok(())
    }

    /// The limit bounds every channel, Exposure scales the master, and an SDR rendition
    /// control never changes it.
    #[test]
    fn the_limit_bounds_the_master_and_exposure_scales_it() -> anyhow::Result<()> {
        let im = grays(&[0.5, 2., 8., 64.]);
        let mut r = hdr_recipe(&im);
        r.hdr.limit = 2.;
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        assert_eq!(out.peak, 4.);
        assert!(out.pixels.iter().flatten().all(|v| *v <= 4. && *v >= 0.));
        assert!(centre(&out, 3) > 3.8);
        r.hdr.limit = 8.;
        let base = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.exposure = 1.;
        let brighter = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        assert!(centre(&brighter, 1) > centre(&base, 1) * 1.5);
        r.exposure = 0.;
        Ok(())
    }

    /// An adjustment or look without a measured HDR rendering refuses the master, not
    /// the edit: its SDR rendition renders it.
    #[test]
    fn unmeasured_adjustments_and_looks_refuse_the_hdr_master() -> anyhow::Result<()> {
        let im = grays(&[0.5]);
        let master = |r: &Recipe| render_hdr(&im, &r.checked().unwrap(), None, 0, Cancel::NEVER);
        let mut r = hdr_recipe(&im);
        r.effects.clarity = 0.2;
        let error = master(&r).unwrap_err().to_string();
        assert!(error.contains("not supported in HDR"), "{error}");
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        r.effects.clarity = 0.;
        r.hdr.sdr.brightness = 0.2;
        assert!(master(&r).is_err());
        r.hdr.sdr.brightness = 0.;
        // A look renders its master at 100%, and with an RGB table not at all.
        r.profile = Some(std::sync::Arc::new(CameraProfile::creative_for_test(
            &im.metadata,
        )));
        master(&r)?;
        r.profile_amount = 0.5;
        assert!(master(&r).is_err());
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        r.profile_amount = 1.;
        let rgb = CameraProfile::creative_for_test(&im.metadata).with_test_rgb_tables();
        r.profile = Some(std::sync::Arc::new(rgb[0].clone()));
        assert!(master(&r).is_err());
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        Ok(())
    }

    /// A look's table and curve render a gray ramp through the ACR3 curve as Camera
    /// Raw 18.7 renders them at HDR limit 8: an S curve lifts values above white, and
    /// a table that scales value by 1 − 0.3 v over eight divisions looks it up on
    /// overrange-encoded values.
    #[test]
    fn a_looks_table_and_curve_render_as_camera_raw_renders_them() -> anyhow::Result<()> {
        let levels = [0.183, 0.5055, 0.9785, 1.9928];
        let im = grays(&levels);
        let s = [
            [0., 0.],
            [64., 44.],
            [128., 132.],
            [192., 214.],
            [255., 255.],
        ]
        .map(|p: [f32; 2]| p.map(|v| v / 255.));
        let identity = [[0., 0.], [1., 1.]];
        let base = hdr_recipe(&im).profile.unwrap().as_ref().clone();
        for (look, measured) in [
            (
                base.clone().with_test_look(1, |_| 1., &s),
                [0.4946, 1.3018, 3.2835, 10.3142],
            ),
            (
                base.with_test_look(8, |v| 1. - 0.3 * v, &identity),
                [0.3665, 0.9066, 1.6358, 3.3036],
            ),
        ] {
            let mut r = hdr_recipe(&im);
            r.profile = Some(std::sync::Arc::new(look));
            let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
            for (i, expected) in measured.iter().enumerate() {
                let got = centre(&out, i);
                assert!(
                    (got / expected).log2().abs() < 0.07,
                    "{i}: {got} {expected}"
                );
            }
        }
        Ok(())
    }

    /// Contrast renders the master: positive Contrast lifts the light above white and
    /// negative Contrast pulls it toward the pivot, up to four stops above white (the
    /// ACR3 curve renders scene 0.5 and 2 at about 1 and 8).
    #[test]
    fn contrast_renders_the_hdr_master() -> anyhow::Result<()> {
        let im = grays(&[0.05, 0.5, 2.]);
        let mut r = hdr_recipe(&im);
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.contrast = 0.5;
        let more = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.contrast = -0.5;
        let less = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        for i in 1..3 {
            assert!(centre(&more, i) > centre(&plain, i) * 1.05, "{i}");
            assert!(centre(&less, i) < centre(&plain, i) * 0.95, "{i}");
        }
        assert!(centre(&less, 0) > centre(&plain, 0));
        Ok(())
    }

    /// The point and parametric curves render a gray ramp through the ACR3 curve as
    /// Camera Raw 18.7 renders them at HDR limit 8: the point curve on encoded values,
    /// continued along its end slope above white, and the parametric curve over an
    /// axis that ends four stops above white.
    #[test]
    fn tone_curves_render_as_camera_raw_renders_them() -> anyhow::Result<()> {
        let levels = [0.183, 0.5055, 0.9785, 1.9928];
        let im = grays(&levels);
        let curve = |points: &[[f32; 2]]| crate::color::curve::ToneCurve {
            points: points.iter().map(|p| p.map(|v| v / 255.)).collect(),
            ..Default::default()
        };
        type Edit = Box<dyn Fn(&mut Recipe)>;
        let cases: [(Edit, [f32; 4]); 6] = [
            (Box::new(|_| {}), [0.3874, 1.0562, 2.5542, 7.8706]),
            (
                Box::new(move |r| {
                    r.curve = curve(&[
                        [0., 0.],
                        [64., 44.],
                        [128., 132.],
                        [192., 214.],
                        [255., 255.],
                    ])
                }),
                [0.4944, 1.0277, 1.5596, 2.6163],
            ),
            (
                Box::new(move |r| r.curve = curve(&[[0., 0.], [128., 100.], [255., 255.]])),
                [0.2739, 1.0757, 3.6125, 16.8747],
            ),
            (
                Box::new(|r| r.effects.parametric[3] = 0.5),
                [0.3874, 1.0601, 3.2286, 11.704],
            ),
            (
                Box::new(|r| r.effects.parametric[1] = 0.5),
                [0.6632, 1.5458, 3.2201, 8.1988],
            ),
            (
                Box::new(|r| r.effects.parametric[2] = -0.5),
                [0.3021, 0.7113, 1.4565, 5.1187],
            ),
        ];
        for (n, (edit, measured)) in cases.into_iter().enumerate() {
            let mut r = hdr_recipe(&im);
            edit(&mut r);
            let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
            for (i, expected) in measured.iter().enumerate() {
                let got = centre(&out, i);
                assert!(
                    (got / expected).log2().abs() < 0.07,
                    "case {n}, {i}: {got} {expected}"
                );
            }
        }
        Ok(())
    }

    /// A curve that ends steeply extends far past every limit; the master stays finite
    /// through sharpening and resizing and rolls off to the limit, not to black.
    #[test]
    fn a_steep_curve_rolls_off_to_the_limit_through_sharpening_and_resizing() -> anyhow::Result<()>
    {
        // All above white after the ACR3 curve, so on the curve's extension.
        let im = grays(&[0.5, 2., 8.]);
        let mut r = hdr_recipe(&im);
        r.curve.points = vec![[0.99, 0.], [1., 1.]];
        r.sharpening = 0.5;
        for max_edge in [0, 12] {
            let out = render_hdr(&im, &r.checked()?, None, max_edge, Cancel::NEVER)?;
            // Every gray renders past the limit's knee: none is lost to NaN.
            let darkest = out.pixels.iter().flatten().fold(f32::MAX, |a, v| a.min(*v));
            assert!(darkest > out.peak * 0.9, "{max_edge}: {darkest}");
        }
        Ok(())
    }

    /// Strong but valid Levels extend far past every limit; with sharpening the master
    /// stays finite and rolls off to the limit, not to black.
    #[test]
    fn strong_levels_roll_off_to_the_limit_through_sharpening() -> anyhow::Result<()> {
        let im = grays(&[1., 4., 16.]);
        let mut r = hdr_recipe(&im);
        (r.black_point, r.white_point, r.midtone) = (0.8, 1., 0.1);
        r.sharpening = 0.5;
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let darkest = out.pixels.iter().flatten().fold(f32::MAX, |a, v| a.min(*v));
        assert!(darkest > out.peak * 0.9, "{darkest}");
        Ok(())
    }

    /// Levels renders in the master as a point curve of its shape: on encoded values,
    /// as in SDR up to white and along its slope at white above it.
    #[test]
    fn levels_render_in_the_master_as_a_curve_of_their_shape() -> anyhow::Result<()> {
        use crate::color::hdr_curve::{decode, encode, levels};
        let im = grays(&[0.1, 0.5, 1.2, 4.]);
        let r = hdr_recipe(&im);
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        for (black, white, midtone) in [(0.05, 1., 1.3), (0., 0.9, 0.8)] {
            let mut edit = r.clone();
            (edit.black_point, edit.white_point, edit.midtone) = (black, white, midtone);
            let out = render_hdr(&im, &edit.checked()?, None, 0, Cancel::NEVER)?;
            for i in 0..4 {
                let want = decode(levels(encode(centre(&plain, i)), black, white, midtone));
                let got = centre(&out, i);
                assert!((got / want - 1.).abs() < 1e-3, "{i}: {got} {want}");
            }
        }
        Ok(())
    }

    /// The colour stage renders above white as Camera Raw 18.7 renders it in HDR
    /// (docs/hdr.md#the-colour-stage): Saturation changes a colour's saturation as much
    /// at 20 times white as at a quarter of it, Vibrance's brightening leaves colours
    /// above white alone, a black & white master is gray and unclipped, and
    /// Calibration renders. Band Luminance refuses the master.
    #[test]
    fn the_colour_stage_renders_above_white_as_camera_raw_does() -> anyhow::Result<()> {
        // An orange that the ACR3 curve renders at about a quarter of white, and with
        // Exposure +5 at about 20 times white.
        let im = patches(&[[0.1, 0.08, 0.07]]);
        // The patch's colour, HSV saturation and luminance (linear ProPhoto RGB).
        let colour = |out: &crate::rendered::HdrImage| {
            let p = out.pixels[4 * out.width as usize + 4];
            let max = p.into_iter().fold(0f32, f32::max);
            let min = p.into_iter().fold(f32::INFINITY, f32::min);
            (
                p,
                (max - min) / max,
                crate::develop::scene_tone::luminance(p),
            )
        };
        let at = |exposure: f32, edit: fn(&mut Recipe)| {
            let mut r = hdr_recipe(&im);
            r.exposure = exposure;
            edit(&mut r);
            render_hdr(&im, &r.checked().unwrap(), None, 0, Cancel::NEVER).map(|o| colour(&o))
        };
        let both =
            |edit: fn(&mut Recipe)| -> anyhow::Result<_> { Ok([at(0., edit)?, at(5., edit)?]) };
        let [(_, s_low, y_low), (base_high, s_high, y_high)] = both(|_| {})?;
        assert!(y_low < 0.5 && y_high > 8., "{y_low} {y_high}");

        // With Contrast, Saturation follows it, as in SDR's colour stage.
        let [_, (p, _, _)] = both(|r| {
            r.contrast = 0.3;
            r.saturation = 0.3;
        })?;
        let [_, (q, _, _)] = both(|r| r.contrast = 0.3)?;
        assert!(p != q && p.iter().all(|v| v.is_finite()), "{p:?} {q:?}");
        // And the point curve, too.
        let [_, (p, _, _)] = both(|r| {
            r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
            r.saturation = 0.3;
        })?;
        let [_, (q, _, _)] = both(|r| r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]])?;
        assert!(p != q && p.iter().all(|v| v.is_finite()), "{p:?} {q:?}");

        let [(_, low, _), (_, high, _)] = both(|r| r.saturation = 1.)?;
        let [low, high] = [low / s_low, high / s_high];
        assert!(
            low > 1.2 && (high / low - 1.).abs() < 0.03,
            "{low} {high} ({s_low} {s_high})"
        );

        // Vibrance's change to HSV value, the brightest channel.
        let value = |p: [f32; 3]| p.into_iter().fold(0f32, f32::max);
        let [(base_low, _, _), _] = both(|_| {})?;
        let [(low, _, _), (high, _, _)] = both(|r| r.vibrance = 1.)?;
        let [low, high] = [value(low) / value(base_low), value(high) / value(base_high)];
        assert!(low > 1.02 && (high - 1.).abs() < 0.01, "{low} {high}");
        assert!(
            at(0., |r| r.hsl[1][2] = 0.5).is_err(),
            "band Luminance is not rendered in HDR"
        );

        let [_, (p, _, y)] = both(|r| r.effects.monochrome = true)?;
        assert!(p.iter().all(|v| (v - p[0]).abs() < 1e-3 * p[0]), "{p:?}");
        assert!(y > 8., "{y}");

        let [_, (p, _, _)] = both(|r| r.effects.calibration[0] = [0.6, -0.4])?;
        assert!(
            (0..3).any(|c| (p[c] / base_high[c] - 1.).abs() > 0.1),
            "{p:?} {base_high:?}"
        );
        Ok(())
    }

    /// The limit's roll-off applies once: a channel at the peak renders where
    /// `roll_off` puts it, not rolled off twice.
    #[test]
    fn the_limit_rolls_off_once() -> anyhow::Result<()> {
        let im = grays(&[40.]);
        let mut r = hdr_recipe(&im);
        r.hdr.limit = 4.;
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let p = out.pixels[4 * out.width as usize + 4];
        let before = crate::camera_profiles::hdr_tone::HdrTone::ordinary(
            &crate::camera_profiles::default_tone(),
        )?
        .evaluate(&crate::camera_profiles::default_tone(), 40.);
        let once = crate::model::hdr::roll_off(before, 16.);
        assert!((p[1] - once).abs() < 0.01, "{} {once}", p[1]);
        Ok(())
    }

    /// The GPU never renders an HDR edit, nor an HDR profile's tables.
    #[test]
    fn the_gpu_port_leaves_hdr_to_the_cpu() {
        use crate::develop::pipeline::pixel_params::supported;
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        assert!(!supported(&r));
        r.hdr.enabled = false;
        assert!(supported(&r));
        let hdr = r.profile.as_deref().unwrap().clone().as_hdr_for_test();
        r.profile = Some(std::sync::Arc::new(hdr));
        assert!(
            !supported(&r),
            "an HDR profile's SDR rendition renders on the CPU"
        );
    }

    /// A switched-off panel's adjustments don't refuse the master.
    #[test]
    fn a_switched_off_panel_does_not_refuse_the_master() -> anyhow::Result<()> {
        use crate::model::panels::{Panel, PanelState};
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        r.effects.vignette = -0.3;
        assert!(render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER).is_err());
        r.panels.set(Panel::Effects, PanelState::Off);
        render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        Ok(())
    }

    /// A resized master keeps its highlights: `image`'s resize clips float samples to
    /// 0–1, which an HDR master's encoded values exceed.
    #[test]
    fn a_resized_master_keeps_light_above_white() -> anyhow::Result<()> {
        let im = grays(&[0.2, 4.064, 4.064, 4.064]);
        let r = hdr_recipe(&im);
        let full = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let small = render_hdr(&im, &r.checked()?, None, 16, Cancel::NEVER)?;
        assert_eq!(small.width, 16);
        let peak =
            |o: &crate::rendered::HdrImage| o.pixels.iter().flatten().copied().fold(0f32, f32::max);
        assert!(peak(&full) > 16.);
        assert!(
            (peak(&small) / peak(&full) - 1.).abs() < 0.05,
            "{} {}",
            peak(&small),
            peak(&full)
        );
        Ok(())
    }

    /// A region of the master at 100% is that part of the whole master, sharpening
    /// included, for a photo and for an HDR file's light; an invalid region is refused.
    #[test]
    fn a_region_of_the_master_is_that_part_of_the_whole() -> Result<()> {
        let same = |whole: &crate::rendered::HdrImage,
                    part: &crate::rendered::HdrImage,
                    [x, y, w, h]: [u32; 4]| {
            assert_eq!((part.width, part.height), (w, h));
            for row in 0..h {
                for col in 0..w {
                    let a = whole.pixels[((y + row) * whole.width + x + col) as usize];
                    let b = part.pixels[(row * w + col) as usize];
                    for c in 0..3 {
                        assert!(
                            (a[c] - b[c]).abs() <= 1e-4 * a[c].abs().max(1.),
                            "{a:?} {b:?}"
                        );
                    }
                }
            }
        };
        let im = grays(&[0.05, 0.2, 1.2, 4.064, 0.4, 2.]);
        let mut r = hdr_recipe(&im);
        r.sharpening = 0.6;
        let r = r.checked()?;
        let whole = render_hdr(&im, &r, None, 0, Cancel::NEVER)?;
        for region in [[0, 0, 48, 8], [8, 1, 9, 5], [31, 2, 17, 6]] {
            same(
                &whole,
                &render_hdr_region(&im, &r, region, Cancel::NEVER)?,
                region,
            );
        }
        assert!(render_hdr_region(&im, &r, [40, 0, 9, 8], Cancel::NEVER).is_err());
        assert!(render_hdr_region(&im, &r, [0, 0, 0, 8], Cancel::NEVER).is_err());

        let file = hdr_file(6, 4, |x, y| [0.5 * x as f32, 0.1 * y as f32, 3.]);
        let r = Recipe::for_new_rendered(&file.metadata);
        let r = r.checked()?;
        let whole = render_hdr(&file, &r, None, 0, Cancel::NEVER)?;
        same(
            &whole,
            &render_hdr_region(&file, &r, [2, 1, 3, 2], Cancel::NEVER)?,
            [2, 1, 3, 2],
        );
        Ok(())
    }

    #[test]
    fn an_sdr_edit_has_no_hdr_master() {
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        r.hdr.enabled = false;
        assert!(render_hdr(&im, &r.checked().unwrap(), None, 0, Cancel::NEVER).is_err());
    }
}
