//! Color Range and Luminance Range components: weights from the developed color of
//! each pixel, before local adjustments, as Lightroom's range masks select.
use crate::color::srgb_decode;
use crate::rendered::Rendered;

/// Developed display pixels (sRGB-encoded, 0–1) aligned with the region the weights
/// are made for.
pub(crate) type RangeInput<'a> = &'a Rendered;

/// Oklab of a display pixel.
pub fn oklab(p: [f32; 3]) -> [f32; 3] {
    let l = p.map(|v| srgb_decode(v.clamp(0., 1.)));
    let m = |r: [f32; 3], v: [f32; 3]| r[0] * v[0] + r[1] * v[1] + r[2] * v[2];
    let lms = [
        m([0.41222146, 0.53633255, 0.051445995], l).cbrt(),
        m([0.2119035, 0.6806995, 0.10739696], l).cbrt(),
        m([0.08830246, 0.28171885, 0.6299787], l).cbrt(),
    ];
    [
        m([0.21045426, 0.7936178, -0.004072047], lms),
        m([1.9779985, -2.4285922, 0.4505937], lms),
        m([0.025904037, 0.78277177, -0.80867577], lms),
    ]
}
/// How close `lab` is to any sample: 1 near a sample, fading to 0. Chromaticity
/// (Oklab a and b relative to lightness, which stay put when a color is darker)
/// counts most and lightness a little, so a color selects in shadow and light alike.
/// `amount` is Lightroom's Refine (0–1): higher selects a wider range.
pub(crate) fn color_weight(samples: &[[f32; 3]], amount: f32, lab: [f32; 3]) -> f32 {
    let tolerance = 0.04 + amount * 0.3;
    let chromaticity = |p: [f32; 3]| {
        let l = p[0].max(0.05);
        [p[1] / l, p[2] / l]
    };
    let c = chromaticity(lab);
    samples
        .iter()
        .map(|s| {
            let t = chromaticity(*s);
            let d = ((c[0] - t[0]).powi(2) + (c[1] - t[1]).powi(2) + 0.2 * (lab[0] - s[0]).powi(2))
                .sqrt();
            1. - smoothstep(tolerance * 0.4, tolerance, d)
        })
        .fold(0., f32::max)
}
/// Lightness in `[low, high]`, fading linearly over `falloff` outside it.
pub(crate) fn luminance_weight(low: f32, high: f32, falloff: [f32; 2], lab: [f32; 3]) -> f32 {
    let l = lab[0].clamp(0., 1.);
    if l < low {
        if falloff[0] <= 0. {
            0.
        } else {
            (1. - (low - l) / falloff[0]).max(0.)
        }
    } else if l > high {
        if falloff[1] <= 0. {
            0.
        } else {
            (1. - (l - high) / falloff[1]).max(0.)
        }
    } else {
        1.
    }
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_select_near_colors_and_lightness() {
        let red = oklab([0.8, 0.2, 0.2]);
        let blue = oklab([0.2, 0.3, 0.8]);
        assert_eq!(color_weight(&[red], 0.5, red), 1.);
        assert_eq!(color_weight(&[red], 0.5, blue), 0.);
        // A darker red still counts; a wider Refine selects more.
        let dark = oklab([0.5, 0.12, 0.12]);
        assert!(color_weight(&[red], 0.5, dark) > color_weight(&[red], 0.1, dark));
        let gray = oklab([0.5; 3]);
        assert_eq!(luminance_weight(0.4, 0.8, [0., 0.], gray), 1.);
        assert_eq!(luminance_weight(0.7, 0.8, [0., 0.], gray), 0.);
        let w = luminance_weight(0.7, 0.8, [0.2, 0.], gray);
        assert!(w > 0. && w < 1.);
    }
}
