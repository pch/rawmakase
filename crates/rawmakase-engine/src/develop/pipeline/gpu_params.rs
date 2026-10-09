//! Parameters the GPU port of the pipeline takes, built from the same recipe and image.
use super::*;

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
