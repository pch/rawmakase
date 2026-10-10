//! The pickers that read a colour from the photo: the fringe color selector and the white-balance neutral.
use super::*;
use crate::develop::effects::EffectsRendering;

/// Lightroom's Fringe Color Selector on the shown colour `rgb` (encoded sRGB): the
/// Purple or Green range is pointed at it (see [`Effects::pick_fringe_hue`]). The
/// picked colour is the one the end of the colour stage, after Defringe, renders
/// closest to the shown colour.
///
/// [`Effects::pick_fringe_hue`]: crate::model::effects::Effects::pick_fringe_hue
pub fn pick_fringe(r: &mut Recipe, rgb: [f32; 3]) -> Option<usize> {
    let hue_of = |lab: [f32; 3]| {
        lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
    };
    let lab = srgb_to_lab(rgb.map(crate::color::srgb_decode));
    let (shown, chroma) = (hue_of(lab), lab[1].hypot(lab[2]));
    // The shown colour of one Defringe sees at lightness `l`, hue `h` and chroma `c`.
    let rendered = |[l, h, c]: [f32; 3]| {
        let angle = h * std::f32::consts::TAU;
        let out = finish_color([l, angle.cos() * c, angle.sin() * c]);
        srgb_to_lab(out.map(crate::color::srgb_decode))
    };
    let miss = |p: [f32; 3]| {
        let q = rendered(p);
        (0..3).map(|i| (q[i] - lab[i]).powi(2)).sum::<f32>()
    };
    let best = |candidates: &mut dyn Iterator<Item = [f32; 3]>| {
        candidates.min_by(|a, b| miss(*a).total_cmp(&miss(*b)))
    };
    // A coarse search over lightness, hue and chroma, then a finer one around the best.
    let coarse = [1. / 20., 1. / 120., 1. / 50.];
    let [l, h, c] = best(&mut (0..=20).flat_map(|i| {
        (0..120).flat_map(move |j| {
            (0..=20).map(move |k| {
                [
                    i as f32 * coarse[0],
                    j as f32 * coarse[1],
                    k as f32 * coarse[2],
                ]
            })
        })
    }))
    .unwrap_or([lab[0], shown, chroma]);
    let fine = |i: i32, step: f32| i as f32 * step / 5.;
    let [_, hue, chroma] = best(&mut (-5..=5).flat_map(|i| {
        (-5..=5).flat_map(move |j| {
            (-5..=5).map(move |k| {
                [
                    (l + fine(i, coarse[0])).clamp(0., 1.),
                    (h + fine(j, coarse[1])).rem_euclid(1.),
                    (c + fine(k, coarse[2])).max(0.),
                ]
            })
        })
    }))
    .unwrap_or([l, h, c]);
    r.effects.pick_fringe_hue(hue, chroma)
}
pub fn neutral_pick(im: &CameraImage, r: &Recipe, u: f32, v: f32) -> [f32; 3] {
    let g = Geometry::new(im, r, 0);
    let [x, y] = g.source(u, v);
    let mut sum = [0.; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let p = sample(im.into(), x + dx as f32, y + dy as f32);
            for c in 0..3 {
                sum[c] += p[c];
            }
        }
    }
    std::array::from_fn(|c| (sum[1] / sum[c].max(1e-6)).clamp(0.01, 100.))
}
