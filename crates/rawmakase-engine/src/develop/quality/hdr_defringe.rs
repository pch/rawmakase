//! Defringe in the HDR master (docs/hdr.md#defringe), as Camera Raw 18.7's HDR
//! Defringe departs from its SDR one on photos (docs/records/hdr-stage1-results.md#defringe):
//! below white it changes colour along edges and leaves smooth and faint colour alone,
//! and above white it takes a band of bright, saturated blues.
//!
//! SDR's Oklab operator renders each pixel, weighted by how sharply the master's
//! luminance changes around it ([`EDGE_RADIUS`], [`EDGE_FULL`]) and by the colour's
//! chroma ([`CHROMA_FROM`], [`CHROMA_FULL`]); then the band of blues loses chroma
//! ([`blue_band`]). It runs on the CPU, after the colour stage, before sharpening, the
//! vignette and grain, as SDR's Defringe precedes them.
use crate::camera_data::CameraImage;
use crate::develop::effects::EffectsRendering;
use crate::model::recipe::Recipe;
use rayon::prelude::*;

/// Half width, in pixels of the full photo, of the window whose luminance range weighs
/// the change.
const EDGE_RADIUS: f32 = 4.;
/// Luminance range across that window, in stops, at which the change is full.
const EDGE_FULL: f32 = 1.;
/// Oklab chroma (of the colour with its brightest channel at most white) below which
/// the master's Defringe changes nothing, and from which it changes as SDR's.
const CHROMA_FROM: f32 = 0.015;
const CHROMA_FULL: f32 = 0.04;

/// The window's half width in pixels of a render of `im`: [`EDGE_RADIUS`] of the full
/// photo, smaller on a reduced copy.
pub(super) fn radius(im: &CameraImage) -> u32 {
    let full = im.metadata.width.max(im.metadata.height).max(1) as f32;
    let own = im.width.max(im.height) as f32;
    let scale = (own / full).min(1.);
    (EDGE_RADIUS * scale).round().max(1.) as u32
}

/// Defringes `pixels` (linear ProPhoto, 1 = graphics white, `width` wide) in place,
/// each by the luminance range within `radius` of it. Pixels within `radius` of the
/// slice's edge read only what the slice holds, so a region renders with that margin.
pub(super) fn apply(pixels: &mut [[f32; 3]], width: usize, r: &Recipe, radius: u32) {
    if r.effects.defringe == [0.; 2] || pixels.is_empty() {
        return;
    }
    let height = pixels.len() / width;
    let level: Vec<f32> = pixels
        .par_iter()
        .map(|p| {
            crate::develop::scene_tone::luminance(*p)
                .max(1. / 4096.)
                .log2()
        })
        .collect();
    let range = local_range(&level, width, height, radius as usize);
    let to_srgb = crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
    let to_prophoto = crate::color::inverse(to_srgb);
    let to_p3 = crate::color::hdr_output::HdrGamut::DisplayP3.prophoto_to_rgb();
    let blue = blue_band_share(r.effects.defringe[0]);
    pixels.par_iter_mut().zip(range).for_each(|(p, range)| {
        let rgb = crate::color::mul(to_srgb, *p);
        let edge = (range / EDGE_FULL).clamp(0., 1.);
        let mut out = rgb;
        if edge > 0. {
            let (defringed, chroma) = defringe_rgb(rgb, r);
            let k = edge * ((chroma - CHROMA_FROM) / (CHROMA_FULL - CHROMA_FROM)).clamp(0., 1.);
            out = std::array::from_fn(|c| rgb[c] + k * (defringed[c] - rgb[c]));
        }
        if blue > 0. {
            let w = blue_band(crate::color::mul(to_p3, *p), rgb);
            if w > 0. {
                out = scale_chroma(out, 1. - w * blue);
            }
        }
        if out != rgb {
            *p = crate::color::mul(to_prophoto, out);
        }
    });
}

/// Share of chroma the band of blues loses at Purple Amount `amount` (0–1): 0.5 at
/// Amount 8 of 20, where Camera Raw's took the Nikon Z 8's sky at 4.5–6.5 times white
/// to ×0.51–0.56; other Amounts follow the Amount curve of SDR's operator, which no
/// capture measured in HDR.
fn blue_band_share(amount: f32) -> f32 {
    let curve = |a: f32| 1. - (-a * 20. / 2.5).exp();
    (0.5 * curve(amount) / curve(0.4)).min(1.)
}

/// How much a colour (`p3`, linear Display P3, 1 = graphics white; `srgb`, the same in
/// sRGB primaries) is in the band of blues Camera Raw 18.7's HDR Defringe takes with
/// Purple Amount 8 (Hue 30–70): Oklab hue 0.63–0.675, its brightest Display P3 channel
/// 4.5–6.5 times white and its saturation there (1 − min/max) above 0.55, each softened
/// at its ends. Measured on two photos (the Nikon Z 8's sky and a Sony ARW's); the
/// twelve photos' other bright blues, dimmer or paler, keep their colour.
fn blue_band(p3: [f32; 3], srgb: [f32; 3]) -> f32 {
    let ramp = |x: f32, a: f32, b: f32| ((x - a) / (b - a)).clamp(0., 1.);
    let within = |x: f32, lo: f32, hi: f32, soft: f32| {
        ramp(x, lo - soft, lo + soft) * (1. - ramp(x, hi - soft, hi + soft))
    };
    let top = p3.into_iter().fold(0f32, f32::max);
    if top < 4. {
        return 0.;
    }
    let low = p3.into_iter().fold(f32::INFINITY, f32::min).max(0.);
    let saturation = 1. - low / top;
    // Oklab's hue is the same whatever the primaries it is computed from.
    let lab = crate::develop::pipeline::srgb_to_lab(srgb);
    let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    within(top, 4.5, 6.5, 0.5) * ramp(saturation, 0.5, 0.6) * within(hue, 0.63, 0.675, 0.01)
}

/// `rgb` (linear sRGB primaries) with its Oklab chroma scaled by `k`, its lightness
/// and hue kept; a colour above white is scaled to its brightest channel at white
/// first, as SDR's operator sees it, and back.
fn scale_chroma(rgb: [f32; 3], k: f32) -> [f32; 3] {
    let top = rgb.into_iter().fold(1f32, f32::max);
    let lab = crate::develop::pipeline::srgb_to_lab(rgb.map(|v| v / top));
    crate::develop::pipeline::lab_to_srgb([lab[0], lab[1] * k, lab[2] * k]).map(|v| v * top)
}

/// SDR's operator on a linear display colour (sRGB primaries, 1 = graphics white), and
/// the colour's Oklab chroma as it sees it: a colour brighter than white is taken with
/// its brightest channel at white and scaled back.
fn defringe_rgb(rgb: [f32; 3], r: &Recipe) -> ([f32; 3], f32) {
    let top = rgb.into_iter().fold(1f32, f32::max);
    let lab = crate::develop::pipeline::srgb_to_lab(rgb.map(|v| v / top));
    let chroma = lab[1].hypot(lab[2]);
    let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let lab = r
        .effects
        .defringe_color([lab[0].clamp(0., 1.), lab[1], lab[2]], hue);
    (
        crate::develop::pipeline::lab_to_srgb(lab).map(|v| v * top),
        chroma,
    )
}

/// Each pixel's range (max − min) of `v` over the square of half width `r` around it,
/// clipped to the image: separable running max and min.
fn local_range(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], max: bool, horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; src.len()];
        let (n, m) = if horizontal { (h, w) } else { (w, h) };
        let idx = |line: usize, i: usize| {
            if horizontal {
                line * w + i
            } else {
                i * w + line
            }
        };
        let lines: Vec<Vec<f32>> = (0..n)
            .into_par_iter()
            .map(|line| {
                (0..m)
                    .map(|i| {
                        let (a, b) = (i.saturating_sub(r), (i + r).min(m - 1));
                        let vals = (a..=b).map(|j| src[idx(line, j)]);
                        if max {
                            vals.fold(f32::MIN, f32::max)
                        } else {
                            vals.fold(f32::MAX, f32::min)
                        }
                    })
                    .collect()
            })
            .collect();
        for (line, vals) in lines.into_iter().enumerate() {
            for (i, v) in vals.into_iter().enumerate() {
                out[idx(line, i)] = v;
            }
        }
        out
    };
    let hi = pass(&pass(v, true, true), true, false);
    let lo = pass(&pass(v, false, true), false, false);
    hi.into_iter().zip(lo).map(|(a, b)| a - b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prophoto(srgb: [f32; 3]) -> [f32; 3] {
        let to_srgb = crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
        crate::color::mul(crate::color::inverse(to_srgb), srgb)
    }
    fn chroma(p: [f32; 3]) -> f32 {
        let to_srgb = crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
        let rgb = crate::color::mul(to_srgb, p);
        let top = rgb.into_iter().fold(1f32, f32::max);
        let lab = crate::develop::pipeline::srgb_to_lab(rgb.map(|v| v / top));
        lab[1].hypot(lab[2])
    }
    fn purple() -> Recipe {
        let mut r = Recipe::default();
        r.effects.defringe = [0.4, 0.];
        r
    }

    /// A fringe colour beside a sharp change of luminance loses chroma as SDR's
    /// operator removes it; the same colour away from the edge, and a faint one beside
    /// it, keep theirs.
    #[test]
    fn only_a_saturated_colour_beside_an_edge_loses_chroma() {
        let fringe = prophoto([0.25, 0.03, 0.3]);
        let faint = prophoto([0.2, 0.18, 0.205]);
        let lab = crate::develop::pipeline::srgb_to_lab([0.25, 0.03, 0.3]);
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        assert!((0.8..0.95).contains(&hue), "a purple fringe: {hue}");
        // A row of 40: 8 dark, then the colour.
        let w = 40usize;
        let row = |c: [f32; 3]| -> Vec<[f32; 3]> {
            (0..w).map(|x| if x < 8 { [0.004; 3] } else { c }).collect()
        };
        let kept = |c: [f32; 3], k: usize| {
            let mut pixels = row(c);
            apply(&mut pixels, w, &purple(), 4);
            chroma(pixels[k]) / chroma(c)
        };
        assert!(
            kept(fringe, 9) < 0.7,
            "beside the edge: {}",
            kept(fringe, 9)
        );
        assert!(
            (kept(fringe, 30) - 1.).abs() < 1e-4,
            "away from it: {}",
            kept(fringe, 30)
        );
        assert!(kept(faint, 9) > 0.999, "faint: {}", kept(faint, 9));
        let before = row(fringe);
        // Without Defringe nothing changes.
        let mut untouched = before.clone();
        apply(&mut untouched, w, &Recipe::default(), 4);
        assert_eq!(untouched, before);
    }

    /// A smooth bright, saturated blue 4.5–6.5 times white loses half its chroma at
    /// Purple Amount 8, as Camera Raw's HDR Defringe took the Nikon Z 8's sky; the same
    /// blue at twice white, or without Purple, keeps its colour.
    #[test]
    fn a_band_of_bright_saturated_blues_loses_chroma() {
        let to_p3 = crate::color::hdr_output::HdrGamut::DisplayP3.prophoto_to_rgb();
        let sky_p3 = [2., 3.65, 5.5];
        let sky = crate::color::mul(crate::color::inverse(to_p3), sky_p3);
        let srgb = crate::color::mul(
            crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb(),
            sky,
        );
        assert!(
            blue_band(sky_p3, srgb) > 0.99,
            "{}",
            blue_band(sky_p3, srgb)
        );
        let render = |p: [f32; 3], r: &Recipe| {
            let mut pixels = vec![p; 16 * 16];
            apply(&mut pixels, 16, r, 4);
            pixels[8 * 16 + 8]
        };
        let kept = chroma(render(sky, &purple())) / chroma(sky);
        assert!((kept - 0.5).abs() < 0.05, "{kept}");
        let dim = sky.map(|v| v / 2.75);
        assert!((chroma(render(dim, &purple())) / chroma(dim) - 1.).abs() < 1e-4);
        let mut green = Recipe::default();
        green.effects.defringe = [0., 0.4];
        assert!((chroma(render(sky, &green)) / chroma(sky) - 1.).abs() < 1e-4);
    }
}
