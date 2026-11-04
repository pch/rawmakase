//! Patch averages, CIELAB and CIEDE2000.
use super::chart::{Patch, SRGB_TO_XYZ, apply, srgb_to_linear};

/// Mean encoded-sRGB value of each patch area, as 16-bit integers.
pub(crate) fn patches(width: u32, pixels: &[[f32; 3]], patches: &[Patch]) -> Vec<[u16; 3]> {
    patches
        .iter()
        .map(|p| {
            let mut sum = [0f64; 3];
            for y in p.y..p.y + p.h {
                for x in p.x..p.x + p.w {
                    let v = pixels[(y * width + x) as usize];
                    for c in 0..3 {
                        sum[c] += v[c] as f64;
                    }
                }
            }
            let n = (p.w * p.h) as f64;
            sum.map(|s| ((s / n).clamp(0., 1.) * 65535.).round() as u16)
        })
        .collect()
}

/// CIELAB relative to D65, from encoded sRGB.
pub(crate) fn lab(rgb: [u16; 3]) -> [f64; 3] {
    let lin = rgb.map(|v| srgb_to_linear(v as f64 / 65535.));
    let xyz = apply(&SRGB_TO_XYZ, lin);
    let white = [0.950_47, 1., 1.088_83];
    let f = |t: f64| {
        if t > 216. / 24389. {
            t.cbrt()
        } else {
            (24389. / 27. * t + 16.) / 116.
        }
    };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / white[i]));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

pub(crate) fn chroma(lab: [f64; 3]) -> f64 {
    lab[1].hypot(lab[2])
}

/// Hue angle difference in degrees, in −180..180.
pub(crate) fn hue_difference(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = b[2].atan2(b[1]).to_degrees() - a[2].atan2(a[1]).to_degrees();
    (d + 540.) % 360. - 180.
}

/// CIEDE2000 colour difference (Sharma, Wu and Dalal 2005).
pub(crate) fn delta_e2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.;
    let g = 0.5 * (1. - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1. + g) * a1, (1. + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0. && b == 0. {
            0.
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0. {
        0.
    } else if (h2p - h1p).abs() <= 180. {
        h2p - h1p
    } else if h2p - h1p > 180. {
        h2p - h1p - 360.
    } else {
        h2p - h1p + 360.
    };
    let dhh = 2. * (c1p * c2p).sqrt() * (dh / 2.).to_radians().sin();
    let l_bar = (l1 + l2) / 2.;
    let cp_bar = (c1p + c2p) / 2.;
    let hp_bar = if c1p * c2p == 0. {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180. {
        (h1p + h2p) / 2.
    } else if h1p + h2p < 360. {
        (h1p + h2p + 360.) / 2.
    } else {
        (h1p + h2p - 360.) / 2.
    };
    let t = 1. - 0.17 * (hp_bar - 30.).to_radians().cos()
        + 0.24 * (2. * hp_bar).to_radians().cos()
        + 0.32 * (3. * hp_bar + 6.).to_radians().cos()
        - 0.20 * (4. * hp_bar - 63.).to_radians().cos();
    let d_theta = 30. * (-((hp_bar - 275.) / 25.).powi(2)).exp();
    let rc = 2. * (cp_bar.powi(7) / (cp_bar.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1. + 0.015 * (l_bar - 50.).powi(2) / (20. + (l_bar - 50.).powi(2)).sqrt();
    let sc = 1. + 0.045 * cp_bar;
    let sh = 1. + 0.015 * cp_bar * t;
    let rt = -(2. * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

#[test]
fn ciede2000_matches_published_pairs() {
    // Sharma, Wu and Dalal (2005), Table 1: pairs 1, 7, 17 and 25.
    for (a, b, expected) in [
        ([50., 2.6772, -79.7751], [50., 0., -82.7485], 2.0425),
        ([50., 0., 0.], [50., -1., 2.], 2.3669),
        ([50., 2.5, 0.], [73., 25., -18.], 27.1492),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
    ] {
        let d = delta_e2000(a, b);
        assert!((d - expected).abs() < 1e-4, "{d} vs {expected}");
    }
}
