//! DNG temperature/tint coordinates (Robertson reciprocal-temperature table).
//! Adapted from Adobe DNG SDK dng_temperature.cpp, Copyright 2006-2019 Adobe
//! Systems Incorporated. See licenses/Adobe-DNG-SDK.txt.
const TABLE: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.3047],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.1874, 0.28668, -0.40955],
    [90.0, 0.1888, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.2401, 0.34308, -2.0637],
    [325.0, 0.24702, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.264, 0.352, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.3132, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.77],
    [600.0, 0.33724, 0.36051, -116.45],
];

pub(crate) fn xy(temperature: f32, tint: f32) -> [f32; 2] {
    xy_at(1e6 / f64::from(temperature.clamp(2000., 50000.)), tint)
}

/// [`xy`] at reciprocal temperature `mired` (0–600; 0 is infinitely hot), without
/// [`xy`]'s temperature clamp: a rendered file's Temp reaches past 50 000 K.
pub(crate) fn xy_reciprocal(mired: f32, tint: f32) -> [f32; 2] {
    xy_at(f64::from(mired.clamp(0., 600.)), tint)
}

fn xy_at(reciprocal: f64, tint: f32) -> [f32; 2] {
    let i = (0..30)
        .find(|&i| reciprocal < TABLE[i + 1][0])
        .unwrap_or(29);
    let a = TABLE[i];
    let b = TABLE[i + 1];
    let f = (b[0] - reciprocal) / (b[0] - a[0]);
    let direction = |slope: f64| {
        let length = (1. + slope * slope).sqrt();
        [1. / length, slope / length]
    };
    let d1 = direction(a[3]);
    let d2 = direction(b[3]);
    let du = d1[0] * f + d2[0] * (1. - f);
    let dv = d1[1] * f + d2[1] * (1. - f);
    let length = du.hypot(dv);
    let offset = f64::from(tint) / -3000.;
    let u = a[1] * f + b[1] * (1. - f) + offset * du / length;
    let v = a[2] * f + b[2] * (1. - f) + offset * dv / length;
    let denominator = u - 4. * v + 2.;
    [(1.5 * u / denominator) as f32, (v / denominator) as f32]
}

pub(crate) fn from_xy(xy: [f32; 2]) -> [f32; 2] {
    let [x, y] = xy.map(f64::from);
    let u = 2. * x / (1.5 - x + 6. * y);
    let v = 3. * y / (1.5 - x + 6. * y);
    let mut previous = [0.; 3];
    for i in 1..31 {
        let a = TABLE[i - 1];
        let b = TABLE[i];
        let length = (1. + b[3] * b[3]).sqrt();
        let du = 1. / length;
        let dv = b[3] / length;
        let distance = -(u - b[1]) * dv + (v - b[2]) * du;
        if distance <= 0. || i == 30 {
            let distance = -distance.min(0.);
            let f = if i == 1 {
                0.
            } else {
                distance / (previous[0] + distance)
            };
            let reciprocal = a[0] * f + b[0] * (1. - f);
            let uu = u - (a[1] * f + b[1] * (1. - f));
            let vv = v - (a[2] * f + b[2] * (1. - f));
            let du = du * (1. - f) + previous[1] * f;
            let dv = dv * (1. - f) + previous[2] * f;
            let tint = -3000. * (uu * du + vv * dv) / du.hypot(dv);
            return [(1e6 / reciprocal.max(1e-6)) as f32, tint as f32];
        }
        previous = [distance, du, dv];
    }
    [6500., 0.]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daylight_and_temperature_tint_roundtrip() {
        let d65 = from_xy([0.3127, 0.3290]);
        assert!((d65[0] - 6504.).abs() < 10.);
        assert!((d65[1] - 9.8).abs() < 0.5);
        for t in [
            2000., 2856., 3200., 5050., 6500., 10000., 15000., 30000., 50000.,
        ] {
            for tint in [-150., -50., 0., 21., 50., 150.] {
                let result = from_xy(xy(t, tint));
                assert!((result[0] - t).abs() / t < 0.002, "{t} {tint}: {result:?}");
                assert!((result[1] - tint).abs() < 0.1);
            }
        }
    }
}

#[cfg(test)]
mod reciprocal_tests {
    use super::*;

    /// `xy` is `xy_reciprocal` at 1e6 / temperature, to the bit, inside its clamp.
    #[test]
    fn xy_is_xy_reciprocal_by_temperature() {
        for (temperature, tint) in [
            (2000., 0.),
            (2856., 5.),
            (5000., 0.),
            (6504., -12.),
            (50000., 30.),
        ] {
            let via_kelvin = xy(temperature, tint);
            let via_mired = xy_reciprocal((1e6 / f64::from(temperature)) as f32, tint);
            assert!(
                (via_kelvin[0] - via_mired[0]).abs() < 1e-6
                    && (via_kelvin[1] - via_mired[1]).abs() < 1e-6,
                "{temperature} K: {via_kelvin:?} vs {via_mired:?}"
            );
        }
    }

    /// Past 50 000 K (under 20 mired) the locus goes on to infinite temperature.
    #[test]
    fn xy_reciprocal_reaches_infinite_temperature() {
        let [x0, y0] = xy_reciprocal(0., 0.);
        let [x20, y20] = xy_reciprocal(20., 0.);
        assert!(x0 < x20 && y0 < y20, "{x0},{y0} vs {x20},{y20}");
        assert_eq!(xy_reciprocal(-5., 0.), xy_reciprocal(0., 0.));
    }
}
