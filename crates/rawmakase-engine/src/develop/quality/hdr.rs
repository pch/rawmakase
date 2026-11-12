//! The HDR master of an edit with HDR on (docs/records/hdr-implementation-plan.md,
//! stage 3).
use super::*;
use rawmakase_cancel::Cancel;

/// The HDR master of an edit with HDR on, on the CPU: the whole photo within
/// `max_edge`, the reference for every other render of it: its SDR rendition is
/// `Recipe::resolved`'s, and the GPU's preview of the master
/// (`PreviewRenderer::render_hdr_to`) is checked against this.
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
/// [`render_hdr`] renders there at full size, sharpening and AI Denoise included.
pub fn render_hdr_region(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    region: [u32; 4],
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    master(im, r, ai, Part::Region(region), cancel)
}

/// A preview of the whole HDR master within `max_edge`, as Develop shows it before
/// a 100% view: [`render_hdr`] of the photo reduced to twice that edge (as
/// [`crate::develop::preview`] reduces it), with the AI Denoise source reduced
/// alike, so an edit with AI Denoise previews its result.
pub fn render_hdr_preview(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    let reduce = max_edge.saturating_mul(2);
    let reduced = crate::develop::preview(im, reduce);
    let ai = ai.filter(|a| a.fits(im)).map(|a| a.reduced(reduce));
    master(&reduced, r, ai.as_ref(), Part::Whole(max_edge), cancel)
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
        let gain = hdr_file_recipe(recipe.to_mut(), &im.metadata);
        scene = hdr_file_scene(im, gain)?;
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
    // An active mask's Sharpness reads as far as sharpening does, and its Noise two
    // pixels further (`local_noise`'s 5×5 average, after sharpening), so a region
    // reads the neighbours the whole master does.
    let masks = || r.masks.iter().filter(|m| m.is_active());
    let sharpens = r.sharpening > 0. || masks().any(|m| m.adjust.sharpness != 0.);
    let halo = if sharpens {
        ((3. * sigma).ceil() as u32).max(1)
    } else {
        0
    } + if masks().any(|m| m.adjust.noise > 0.) {
        2
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
    let (mut out, weights) =
        develop::render_base(&toned, &tonal_recipe, &g, base, 0., cancel, None, srgb)?;
    // A mask's Sharpness and Noise finish the master as they finish SDR renders, on
    // its encoded values: they change detail, not tone.
    let local_finish = weights
        .as_ref()
        .is_some_and(|w| w.uses(&[slot::SHARPNESS, slot::NOISE]));
    check_cancel(cancel)?;
    let peak = r.hdr.peak();
    let encode = |p: &mut [f32; 3]| *p = p.map(|v| srgb_encode(v.max(0.)));
    let decode = |p: &mut [f32; 3]| *p = p.map(|v| srgb_decode(v.max(0.)));
    // Vignette and grain follow sharpening, before the crop to the region and resizing,
    // as in SDR renders, on linear values.
    let effects = r.effects.grain != 0. || r.effects.vignette != 0.;
    let finish_effects = |out: &mut crate::rendered::Rendered| -> Result<()> {
        let neutral = develop::pipeline::hdr_neutral_response(toned.source(), &tonal_recipe);
        let neutral = anyhow::Context::context(neutral, "This edit renders no HDR master")?;
        let response = crate::develop::effects::HdrResponse::new(neutral);
        crate::develop::effects::hdr_spatial_finish(
            out,
            r,
            [left, top],
            [g.width, g.height],
            scale,
            &response,
        );
        Ok(())
    };
    let resized = if r.sharpening > 0.
        || local_finish
        || output_size(g.width, g.height, max_edge) != (g.width, g.height)
    {
        out.pixels.par_iter_mut().for_each(encode);
        let prophoto = crate::color::OutputSpace::ProPhoto;
        sharpen_unbounded(&mut out, r, sigma, weights.as_deref(), prophoto, cancel)?;
        local_noise(&mut out, weights.as_deref(), prophoto);
        if effects {
            out.pixels.par_iter_mut().for_each(decode);
            finish_effects(&mut out)?;
            out.pixels.par_iter_mut().for_each(encode);
        }
        let out = super::render::crop(out, [x - left, y - top, w, h]);
        let mut out = resize_unbounded(out, max_edge);
        out.pixels
            .par_iter_mut()
            .for_each(|p| *p = p.map(|v| srgb_decode(v.max(0.))));
        out
    } else {
        if effects {
            finish_effects(&mut out)?;
        }
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

/// The recipe an HDR file's pseudo-scene ([`hdr_file_scene`]) renders with, from its
/// own resolved with HDR on: the profile (the Embedded one when the edit has none)
/// with a linear curve, and no Exposure, whose gain of the light it returns for the
/// scene to apply.
pub(crate) fn hdr_file_recipe(
    recipe: &mut crate::model::recipe::Recipe,
    metadata: &crate::camera_data::Metadata,
) -> f32 {
    let gain = recipe.exposure.exp2();
    recipe.exposure = 0.;
    let profile = match recipe.profile.as_deref() {
        Some(p) => p.for_hdr_light(),
        None => crate::camera_profiles::CameraProfile::embedded(metadata).for_hdr_light(),
    };
    recipe.profile = Some(std::sync::Arc::new(profile));
    gain
}

/// An HDR file as the scene its HDR master renders (docs/records/hdr-stage4-design.md):
/// its light through [`crate::develop::rendered::scene_from_light`], which the master
/// at the file's own settings (`Recipe::for_rendered`, the Embedded profile with a
/// linear curve) renders as the light again. `im` is the decoded photo or its preview reduction, which hold
/// the light; a working image doesn't, and a fact in its metadata alone renders
/// nothing. The scene carries no light of its own, so no stage can reach it. `gain`
/// scales the light first.
pub(crate) fn hdr_file_scene(im: &CameraImage, gain: f32) -> Result<CameraImage> {
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

    /// The SDR rendition's sliders render on an HDR edit's SDR rendition, as Camera Raw
    /// 18.7 changes its own (Brightness +50 about half a stop brighter, −50 darker), and
    /// never on its master; an edit with HDR off keeps them unrendered, as Camera Raw
    /// keeps values set while HDR was off.
    #[test]
    fn the_sdr_rendition_renders_its_own_sliders() -> anyhow::Result<()> {
        let im = grays(&[0.05, 0.18, 0.4]);
        let r = hdr_recipe(&im);
        let sdr = |r: &Recipe| crate::develop::render(&im, &r.checked().unwrap(), 0, Cancel::NEVER);
        // The darkest patch's luminance, where Brightness ±50 moves about half a stop.
        let mean = |out: &crate::rendered::Rendered| {
            crate::color::luminance(
                out.pixels[4 * out.width as usize + 4].map(crate::color::srgb_decode),
            )
        };
        let plain = sdr(&r)?;
        let master = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let mut brighter = r.clone();
        brighter.hdr.sdr.brightness = 0.5;
        let lifted = sdr(&brighter)?;
        let stops = (mean(&lifted) / mean(&plain)).log2();
        assert!((0.4..0.6).contains(&stops), "{stops}");
        assert_eq!(
            render_hdr(&im, &brighter.checked()?, None, 0, Cancel::NEVER)?.pixels,
            master.pixels
        );
        brighter.hdr.sdr.brightness = -0.5;
        let stops = (mean(&sdr(&brighter)?) / mean(&plain)).log2();
        assert!((-0.6..-0.4).contains(&stops), "{stops}");
        // Beyond the measured range none renders.
        brighter.hdr.sdr.brightness = -0.8;
        assert_eq!(sdr(&brighter)?.pixels, plain.pixels);
        // HDR off: the values are kept, not rendered.
        let mut off = r;
        off.hdr.enabled = false;
        let unedited = sdr(&off)?;
        off.hdr.sdr.brightness = 0.5;
        assert_eq!(sdr(&off)?.pixels, unedited.pixels);
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
        r.effects.clarity = -0.6;
        let error = master(&r).unwrap_err().to_string();
        assert!(error.contains("not supported in HDR"), "{error}");
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        r.effects.clarity = 0.;
        // The SDR rendition's sliders never change the master, as in Camera Raw 18.7:
        // it renders with them, bit for bit as without.
        let without = master(&r)?;
        r.hdr.sdr.brightness = 0.2;
        r.hdr.sdr.blend = -0.4;
        r.hdr.sdr.clarity = 0.6;
        assert_eq!(master(&r)?.pixels, without.pixels);
        r.hdr.sdr = Default::default();
        // A look renders its master at any Profile Amount and with an RGB table; a
        // setting of its own that the master doesn't render refuses it.
        r.profile = Some(std::sync::Arc::new(CameraProfile::creative_for_test(
            &im.metadata,
        )));
        master(&r)?;
        r.profile_amount = 0.5;
        master(&r)?;
        r.profile_amount = 1.;
        let rgb = CameraProfile::creative_for_test(&im.metadata).with_test_rgb_tables();
        r.profile = Some(std::sync::Arc::new(rgb[0].clone()));
        master(&r)?;
        // A look's Clarity +10 (Adobe Vivid's) renders; above +50 it refuses.
        let mut clarity = rgb[0].clone();
        clarity.enhanced.as_mut().unwrap().clarity = 0.1;
        r.profile = Some(std::sync::Arc::new(clarity.clone()));
        master(&r)?;
        clarity.enhanced.as_mut().unwrap().clarity = 0.6;
        r.profile = Some(std::sync::Arc::new(clarity));
        let error = master(&r).unwrap_err().to_string();
        assert!(error.contains("which the look sets"), "{error}");
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        Ok(())
    }

    /// Red Eye renders in the master as in its SDR rendition: it corrects the camera's
    /// samples before tone, so a red pupil turns to the same dark neutral in both, and
    /// light above white elsewhere stays.
    #[test]
    fn red_eye_renders_in_the_master() -> anyhow::Result<()> {
        use crate::model::image_frame::ImageFrame;
        use crate::model::red_eye::{EyeKind, RedEyeOp};
        let im = patches(&[[3., 3., 3.], [0.6, 0.05, 0.04], [0.2, 0.2, 0.2]]);
        let mut r = hdr_recipe(&im);
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let frame = ImageFrame::new(&im);
        let side = 3. / frame.long_edge();
        r.red_eye = vec![RedEyeOp {
            kind: EyeKind::Red,
            center: frame.to_image(12., 4.),
            radius: [side, side],
            correlation: 0.,
            pupil_size: 0.5,
            darken: 0.5,
        }]
        .into();
        let fixed = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let at = |out: &crate::rendered::HdrImage, column: usize| {
            out.pixels[4 * out.width as usize + column * 8 + 4]
        };
        let (p, q) = (at(&fixed, 1), at(&plain, 1));
        assert!(p[0] < 0.5 * q[0] && p[0] < 1.3 * p[1], "{p:?} {q:?}");
        assert_eq!(at(&fixed, 0), at(&plain, 0), "light above white stays");
        let sdr = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        let s = sdr.pixels[4 * sdr.width as usize + 8 + 4];
        assert!(s[0] < 1.3 * s[1], "{s:?}");
        Ok(())
    }

    /// A look's split toning renders in the master as colour grading does, in its HDR
    /// domain: it tints the master's grays below and above white, and stays finite.
    #[test]
    fn a_looks_split_toning_renders_in_the_master() -> anyhow::Result<()> {
        let im = grays(&[0.05, 0.3, 1.2, 40.]);
        let mut r = hdr_recipe(&im);
        let look = CameraProfile::creative_for_test(&im.metadata);
        r.profile = Some(std::sync::Arc::new(look.clone()));
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let mut toned = look;
        toned.enhanced.as_mut().unwrap().settings.toning = Some(crate::camera_profiles::Toning {
            shadows: [210. / 360., 0.4],
            highlights: [50. / 360., 0.3],
            balance: 0.,
        });
        r.profile = Some(std::sync::Arc::new(toned));
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let at =
            |o: &crate::rendered::HdrImage, k: usize| o.pixels[4 * o.width as usize + k * 8 + 4];
        let tint = |p: [f32; 3]| p[2] / p[0];
        assert!(
            tint(at(&out, 0)) > 1.05 * tint(at(&plain, 0)),
            "cool shadows"
        );
        assert!(
            tint(at(&out, 2)) < tint(at(&plain, 2)) / 1.02,
            "warm highlights"
        );
        assert!(
            out.pixels
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.)
        );
        Ok(())
    }

    /// A `.cube` look's RGB table renders a gray ramp through the ACR3 curve as Camera
    /// Raw 18.7 renders it at HDR limit 8: on overrange-encoded values, and above 16
    /// times white with the light beyond added back (the levels' default renders are
    /// those of `an_hdr_master_extends_above_white_as_camera_raw_does`).
    #[test]
    fn an_rgb_table_renders_as_camera_raw_renders_it() -> anyhow::Result<()> {
        let im = grays(&[0.1887, 0.4789, 1.2006, 4.064]);
        // scripts/corpus/synthetic-cubes.py's 1D cube: a curve for each channel.
        let mut cube = String::from("LUT_1D_SIZE 1024\n");
        for i in 0..1024 {
            let v = i as f64 / 1023.;
            let rgb = [
                0.05 + 0.9 * v.powf(0.85),
                v + 0.06 * (std::f64::consts::TAU * v).sin(),
                0.1 + 0.75 * v,
            ];
            cube += &format!("{:.6} {:.6} {:.6}\n", rgb[0], rgb[1], rgb[2]);
        }
        let mut r = hdr_recipe(&im);
        let look = r.profile.as_deref().unwrap().with_cube_for_test(&cube)?;
        r.profile = Some(std::sync::Arc::new(look));
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        for (i, expected) in [0.3832, 0.7807, 2.5065, 17.2935].iter().enumerate() {
            let got = centre(&out, i);
            assert!(
                (got / expected).log2().abs() < 0.05,
                "{i}: {got} {expected}"
            );
        }
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
    /// through sharpening and resizing and reaches the curve's hold (about 49 times
    /// white, `hdr_curve::hold`) or the limit, whichever is lower, not black.
    #[test]
    fn a_steep_curve_rolls_off_to_the_limit_through_sharpening_and_resizing() -> anyhow::Result<()>
    {
        // All above white after the ACR3 curve, so on the curve's extension.
        let im = grays(&[0.5, 2., 8.]);
        let mut r = hdr_recipe(&im);
        r.curve.points = vec![[0.99, 0.], [1., 1.]];
        r.sharpening = 0.5;
        let held = crate::color::hdr_curve::decode(crate::color::hdr_curve::HOLD);
        for max_edge in [0, 12] {
            let out = render_hdr(&im, &r.checked()?, None, max_edge, Cancel::NEVER)?;
            // Every gray renders near the hold or the limit: none is lost to NaN.
            let darkest = out.pixels.iter().flatten().fold(f32::MAX, |a, v| a.min(*v));
            assert!(darkest > out.peak.min(held) * 0.85, "{max_edge}: {darkest}");
            assert!(out.pixels.iter().flatten().all(|v| v.is_finite()));
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

    /// A channel's point curve renders in the master after the master curve, on that
    /// channel of encoded ProPhoto RGB, along its end slope above white and held at
    /// 16 (`hdr_curve::channel`); the other channels keep the master curve's output.
    #[test]
    fn channel_curves_render_on_their_own_channel_after_the_master_curve() -> anyhow::Result<()> {
        use crate::color::curve::ToneCurve;
        use crate::color::hdr_curve::{HdrPointCurve, channel, encode};
        let im = grays(&[0.2, 0.5, 1.2, 4., 16.]);
        let mut r = hdr_recipe(&im);
        r.curve.points = vec![[0., 0.], [0.5, 0.55], [1., 1.]];
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let at = |out: &crate::rendered::HdrImage, i: usize| {
            out.pixels[4 * out.width as usize + i * 8 + 4]
        };
        let red = ToneCurve {
            points: vec![[0., 0.], [0.5, 0.63], [1., 1.]],
            ..Default::default()
        };
        let blue = ToneCurve {
            points: vec![[0., 0.], [0.5, 0.39], [1., 1.]],
            ..Default::default()
        };
        r.effects.channels[0] = red.clone();
        r.effects.channels[2] = blue.clone();
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let curves = [HdrPointCurve::new(&red), HdrPointCurve::new(&blue)];
        for i in 0..5 {
            let (p, q) = (at(&plain, i), at(&out, i));
            let want = [
                channel(&curves[0], encode(p[0])),
                p[1],
                channel(&curves[1], encode(p[2])),
            ];
            for c in 0..3 {
                assert!(
                    (q[c] / want[c] - 1.).abs() < 1e-3,
                    "{i}, channel {c}: {} against {}",
                    q[c],
                    want[c]
                );
            }
        }
        // The darkening blue curve ends steeper than the identity, and holds at 16.
        assert!(at(&out, 3)[2] > at(&plain, 3)[2]);
        assert!((at(&out, 4)[2] - 16.).abs() < 1e-3, "{:?}", at(&out, 4));
        Ok(())
    }

    /// The colour stage renders above white as Camera Raw 18.7 renders it in HDR
    /// (docs/hdr.md#the-colour-stage): Saturation changes a colour's saturation as much
    /// at 20 times white as at a quarter of it, Vibrance's brightening leaves colours
    /// above white alone, a black & white master is gray and unclipped, and
    /// Calibration and band Luminance render.
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
        // Band Luminance brightens the orange at a quarter of white and leaves its
        // value alone at 20 times white; negative Luminance renders down to −100, and
        // above +40 it refuses the master.
        let [(low, _, _), (high, _, _)] = both(|r| r.hsl[1][2] = 0.4)?;
        assert!(value(low) > value(base_low) * 1.002, "{low:?} {base_low:?}");
        assert!(
            (value(high) / value(base_high) - 1.).abs() < 1e-3,
            "{high:?} {base_high:?}"
        );
        assert!(at(0., |r| r.hsl[1][2] = 0.5).is_err());
        let [(low, _, _), _] = both(|r| r.hsl[1][2] = -1.)?;
        assert!(value(low) < value(base_low) * 0.995, "{low:?} {base_low:?}");

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

    /// Colour grading renders in the master as Camera Raw 18.7 renders it
    /// (docs/hdr.md#color-grading): the SDR curves in a domain that compresses the HDR
    /// range, so Highlights tint and lift most a few stops above white, every region
    /// fades out by 16 times white, and Highlights darkening keeps darkening up to it.
    #[test]
    fn colour_grading_spreads_its_regions_over_the_hdr_range() -> anyhow::Result<()> {
        // Gray scene values the ACR3 curve's HDR extension renders at about 0.05,
        // 3 and 16 times white.
        let levels = [0.03, 0.9, 3.5];
        let im = grays(&levels);
        let render = |edit: fn(&mut Recipe)| -> anyhow::Result<Vec<[f32; 3]>> {
            let mut r = hdr_recipe(&im);
            edit(&mut r);
            let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
            Ok((0..levels.len())
                .map(|k| out.pixels[4 * out.width as usize + k * 8 + 4])
                .collect())
        };
        let base = render(|_| {})?;
        let y = |p: [f32; 3]| crate::develop::scene_tone::luminance(p);
        assert!(
            y(base[0]) < 0.1 && (2. ..5.).contains(&y(base[1])) && y(base[2]) > 12.,
            "{base:?}"
        );
        let gain = |p: &[[f32; 3]], k: usize| y(p[k]) / y(base[k]);
        // Highlights Luminance +50: Camera Raw's ×1.60 near three times white (SDR's
        // largest gain is ×1.15, at half of white), ×1.02 in the shadows.
        let lifted = render(|r| r.grading[2] = [0., 0., 0.5])?;
        assert!((1.4..1.8).contains(&gain(&lifted, 1)), "{lifted:?}");
        assert!(gain(&lifted, 0) < 1.05, "{lifted:?}");
        // Highlights Luminance −50: still darkening near 16 times white.
        let darker = render(|r| r.grading[2] = [0., 0., -0.5])?;
        assert!(gain(&darker, 2) < 0.6, "{darker:?}");
        // A blue Highlights tint is strong above white and nothing turns negative or
        // infinite.
        let blue = render(|r| r.grading[2] = [210. / 360., 0.5, 0.])?;
        let ratio = |p: [f32; 3]| p[2] / p[0];
        assert!(ratio(blue[1]) > 1.5 * ratio(base[1]), "{blue:?}");
        assert!(blue.iter().flatten().all(|v| v.is_finite() && *v >= 0.));
        // Global's lift is a gain from the shadows to above white.
        let global = render(|r| r.effects.global_grade = [0., 0., 0.5])?;
        assert!(
            (gain(&global, 0) / gain(&global, 1) - 1.).abs() < 0.05,
            "{global:?}"
        );
        Ok(())
    }

    /// The post-crop vignette and grain render in the master (docs/hdr.md#vignette-and-grain):
    /// the vignette leaves the frame's middle alone and changes its corners, above
    /// white too, a 100% region renders them as the whole master does there, and the
    /// GPU leaves them to the CPU.
    #[test]
    fn vignette_and_grain_render_in_the_master() -> Result<()> {
        use crate::develop::pipeline::pixel_params::supported_hdr;
        let im = grays(&[2.; 8]);
        let mut r = hdr_recipe(&im);
        r.sharpening = 0.4;
        let base = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let at =
            |o: &crate::rendered::HdrImage, x: u32, y: u32| o.pixels[(y * o.width + x) as usize][1];
        assert!(at(&base, 0, 0) > 4., "{}", at(&base, 0, 0));
        for style in [
            crate::model::effects::VignetteStyle::PaintOverlay,
            crate::model::effects::VignetteStyle::HighlightPriority,
            crate::model::effects::VignetteStyle::ColorPriority,
        ] {
            let mut v = r.clone();
            v.effects.vignette = -0.6;
            v.effects.vignette_style = style;
            assert!(
                !supported_hdr(&v, &im.metadata),
                "the GPU leaves it to the CPU"
            );
            let v = v.checked()?;
            let out = render_hdr(&im, &v, None, 0, Cancel::NEVER)?;
            let (w, h) = (out.width, out.height);
            assert!((at(&out, w / 2, h / 2) / at(&base, w / 2, h / 2) - 1.).abs() < 0.02);
            assert!(at(&out, 0, 0) < 0.8 * at(&base, 0, 0), "{style:?}");
            let region = render_hdr_region(&im, &v, None, [40, 1, 24, 6], Cancel::NEVER)?;
            for row in 0..6 {
                for col in 0..24 {
                    let a = out.pixels[((1 + row) * w + 40 + col) as usize];
                    let b = region.pixels[(row * 24 + col) as usize];
                    assert!(
                        (0..3).all(|c| (a[c] - b[c]).abs() <= 1e-4 * a[c].max(1.)),
                        "{a:?} {b:?}"
                    );
                }
            }
        }
        let mut grain = r;
        grain.effects.grain = 0.6;
        let out = render_hdr(&im, &grain.checked()?, None, 0, Cancel::NEVER)?;
        let spread = out
            .pixels
            .iter()
            .map(|p| (p[1] - at(&base, 0, 0)).abs())
            .fold(0f32, f32::max);
        assert!(
            spread > 0.05
                && out
                    .pixels
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && *v >= 0.)
        );
        Ok(())
    }

    /// A mask renders in the master (docs/hdr.md#masks): over the whole frame, its
    /// Exposure, Shadows and Texture render the master the global slider renders, above
    /// white too, and half a frame's mask changes only its half; its Contrast and Color
    /// leave light above white alone, its Whites lifts light above white too, and
    /// Sharpness and Noise leave tone alone. Blacks, Hue, Saturation and Highlights
    /// refuse the master by name, and the GPU leaves a masked master to the CPU.
    #[test]
    fn a_mask_renders_in_the_master_with_the_global_sliders_it_shares() -> Result<()> {
        use crate::develop::pipeline::pixel_params::supported_hdr;
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        let im = patches(&[
            [0.02, 0.02, 0.02],
            [0.1, 0.08, 0.07],
            [0.4, 0.4, 0.4],
            [1.5, 1.2, 1.],
            [3., 3., 3.],
            [0.3, 0.5, 0.8],
        ]);
        let base = hdr_recipe(&im);
        let masked = |adjust: LocalAdjust, from: [f32; 2], to: [f32; 2], invert: bool| {
            let mut r = base.clone();
            r.masks = vec![MaskGroup {
                components: vec![MaskComponent {
                    invert,
                    ..MaskComponent::new(MaskShape::Linear { from, to })
                }],
                adjust,
                ..Default::default()
            }];
            r
        };
        // A linear gradient with no effect from above the frame, inverted, covers all
        // of it.
        let everywhere = |adjust| masked(adjust, [0., -3.], [0., -2.9], true);
        let render = |r: &Recipe| render_hdr(&im, &r.checked().unwrap(), None, 0, Cancel::NEVER);
        let close = |a: &crate::rendered::HdrImage, b: &crate::rendered::HdrImage, what: &str| {
            for (p, q) in a.pixels.iter().zip(&b.pixels) {
                for c in 0..3 {
                    assert!(
                        (p[c] - q[c]).abs() <= 2e-3 * q[c].max(0.05),
                        "{what}: {p:?} {q:?}"
                    );
                }
            }
        };
        for (adjust, global) in [
            (
                LocalAdjust {
                    exposure: 1.,
                    ..Default::default()
                },
                Recipe {
                    exposure: 1.,
                    ..base.clone()
                },
            ),
            (
                LocalAdjust {
                    shadows: 0.5,
                    ..Default::default()
                },
                Recipe {
                    shadows: 0.5,
                    ..base.clone()
                },
            ),
            (
                LocalAdjust {
                    clarity: 0.3,
                    ..Default::default()
                },
                Recipe {
                    effects: crate::model::effects::Effects {
                        clarity: 0.3,
                        ..base.effects.clone()
                    },
                    ..base.clone()
                },
            ),
            (
                LocalAdjust {
                    texture: 0.4,
                    ..Default::default()
                },
                Recipe {
                    effects: crate::model::effects::Effects {
                        texture: 0.4,
                        ..base.effects.clone()
                    },
                    ..base.clone()
                },
            ),
        ] {
            let mask = everywhere(adjust);
            assert!(
                !supported_hdr(&mask, &im.metadata),
                "the GPU leaves it to the CPU"
            );
            close(&render(&mask)?, &render(&global)?, &format!("{adjust:?}"));
        }
        let warm = render(&everywhere(LocalAdjust {
            temperature: 0.4,
            ..Default::default()
        }))?;
        let plain = render(&base)?;
        let p = warm.pixels[4 * warm.width as usize + 3 * 8 + 4];
        let q = plain.pixels[4 * plain.width as usize + 3 * 8 + 4];
        assert!(p[0] / p[2] > 1.05 * (q[0] / q[2]), "{p:?} {q:?}");
        // The bottom of the frame: full effect below 0.6 of its height, none above 0.4.
        let half = render(&masked(
            LocalAdjust {
                exposure: 1.,
                ..Default::default()
            },
            [0., 0.6],
            [0., 0.4],
            false,
        ))?;
        let w = half.width as usize;
        let last = (half.height as usize - 1) * w;
        let (top, bottom) = (half.pixels[2 * 8 + 4], half.pixels[last + 2 * 8 + 4]);
        let (top0, bottom0) = (plain.pixels[2 * 8 + 4], plain.pixels[last + 2 * 8 + 4]);
        assert!(
            bottom[1] > 1.5 * bottom0[1] && (top[1] / top0[1] - 1.).abs() < 1e-3,
            "{top:?} {bottom:?}"
        );
        // A mask's Contrast is SDR's below white and changes nothing above it; its
        // Color tints below white and fades out by white (Camera Raw 18.7's HDR
        // renders, docs/records/hdr-stage1-results.md#masks).
        let gray = |out: &crate::rendered::HdrImage, column: usize| centre(out, column);
        let contrast = render(&everywhere(LocalAdjust {
            contrast: 0.5,
            ..Default::default()
        }))?;
        assert!(gray(&contrast, 0) < 0.9 * gray(&plain, 0), "darker shadows");
        assert!(
            (gray(&contrast, 4) / gray(&plain, 4) - 1.).abs() < 1e-4,
            "{} {}",
            gray(&contrast, 4),
            gray(&plain, 4)
        );
        let tinted = render(&everywhere(LocalAdjust {
            color: [0.6, 0.8],
            ..Default::default()
        }))?;
        let at = |out: &crate::rendered::HdrImage, column: usize| {
            out.pixels[4 * out.width as usize + column * 8 + 4]
        };
        let (t, q) = (at(&tinted, 1), at(&plain, 1));
        assert!(t[2] / t[0] > 1.1 * q[2] / q[0], "{t:?} {q:?}");
        let (t, q) = (at(&tinted, 4), at(&plain, 4));
        assert!(
            t.iter().zip(&q).all(|(a, b)| (a / b - 1.).abs() < 1e-4),
            "{t:?} {q:?}"
        );
        // Whites follows Camera Raw's HDR mask Whites: a gain that keeps going above
        // white, where SDR's mask curve falls (docs/records/hdr-stage1-results.md#masks).
        let whites = render(&everywhere(LocalAdjust {
            whites: 0.6,
            ..Default::default()
        }))?;
        for column in [0, 3, 4] {
            let lift = (gray(&whites, column) / gray(&plain, column)).log2();
            assert!((0.15..0.8).contains(&lift), "{column}: {lift}");
        }
        assert!(
            gray(&whites, 4) / gray(&plain, 4) > gray(&whites, 0) / gray(&plain, 0),
            "lifts light above white more than the shadows"
        );
        // Sharpness and Noise change detail, not tone: flat patches keep their light,
        // above white too.
        for adjust in [
            LocalAdjust {
                sharpness: 0.8,
                ..Default::default()
            },
            LocalAdjust {
                noise: 0.8,
                ..Default::default()
            },
        ] {
            let out = render(&everywhere(adjust))?;
            for column in 0..6 {
                let (p, q) = (at(&out, column), at(&plain, column));
                assert!(
                    p.iter()
                        .zip(&q)
                        .all(|(a, b)| (a - b).abs() <= 1e-3 * b.max(0.05)),
                    "{adjust:?} {column}: {p:?} {q:?}"
                );
            }
        }
        for (name, adjust) in [
            (
                "Saturation",
                LocalAdjust {
                    saturation: -0.3,
                    ..Default::default()
                },
            ),
            (
                "Highlights",
                LocalAdjust {
                    highlights: -0.3,
                    ..Default::default()
                },
            ),
            (
                "Blacks",
                LocalAdjust {
                    blacks: -0.3,
                    ..Default::default()
                },
            ),
            (
                "Hue",
                LocalAdjust {
                    hue: 20.,
                    ..Default::default()
                },
            ),
        ] {
            let error = render(&everywhere(adjust)).unwrap_err().to_string();
            assert!(error.contains(&format!("A mask's {name}")), "{error}");
        }
        Ok(())
    }

    /// A range mask selects from the SDR rendition, as the mask overlay shows it, so
    /// the master adjusts the pixels its SDR rendition adjusts: Luminance Range by the
    /// display luminance of the SDR rendition, not of the master's light.
    #[test]
    fn a_range_mask_selects_in_the_master_what_it_selects_in_the_sdr_rendition() -> Result<()> {
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        let levels = [0.01, 0.04, 0.1, 0.2, 0.4, 0.8, 2., 6.];
        let im = grays(&levels);
        let mut r = hdr_recipe(&im);
        r.masks = vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::LuminanceRange {
                low: 0.45,
                high: 0.75,
                falloff: [0., 0.],
            })],
            adjust: LocalAdjust {
                exposure: 1.,
                ..Default::default()
            },
            ..Default::default()
        }];
        let mut plain = r.clone();
        plain.masks.clear();
        let gains = |sdr: bool| -> Result<Vec<f32>> {
            let render = |r: &Recipe| -> Result<Vec<f32>> {
                let mut r = r.clone();
                r.hdr.enabled = !sdr;
                Ok(if sdr {
                    let out = crate::develop::quality::render(
                        &im,
                        &r.checked()?,
                        0,
                        None,
                        Cancel::NEVER,
                    )?;
                    (0..levels.len())
                        .map(|k| {
                            let p = out.pixels[4 * out.width as usize + k * 8 + 4];
                            crate::color::srgb_decode(p[1])
                        })
                        .collect()
                } else {
                    let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
                    (0..levels.len()).map(|k| centre(&out, k)).collect()
                })
            };
            let (a, b) = (render(&r)?, render(&plain)?);
            Ok(a.iter().zip(&b).map(|(a, b)| a / b.max(1e-6)).collect())
        };
        let (sdr, hdr) = (gains(true)?, gains(false)?);
        let selected = |g: &[f32]| g.iter().map(|g| *g > 1.2).collect::<Vec<_>>();
        assert!(
            selected(&sdr).iter().any(|s| *s) && selected(&sdr).iter().any(|s| !*s),
            "{sdr:?}"
        );
        assert_eq!(selected(&sdr), selected(&hdr), "SDR {sdr:?} HDR {hdr:?}");
        Ok(())
    }

    /// The SDR rendition's sliders change neither what a range mask selects nor, so,
    /// the master: its selection is the edit's, without them.
    #[test]
    fn the_sdr_rendition_sliders_leave_a_masked_master_as_it_is() -> Result<()> {
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        let im = grays(&[0.01, 0.04, 0.1, 0.2, 0.4, 0.8, 2., 6.]);
        let mut r = hdr_recipe(&im);
        r.masks = vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::LuminanceRange {
                low: 0.3,
                high: 0.6,
                falloff: [0.1, 0.1],
            })],
            adjust: LocalAdjust {
                exposure: 1.,
                ..Default::default()
            },
            ..Default::default()
        }];
        let master = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.hdr.sdr.brightness = 1.;
        let brighter = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        assert_eq!(brighter.pixels, master.pixels);
        Ok(())
    }

    /// Point Color renders in the master as Camera Raw 18.7 renders it
    /// (docs/hdr.md#point-color): its swatches in colour grading's HDR domain, so a
    /// Luminance shift lifts a colour at white several times (Camera Raw's ×3.5 at
    /// Luminance +50) where SDR clips it, another hue keeps its colour, and the GPU
    /// leaves the master to the CPU.
    #[test]
    fn point_color_works_in_the_hdr_domain() -> Result<()> {
        use crate::develop::pipeline::pixel_params::supported_hdr;
        use crate::model::point_color::PointColor;
        let im = patches(&[[0.12, 0.07, 0.03], [0.03, 0.05, 0.12]]);
        let pixel = |out: &crate::rendered::HdrImage, k: usize| {
            out.pixels[4 * out.width as usize + k * 8 + 4]
        };
        let mut r = hdr_recipe(&im);
        // The orange patch at about white.
        let mut base = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let scale = 1. / pixel(&base, 0).into_iter().fold(0f32, f32::max);
        r.exposure = scale.log2();
        base = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let orange = pixel(&base, 0);
        let max = orange.into_iter().fold(0f32, f32::max);
        let min = orange.into_iter().fold(f32::INFINITY, f32::min);
        let hue = {
            let [r, g, b] = orange;
            let d = max - min;
            if max == r {
                ((g - b) / d).rem_euclid(6.)
            } else if max == g {
                (b - r) / d + 2.
            } else {
                (r - g) / d + 4.
            }
        };
        let mut swatch = PointColor::sampled([hue, (max - min) / max, 0.5]);
        swatch.luminance_range = [0., 0., 1., 1.];
        swatch.shift = [0., 0., 0.5];
        r.point_colors = vec![swatch];
        assert!(
            !supported_hdr(&r, &im.metadata),
            "the GPU leaves it to the CPU"
        );
        let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        let y = |p: [f32; 3]| crate::develop::scene_tone::luminance(p);
        let lifted = y(pixel(&out, 0)) / y(orange);
        assert!((2.5..4.5).contains(&lifted), "{lifted}");
        assert!((y(pixel(&out, 1)) / y(pixel(&base, 1)) - 1.).abs() < 1e-3);
        assert!(
            out.pixels
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.)
        );
        Ok(())
    }

    /// Highlight Priority's vignette exposes through what the master renders a neutral
    /// as (`pipeline::hdr_neutral_response`): the scene tone stage's toe, Whites and
    /// Blacks and the finish at the edit's settings, so neutral patches render as the
    /// response gives them, and positive Shadows' black shift is in it as in the render.
    #[test]
    fn the_vignettes_response_is_the_masters_neutral() -> Result<()> {
        use crate::develop::quality::{Stage, render_stage};
        let levels = [0.004, 0.02, 0.1, 0.4, 1.5, 5.];
        let im = grays(&levels);
        // The response of `r` at scene values `xs`.
        let response = |r: &Recipe, xs: &[f32]| -> Vec<f32> {
            let r = r.resolved_hdr(&im.metadata).into_owned();
            let f = crate::develop::pipeline::hdr_neutral_response(
                crate::develop::pipeline::Source::new(&im),
                &r,
            )
            .expect("an HDR master");
            xs.iter().map(|x| f(*x)).collect()
        };
        let edits: [fn(&mut Recipe); 3] = [
            |_| {},
            |r| {
                r.contrast = 0.4;
                r.blacks = -0.3;
            },
            |r| r.whites = 0.3,
        ];
        for edit in edits {
            let mut r = hdr_recipe(&im);
            edit(&mut r);
            let scene = render_stage(&im, &r, Stage::SceneInput, Cancel::NEVER)?;
            let out = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
            let xs: Vec<f32> = (0..levels.len())
                .map(|k| {
                    crate::develop::scene_tone::luminance(
                        scene.pixels[4 * scene.width as usize + k * 8 + 4],
                    )
                })
                .collect();
            for (k, got) in response(&r, &xs).into_iter().enumerate() {
                let want = centre(&out, k);
                assert!(
                    (got - want).abs() <= 0.01 * want + 1e-5,
                    "{k}: {got} {want}"
                );
            }
        }
        // Positive Shadows moves the scene tone stage's black for every pixel; the
        // response follows it.
        let plain = response(&hdr_recipe(&im), &[0.002])[0];
        let mut lifted = hdr_recipe(&im);
        lifted.shadows = 0.7;
        let lifted = response(&lifted, &[0.002])[0];
        assert!((lifted / plain - 1.).abs() > 0.01, "{lifted} {plain}");
        Ok(())
    }

    /// Texture renders in the master, on the camera image before tone as in SDR, and
    /// Clarity up to +50 and Dehaze from −30 to +100 by the SDR operators, unclipped;
    /// negative Clarity, Clarity above +50 and Dehaze below −30 still refuse it.
    #[test]
    fn texture_clarity_and_dehaze_render_in_the_master() -> anyhow::Result<()> {
        let im = grays(&[0.2, 0.6, 1.5, 3.]);
        let mut r = hdr_recipe(&im);
        r.effects.texture = 0.5;
        render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.effects.clarity = 0.5;
        render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        for dehaze in [0.3, 1., -0.3] {
            r.effects.dehaze = dehaze;
            render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        }
        type Change = fn(&mut Recipe);
        let refused: [(Change, &str); 3] = [
            (|r| r.effects.clarity = 0.6, "Clarity beyond"),
            (|r| r.effects.clarity = -0.6, "Clarity beyond"),
            (|r| r.effects.dehaze = -0.4, "Dehaze below"),
        ];
        for (change, reason) in refused {
            let mut edit = r.clone();
            change(&mut edit);
            let error = render_hdr(&im, &edit.checked()?, None, 0, Cancel::NEVER).unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
        }
        Ok(())
    }

    /// Shadows renders the master as in SDR: positive Shadows lifts the dark end and
    /// leaves light above white nearly alone, as Camera Raw's does. Highlights, whose
    /// HDR rendering above white is not the SDR rule's, still refuses it.
    #[test]
    fn shadows_render_the_hdr_master_and_highlights_do_not() -> anyhow::Result<()> {
        let im = grays(&[0.01, 0.05, 4., 8.]);
        let mut r = hdr_recipe(&im);
        let plain = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        r.shadows = 0.5;
        let lifted = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)?;
        assert!(centre(&lifted, 0) > centre(&plain, 0) * 1.1);
        for i in 2..4 {
            assert!(
                (centre(&lifted, i) / centre(&plain, i)).log2().abs() < 0.1,
                "{i}"
            );
        }
        r.shadows = 0.;
        r.highlights = -0.5;
        let error = render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Highlights"), "{error}");
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

    /// The GPU port's SDR rendition has HDR off and leaves an HDR profile to the CPU;
    /// its master preview covers ordinary and HDR profiles and looks, up to the
    /// profile's finish, and an HDR file's master from its pseudo-scene.
    #[test]
    fn the_gpu_port_renders_masters_of_ordinary_profiles_only() {
        use crate::develop::pipeline::pixel_params::{supported, supported_hdr};
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        assert!(!supported(&r), "the SDR rendition's port has HDR off");
        assert!(supported_hdr(&r, &im.metadata));
        // Contrast, the tone curves, the colour stage, colour grading and Texture
        // render on the GPU.
        let on_the_gpu: [fn(&mut Recipe); 9] = [
            |r| r.grading[2] = [0.6, 0.4, 0.2],
            |r| r.contrast = 0.2,
            |r| r.white_point = 0.9,
            |r| r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]],
            |r| r.effects.channels[0].points = vec![[0., 0.], [0.5, 0.6], [1., 1.]],
            |r| r.effects.parametric[0] = 0.3,
            |r| r.saturation = 0.2,
            |r| r.effects.monochrome = true,
            |r| r.effects.texture = 0.3,
        ];
        for (i, set) in on_the_gpu.iter().enumerate() {
            let mut edited = r.clone();
            set(&mut edited);
            assert!(supported_hdr(&edited, &im.metadata), "GPU case {i}");
        }
        r.highlights = -0.4;
        assert!(
            !supported_hdr(&r, &im.metadata),
            "a master that doesn't render"
        );
        r.highlights = 0.;
        // A look's Clarity is in the resolved recipe's own, and counted once: +10 with
        // the user's +35 is +45, which renders on the GPU too.
        let mut look = r.clone();
        let mut profile = CameraProfile::creative_for_test(&im.metadata);
        profile.enhanced.as_mut().unwrap().clarity = 0.1;
        look.profile = Some(std::sync::Arc::new(profile));
        look.effects.clarity = 0.35;
        let resolved = look
            .checked()
            .unwrap()
            .resolved_hdr(&im.metadata)
            .into_owned();
        assert!(supported_hdr(&resolved, &im.metadata));
        r.hdr.enabled = false;
        assert!(supported(&r) && !supported_hdr(&r, &im.metadata));
        let hdr = r.profile.as_deref().unwrap().clone().as_hdr_for_test();
        r.profile = Some(std::sync::Arc::new(hdr));
        assert!(
            !supported(&r),
            "an HDR profile's SDR rendition renders on the CPU"
        );
        r.hdr.enabled = true;
        assert!(
            supported_hdr(&r, &im.metadata),
            "an HDR profile's master renders on the GPU"
        );
        let mut looked = hdr_recipe(&im);
        let profile = looked.profile.as_deref().unwrap().clone();
        looked.profile = Some(std::sync::Arc::new(profile.with_test_look(
            2,
            |_| 1.,
            &[[0., 0.], [1., 1.]],
        )));
        assert!(
            supported_hdr(&looked, &im.metadata),
            "a look's master renders on the GPU"
        );
        // An HDR file's master renders on the GPU from its pseudo-scene, with the
        // recipe the scene renders with; an SDR rendered file has no master.
        let file = hdr_file(2, 2, |_, _| [2.; 3]);
        let mut r = Recipe::for_new_rendered(&file.metadata);
        assert!(r.hdr.is_active());
        hdr_file_recipe(&mut r, &file.metadata);
        assert!(
            supported_hdr(&r, &file.metadata),
            "an HDR file's master renders on the GPU"
        );
        let mut sdr_file = file.metadata.clone();
        sdr_file.hdr_source = None;
        assert!(!supported_hdr(&r, &sdr_file), "an SDR file has no master");
    }

    /// A switched-off panel's adjustments don't refuse the master.
    #[test]
    fn a_switched_off_panel_does_not_refuse_the_master() -> anyhow::Result<()> {
        use crate::model::panels::{Panel, PanelState};
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        // A darkening vignette's Highlights, which the master doesn't render.
        r.effects.vignette = -0.3;
        r.effects.vignette_highlights = 0.5;
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
                &render_hdr_region(&im, &r, None, region, Cancel::NEVER)?,
                region,
            );
        }
        assert!(render_hdr_region(&im, &r, None, [40, 0, 9, 8], Cancel::NEVER).is_err());
        assert!(render_hdr_region(&im, &r, None, [0, 0, 0, 8], Cancel::NEVER).is_err());
        // A mask's Sharpness and Noise read the same neighbours in a region as in the
        // whole, without global sharpening too.
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        for adjust in [
            LocalAdjust {
                sharpness: -1.,
                ..Default::default()
            },
            LocalAdjust {
                noise: 1.,
                ..Default::default()
            },
            LocalAdjust {
                sharpness: 1.,
                noise: 1.,
                ..Default::default()
            },
        ] {
            let mut masked = hdr_recipe(&im);
            masked.masks = vec![MaskGroup {
                components: vec![MaskComponent {
                    invert: true,
                    ..MaskComponent::new(MaskShape::Linear {
                        from: [0., -3.],
                        to: [0., -2.9],
                    })
                }],
                adjust,
                ..Default::default()
            }];
            let masked = masked.checked()?;
            let whole = render_hdr(&im, &masked, None, 0, Cancel::NEVER)?;
            for region in [[8, 1, 9, 5], [31, 2, 17, 6]] {
                same(
                    &whole,
                    &render_hdr_region(&im, &masked, None, region, Cancel::NEVER)?,
                    region,
                );
            }
        }

        let file = hdr_file(6, 4, |x, y| [0.5 * x as f32, 0.1 * y as f32, 3.]);
        let r = Recipe::for_new_rendered(&file.metadata);
        let r = r.checked()?;
        let whole = render_hdr(&file, &r, None, 0, Cancel::NEVER)?;
        same(
            &whole,
            &render_hdr_region(&file, &r, None, [2, 1, 3, 2], Cancel::NEVER)?,
            [2, 1, 3, 2],
        );
        Ok(())
    }

    /// AI Denoise renders in the master as in the SDR rendition: the whole master, a
    /// 100% region of it and a reduced preview each show the AI source blended in by
    /// Amount, and a region is still that part of the whole.
    #[test]
    fn the_master_renders_ai_denoise() -> Result<()> {
        let im = grays(&[0.05, 0.2, 1.2, 4.064, 0.4, 2.]);
        let brighter = CameraImage {
            pixels: im.pixels.iter().map(|p| p.map(|v| v * 1.5)).collect(),
            ..im.clone()
        };
        let ai = crate::develop::AiSource::recover(brighter, Cancel::NEVER)?;
        let mut r = hdr_recipe(&im);
        r.ai_denoise.enabled = true;
        r.ai_denoise.amount = 1.;
        let r = r.checked()?;
        let plain = render_hdr(&im, &r, None, 0, Cancel::NEVER)?;
        let whole = render_hdr(&im, &r, Some(&ai), 0, Cancel::NEVER)?;
        // Column 1 (0.2) is below white in both; the source's is 1.5 times brighter.
        let ratio = centre(&whole, 1) / centre(&plain, 1);
        assert!(ratio > 1.2, "AI Denoise's source shows: {ratio}");
        let region = [8, 1, 17, 6];
        let part = render_hdr_region(&im, &r, Some(&ai), region, Cancel::NEVER)?;
        for row in 0..6 {
            for col in 0..17 {
                let a = whole.pixels[((1 + row) * whole.width + 8 + col) as usize];
                let b = part.pixels[(row * 17 + col) as usize];
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() <= 1e-4 * a[c].abs().max(1.));
                }
            }
        }
        // A preview reduced to 24 pixels reduces the source with the photo.
        let reduced = render_hdr_preview(&im, &r, Some(&ai), 12, Cancel::NEVER)?;
        let reduced_plain = render_hdr_preview(&im, &r, None, 12, Cancel::NEVER)?;
        assert_eq!((reduced.width, reduced.height), (12, 2));
        let luma = |o: &crate::rendered::HdrImage, x: usize| {
            crate::develop::scene_tone::luminance(o.pixels[o.width as usize + x])
        };
        let ratio = luma(&reduced, 2) / luma(&reduced_plain, 2);
        assert!(ratio > 1.2, "the reduced preview shows the source: {ratio}");
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
