//! Colour difference: CIELAB of linear sRGB, and CIEDE2000 between two colours.
//! Camera Look's fit and the half-size checks measure with it.

/// CIELAB (D65) of linear sRGB.
pub(crate) fn lab([r, g, b]: [f64; 3]) -> [f64; 3] {
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216. / 24389. {
            t.cbrt()
        } else {
            (24389. / 27. * t + 16.) / 116.
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciede2000_matches_published_pairs() {
        // Sharma, Wu and Dalal's test data, pairs 1, 7 and 17.
        for (a, b, want) in [
            ([50., 2.6772, -79.7751], [50., 0., -82.7485], 2.0425),
            ([50., 0., 0.], [50., -1., 2.], 2.3669),
            ([50., 2.5, 0.], [73., 25., -18.], 27.1492),
        ] {
            let got = delta_e2000(a, b);
            assert!((got - want).abs() < 1e-4, "{got} for {want}");
        }
    }
}
