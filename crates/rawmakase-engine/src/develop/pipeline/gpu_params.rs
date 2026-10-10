//! Parameters the GPU port of the pipeline takes, built from the same recipe and image.
use super::*;

/// Parameters of the GPU per-pixel stage for `toned` and the resolved recipe `r`, or
/// `None` when the port does not cover it. The photo's measures and the
/// Shadows/Highlights map's base come from `cache` when the tone stage that made them is
/// unchanged, so the Basic sliders only rebuild their curves; otherwise the map's tone
/// pass over the reduced photo runs on the GPU and the map is built from its luminance.
pub(crate) fn gpu_pixel_params(
    toned: &Toned,
    r: &Recipe,
    cache: &mut crate::develop::stage_cache::StageCache,
    backend: &mut crate::develop::preview_renderer::Backend,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<pixel_params::PixelParams> {
    use crate::develop::stage_cache::ToneKey;
    let im = toned.source();
    // Measures only `r` needs, kept for the tone stage they were made with; a failure
    // (none is expected) measures again rather than rendering without them.
    let pivot = measures_contrast_pivot(r).then(|| {
        let key = ToneKey::without_exposure(toned, r);
        let made = cache
            .pivots
            .get_or_try(key, |_| 4, || Ok(measured_pivot(im, r)));
        made.map_or_else(|_| measured_pivot(im, r), |v| *v)
    });
    let highlights = measures_whites(r).then(|| {
        let key = ToneKey::new(toned, r);
        let made = cache
            .highlights
            .get_or_try(key, |_| 4, || Ok(measured_highlights(im, r)));
        made.map_or_else(|_| measured_highlights(im, r), |v| *v)
    });
    let measures = PhotoMeasures { pivot, highlights };
    // The final pass may keep its tone stage for its samples (see `PixelParams::tone`).
    let keep = |mut p: pixel_params::PixelParams| {
        p.tone = Some(crate::develop::stage_cache::tone_recipe(r));
        p
    };
    if !pixel_params::needs_map(r) {
        return pixel_params::measured_params(im, r, measures, false).map(keep);
    }
    let tone = pixel_params::measured_params(im, r, measures, true)?;
    let sliders = crate::develop::local_tone::Sliders::of(r);
    // The map's base is built and kept on the device, unless the measured Clarity
    // needs it on the CPU.
    if sliders.clarity == 0.
        && let Some(small) = im.reduced
    {
        let key = ToneKey::new(toned, r);
        let map = backend.run(cancel, |gpu| {
            gpu.scoped(|gpu| gpu.device_map(key, small, &tone))
        });
        if let Some(map) = map {
            let source = [im.width, im.height];
            let p = pixel_params::with_device_map(tone, map, source, sliders);
            return Some(keep(p));
        }
    }
    let base = cache.maps.get_or_try(
        ToneKey::new(toned, r),
        crate::develop::local_tone::MapBase::bytes,
        || {
            let small = match im.reduced {
                Some(small) => std::borrow::Cow::Borrowed(small),
                None => std::borrow::Cow::Owned(preview_source(
                    im,
                    crate::develop::local_tone::MAP_EDGE,
                )),
            };
            let toned = backend
                .run(cancel, |gpu| {
                    gpu.scoped(|gpu| gpu.develop_pixels(&small.pixels, &tone, cancel))
                })
                .ok_or_else(|| anyhow::anyhow!("GPU tone pass failed"))?;
            let lum = toned
                .into_iter()
                .map(crate::develop::local_tone::luminance)
                .collect();
            Ok(crate::develop::local_tone::MapBase::new(
                lum,
                [small.width, small.height],
                [im.width, im.height],
            ))
        },
    );
    let Ok(base) = base else {
        // As before the map was kept: the CPU builds what the GPU could not.
        return pixel_params::pixel_params(im, r);
    };
    let map = crate::develop::local_tone::LocalToneMap::from_base(&base, sliders);
    Some(keep(pixel_params::with_map(tone, &map)))
}
/// Lens correction for `gpu/local.wgsl`, from `S_LENS` to `S_VIGNETTING_AMOUNT`, with
/// radial tables appended to `tables` (each: knots, then values) at offsets counted
/// from `base`. Offsets are -1 for absent tables.
pub(crate) fn lens_gpu_params(
    im: &CameraImage,
    r: &Recipe,
    base: usize,
    tables: &mut Vec<f32>,
) -> [f32; 15] {
    let mut push = |radial: Option<&crate::optics::Radial>| match radial {
        Some(radial) => {
            let at = base + tables.len();
            tables.extend(&radial.knots);
            tables.extend(&radial.values);
            [at as f32, radial.knots.len() as f32]
        }
        None => [-1., 0.],
    };
    let mut out = [0.; 15];
    let Some(warp) = LensWarp::new(im, r) else {
        out[5..13].copy_from_slice(&[-1., 0., -1., 0., -1., 0., -1., 0.]);
        return out;
    };
    let lens = warp.map.lens;
    let distortion = push(lens.distortion.as_ref());
    let [red, blue] = match warp.map.chromatic.or(lens.chromatic.as_ref()) {
        Some([red, blue]) => [push(Some(red)), push(Some(blue))],
        None => [[-1., 0.]; 2],
    };
    let vignetting = push(warp.vignetting.as_ref().map(VignetteField::table));
    let m = &warp.map;
    // 2 marks a measured aberration, evaluated at the distorted radius.
    let mode = if m.chromatic.is_some() { 2. } else { 1. };
    out[..5].copy_from_slice(&[mode, m.center[0], m.center[1], m.half, m.fill]);
    out[5] = m.amount;
    out[6..8].copy_from_slice(&distortion);
    out[8..10].copy_from_slice(&red);
    out[10..12].copy_from_slice(&blue);
    out[12..14].copy_from_slice(&vignetting);
    out[14] = warp.vignetting.as_ref().map_or(0., |v| v.amount);
    out
}
/// The photo pixels (x, y, width, height) that sampling `region` of the output `g`
/// reads, found by mapping the region's edges through geometry and lens correction and
/// adding the bilinear, noise-reduction and footprint taps. Empty when none map inside.
pub(crate) fn source_bounds(
    im: &CameraImage,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
) -> [u32; 4] {
    let warp = LensWarp::new(im, r);
    let [x0, y0, w, h] = region;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut add = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        let points = match &warp {
            Some(warp) => warp.positions(sx, sy),
            None => [[sx, sy]; 3],
        };
        for p in points {
            for c in 0..2 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
    };
    for x in x0..x0 + w {
        add(x, y0);
        add(x, y0 + h - 1);
    }
    for y in y0..y0 + h {
        add(x0, y);
        add(x0 + w - 1, y);
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return [0; 4];
    }
    let pad = 2. + spread.ceil();
    let size = [im.width, im.height];
    let [a, b] = std::array::from_fn(|c| {
        let top = (size[c] - 1) as f32;
        (
            (lo[c] - pad).clamp(0., top).floor() as u32,
            (hi[c] + pad).clamp(0., top).ceil() as u32,
        )
    });
    [a.0, b.0, a.1 - a.0 + 1, b.1 - b.0 + 1]
}
/// Vignetting (lens profile and manual) for `gpu/logs.wgsl`: centre, half diagonal, amount and the
/// radial table (knots, then values), or `None`.
pub(crate) fn vignetting_gpu_params(im: &CameraImage, r: &Recipe) -> Option<([f32; 4], Vec<f32>)> {
    let v = VignetteField::new(im, r)?;
    let radial = v.table();
    let mut table = radial.knots.clone();
    table.extend(&radial.values);
    Some(([v.center[0], v.center[1], v.half, v.amount], table))
}
