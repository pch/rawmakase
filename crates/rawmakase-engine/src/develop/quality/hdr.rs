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
    use crate::color::{srgb_decode, srgb_encode};
    ensure!(r.hdr.is_active(), "This edit has HDR off");
    crate::model::hdr::check_for(r, &im.metadata)?;
    if im.metadata.hdr_source.is_some() {
        return render_hdr_light(im, r, max_edge, cancel);
    }
    let recipe = r.resolved_hdr(&im.metadata);
    ensure!(
        recipe
            .profile
            .as_ref()
            .is_some_and(|p| p.hdr_tone().is_ok()),
        "This photo's camera profile does not render in HDR"
    );
    let geometry = Geometry::new(im, &recipe, 0);
    let resolved = Resolved { recipe, geometry };
    let source = resolved.source(im, ai, cancel, None)?;
    let r = resolved.recipe.as_ref();
    let g = resolved.geometry;
    let (toned, tonal_recipe) = local_stage(&source.image, &source, r, 1., cancel, None)?;
    let srgb = crate::color::OutputSpace::Srgb;
    let (mut out, _) = develop::render_base(
        &toned,
        &tonal_recipe,
        &g,
        [0, 0, g.width, g.height],
        0.,
        cancel,
        None,
        srgb,
    )?;
    check_cancel(cancel)?;
    let peak = r.hdr.peak();
    let encode = |p: &mut [f32; 3]| *p = p.map(|v| srgb_encode(v.max(0.)));
    let resized =
        if r.sharpening > 0. || output_size(g.width, g.height, max_edge) != (g.width, g.height) {
            out.pixels.par_iter_mut().for_each(encode);
            sharpen_unbounded(
                &mut out,
                r,
                Sharpener::new(r).sigma,
                crate::color::OutputSpace::ProPhoto,
                cancel,
            )?;
            let mut out = resize_unbounded(out, max_edge);
            out.pixels
                .par_iter_mut()
                .for_each(|p| *p = p.map(|v| srgb_decode(v.max(0.))));
            out
        } else {
            out
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

/// The HDR master of an HDR file (docs/records/hdr-stage4-design.md): its own light,
/// which no profile, white balance or tone stage applies to, through Exposure (a gain),
/// the geometry its SDR rendition renders with, a resize in linear light, and the
/// limit. `im` is the decoded photo or its preview reduction, which hold the light;
/// a working image doesn't, and a fact in its metadata alone renders nothing.
/// Resizing clamps negative light to 0; at full size it stays signed.
fn render_hdr_light(
    im: &CameraImage,
    r: &ValidRecipe,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<crate::rendered::HdrImage> {
    use anyhow::Context;
    let light = im.hdr().context("This HDR file's light was not decoded")?;
    let recipe = r.resolved_hdr(&im.metadata);
    let source = CameraImage {
        width: light.width(),
        height: light.height(),
        recovered: Default::default(),
        pixels: light.pixels().to_vec(),
        metadata: im.metadata.clone(),
        fast: im.fast,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let g = Geometry::new(&source, &recipe, 0);
    let samples = crate::develop::pipeline::sample_region(
        (&source).into(),
        &recipe,
        &g,
        [0, 0, g.width, g.height],
        0.,
        cancel,
    )?;
    check_cancel(cancel)?;
    let gain = recipe.exposure.exp2();
    let out = crate::rendered::Rendered {
        width: samples.width,
        height: samples.height,
        pixels: samples
            .pixels
            .into_iter()
            .map(|p| p.map(|v| v * gain))
            .collect(),
    };
    let resized = resize_unbounded(out, max_edge);
    let peak = recipe.hdr.peak();
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

    /// An HDR file's master at its own settings is its light: above white, signed and
    /// coloured alike. Exposure is a gain; Rotate turns it as the SDR rendition turns.
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
            let want = light(i as u32 % 6, i as u32 / 6);
            assert!(
                (0..3).all(|c| (p[c] - want[c]).abs() < 1e-5),
                "{i}: {p:?} against {want:?}"
            );
        }
        let mut brighter = r.clone();
        brighter.exposure = 1.;
        brighter.hdr.limit = 4.;
        let out = render_hdr(&im, &brighter.checked()?, None, 0, Cancel::NEVER)?;
        assert!((out.pixels[0][0] - 12.).abs() < 1e-4, "{:?}", out.pixels[0]);
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
        let (w, h) = (levels.len() as u32 * 8, 8);
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
            pixels: (0..w * h)
                .map(|i| [levels[(i % w / 8) as usize]; 3])
                .collect(),
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
        r.profile = Some(std::sync::Arc::new(CameraProfile::creative_for_test(
            &im.metadata,
        )));
        assert!(master(&r).is_err());
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
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
        r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
        assert!(render_hdr(&im, &r.checked()?, None, 0, Cancel::NEVER).is_err());
        r.panels.set(Panel::ToneCurve, PanelState::Off);
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

    #[test]
    fn an_sdr_edit_has_no_hdr_master() {
        let im = grays(&[0.5]);
        let mut r = hdr_recipe(&im);
        r.hdr.enabled = false;
        assert!(render_hdr(&im, &r.checked().unwrap(), None, 0, Cancel::NEVER).is_err());
    }
}
