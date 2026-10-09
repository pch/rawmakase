//! Additional photographic controls used by imported XMP recipes.
use crate::model::effects::{Effects, VignetteStyle};
use crate::model::recipe::Recipe;
use crate::rendered::Rendered;
use rayon::prelude::*;
mod grain;
mod lens_vignette;
mod vignette;
pub(crate) use grain::GrainField;
pub(crate) use lens_vignette::{ManualVignette, combined_table};
pub(crate) use vignette::PostCropVignette;
/// How the renderer applies a recipe's Effects, Calibration and Defringe settings, and the
/// Fringe Color Selector.
pub trait EffectsRendering {
    /// How strongly Highlights protects bright pixels. As in Lightroom, it applies only to
    /// Highlight Priority and Color Priority, and only when the vignette darkens.
    fn vignette_highlight_protection(&self) -> f32;
    /// The parametric curve's region (0 Shadows, 1 Darks, 2 Lights, 3 Highlights) an
    /// input `x` falls in, between the split points.
    fn parametric_region(&self, x: f32) -> usize;
    /// As [`Self::pick_fringe_hue`], for the fringe colour `rgb` (encoded sRGB, as
    /// shown).
    #[cfg(test)]
    fn pick_fringe(&mut self, rgb: [f32; 3]) -> Option<usize>;
    /// Lightroom's Fringe Color Selector: points the Purple or Green hue range at the
    /// fringe colour of Oklab `hue` and `chroma`, as Defringe sees it, and turns that
    /// Amount on if it is off. Returns which (0 purple, 1 green), or `None` when the
    /// colour is neither.
    fn pick_fringe_hue(&mut self, hue: f32, chroma: f32) -> Option<usize>;
    /// Lightroom's Defringe: chroma of hues inside the Purple and Green ranges is
    /// reduced, the more the higher Amount and the chroma (see
    /// docs/lens-corrections.md).
    fn defringe_color(&self, lab: [f32; 3], h: f32) -> [f32; 3];
}
impl EffectsRendering for Effects {
    fn vignette_highlight_protection(&self) -> f32 {
        match self.vignette_style {
            VignetteStyle::HighlightPriority | VignetteStyle::ColorPriority
                if self.vignette < 0. =>
            {
                self.vignette_highlights
            }
            _ => 0.,
        }
    }
    fn parametric_region(&self, x: f32) -> usize {
        self.splits.iter().filter(|s| x > **s).count()
    }
    #[cfg(test)]
    fn pick_fringe(&mut self, rgb: [f32; 3]) -> Option<usize> {
        let lab = crate::develop::pipeline::srgb_to_lab(rgb.map(crate::color::srgb_decode));
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        self.pick_fringe_hue(hue, lab[1].hypot(lab[2]))
    }
    fn pick_fringe_hue(&mut self, hue: f32, chroma: f32) -> Option<usize> {
        if chroma < 0.02 {
            return None;
        }
        // The nearer window that holds the hue well inside its slider: the outer ends
        // reach reds, yellows and blues, which are not fringes.
        let (i, at) = DEFRINGE_CENTERS
            .into_iter()
            .map(|center| ((hue - center + 0.5).rem_euclid(1.) - 0.5) / DEFRINGE_WINDOW + 0.5)
            .enumerate()
            .filter(|(_, at)| (0.15..=0.85).contains(at))
            .min_by(|a, b| (a.1 - 0.5).abs().total_cmp(&(b.1 - 0.5).abs()))?;
        // A range 20 wide on Lightroom's 0–100 hue sliders around the picked colour.
        let lo = (at - 0.1).clamp(0., 0.8);
        self.defringe_ranges[i] = [lo, lo + 0.2];
        if self.defringe[i] == 0. {
            // Lightroom's Amount 5 of 20.
            self.defringe[i] = 0.25;
        }
        Some(i)
    }
    fn defringe_color(&self, mut lab: [f32; 3], h: f32) -> [f32; 3] {
        for (i, center) in DEFRINGE_CENTERS.into_iter().enumerate() {
            if self.defringe[i] == 0. {
                continue;
            }
            let chroma = lab[1].hypot(lab[2]);
            let k = 1.
                - defringe_weight(h, center, self.defringe_ranges[i])
                    * defringe_strength(self.defringe[i], chroma);
            lab[1] *= k;
            lab[2] *= k;
        }
        lab
    }
}
/// Centres of the Purple and Green hue windows (Oklab hue, 0–1) and the hue span of
/// each Hue slider's 0–100, fitted to Camera Raw 18.6 renders.
const DEFRINGE_CENTERS: [f32; 2] = [0.875, 0.46];
const DEFRINGE_WINDOW: f32 = 0.5;
/// Half width of the soft edge of a hue range.
const DEFRINGE_SOFT: f32 = 0.025;
/// Share of chroma kept at full strength for a grey-ish colour, falling as chroma
/// rises (Oklab chroma scale), and the Amount (0–20) over which strength builds up.
const DEFRINGE_KEEP: f32 = 0.45;
const DEFRINGE_CHROMA: f32 = 0.09;
const DEFRINGE_RATE: f32 = 2.5;
/// How much of hue `h` the range `[lo, hi]` (0–1 of the Hue slider) of the window
/// centred at `center` selects.
fn defringe_weight(h: f32, center: f32, [lo, hi]: [f32; 2]) -> f32 {
    let d = (h - center + 0.5).rem_euclid(1.) - 0.5;
    let step = |x: f32| {
        let t = ((x + DEFRINGE_SOFT) / (2. * DEFRINGE_SOFT)).clamp(0., 1.);
        t * t * (3. - 2. * t)
    };
    step(d - (lo - 0.5) * DEFRINGE_WINDOW) * step((hi - 0.5) * DEFRINGE_WINDOW - d)
}
/// Share of chroma removed at full weight for Amount `amount` (0–1): Camera Raw
/// removes more of a stronger fringe colour.
fn defringe_strength(amount: f32, chroma: f32) -> f32 {
    (1. - DEFRINGE_KEEP * (-chroma / DEFRINGE_CHROMA).exp())
        * (1. - (-amount * 20. / DEFRINGE_RATE).exp())
}
pub fn spatial_finish(im: &mut Rendered, r: &Recipe, origin: [u32; 2], full: [u32; 2]) {
    spatial_finish_scaled(im, r, origin, full, 1.);
}
/// Vignettes and grain for an output with `scale` pixels per full-resolution pixel.
/// Grain keeps its full-resolution pattern and, like the full render resized, loses
/// amplitude where a preview pixel averages several grains.
pub(crate) fn spatial_finish_scaled(
    im: &mut Rendered,
    r: &Recipe,
    origin: [u32; 2],
    full: [u32; 2],
    scale: f32,
) {
    let e = &r.effects;
    if e.grain == 0. && e.vignette == 0. {
        return;
    }
    let vignette = PostCropVignette::new(e, full);
    // `full` is the whole output, `scale` its pixels per full-resolution pixel.
    let edge = full[0].max(full[1]) as f32 / scale;
    let grain = GrainField::new(e, edge);
    im.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        let x = origin[0] + i as u32 % im.width;
        let y = origin[1] + i as u32 / im.width;
        let nx = ((x as f32 + 0.5) / full[0] as f32 - 0.5) * 2.;
        let ny = ((y as f32 + 0.5) / full[1] as f32 - 0.5) * 2.;
        let l = crate::color::luminance(*p);
        if let Some(v) = &vignette {
            *p = v.apply(*p, v.mask(nx, ny));
        }
        let (gx, gy) = if scale == 1. {
            (x as f32, y as f32)
        } else {
            (
                (x as f32 + 0.5) / scale - 0.5,
                (y as f32 + 0.5) / scale - 0.5,
            )
        };
        let noise = grain.noise(gx, gy, scale, l);
        for v in p {
            *v = (*v + noise).clamp(0., 1.);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vignette_styles_follow_lightroom_codes_and_old_recipes_keep_theirs() {
        use VignetteStyle::*;
        for (code, style) in [
            (0, HighlightPriority),
            (1, HighlightPriority),
            (2, ColorPriority),
            (3, PaintOverlay),
        ] {
            assert_eq!(VignetteStyle::try_from(code).unwrap(), style);
        }
        assert!(VignetteStyle::try_from(4).is_err());
        let old: Effects = serde_json::from_str(r#"{"vignette_style": 0}"#).unwrap();
        assert_eq!(old.vignette_style, HighlightPriority);
        let old: Effects = serde_json::from_str(r#"{"vignette_style": 2}"#).unwrap();
        assert_eq!(old.vignette_style, ColorPriority);
        let paint = Effects {
            vignette_style: PaintOverlay,
            ..Default::default()
        };
        let json = serde_json::to_string(&paint).unwrap();
        assert!(json.contains(r#""vignette_style":3"#), "{json}");
        assert_eq!(serde_json::from_str::<Effects>(&json).unwrap(), paint);
    }
    #[test]
    fn vignette_highlights_apply_only_to_darkening_highlight_and_color_priority() {
        use VignetteStyle::*;
        let corner = |style, vignette, highlights| {
            let r = Recipe {
                effects: Effects {
                    vignette_style: style,
                    vignette,
                    vignette_highlights: highlights,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut im = Rendered {
                width: 9,
                height: 9,
                pixels: vec![[0.95; 3]; 81],
            };
            spatial_finish(&mut im, &r, [0, 0], [9, 9]);
            im.pixels[0][1]
        };
        for (style, vignette, protected) in [
            (HighlightPriority, -0.6, true),
            (ColorPriority, -0.6, true),
            (PaintOverlay, -0.6, false),
            (HighlightPriority, 0.6, false),
            (ColorPriority, 0.6, false),
        ] {
            let with = corner(style, vignette, 0.8);
            let without = corner(style, vignette, 0.);
            assert_eq!(with != without, protected, "{style:?} {vignette}");
        }
    }
    #[test]
    fn effects_reset_covers_every_vignette_and_grain_control_and_nothing_else() {
        let d = Effects::default();
        let mut e = Effects {
            clarity: 0.4,
            dehaze: -0.2,
            defringe: [0.5, 0.5],
            lens_vignette: 0.3,
            grain_seed: 7,
            grain: 0.6,
            grain_size: 0.9,
            grain_roughness: 0.1,
            vignette: -0.7,
            vignette_midpoint: 0.2,
            vignette_roundness: 0.6,
            vignette_feather: 0.9,
            vignette_highlights: 0.8,
            vignette_style: VignetteStyle::ColorPriority,
            ..Default::default()
        };
        let kept = e.clone();
        e.reset_post_crop();
        assert_eq!(
            (e.grain, e.grain_size, e.grain_roughness),
            (d.grain, d.grain_size, d.grain_roughness)
        );
        assert_eq!(
            (
                e.vignette,
                e.vignette_midpoint,
                e.vignette_roundness,
                e.vignette_feather,
                e.vignette_highlights,
                e.vignette_style
            ),
            (
                d.vignette,
                d.vignette_midpoint,
                d.vignette_roundness,
                d.vignette_feather,
                d.vignette_highlights,
                d.vignette_style
            )
        );
        assert_eq!(
            (
                e.clarity,
                e.dehaze,
                e.defringe,
                e.lens_vignette,
                e.grain_seed
            ),
            (
                kept.clarity,
                kept.dehaze,
                kept.defringe,
                kept.lens_vignette,
                kept.grain_seed
            )
        );
    }
    #[test]
    fn fringe_selector_sets_the_band_of_the_picked_color() {
        let mut e = Effects::default();
        assert_eq!(e.pick_fringe([0.6, 0.3, 0.8]), Some(0));
        assert_eq!(e.defringe[0], 0.25);
        let [lo, hi] = e.defringe_ranges[0];
        assert!((hi - lo - 0.2).abs() < 1e-6 && (0. ..=1.).contains(&lo) && hi <= 1.);
        // The picked hue is inside the new range and is removed.
        let lab = crate::develop::pipeline::srgb_to_lab(
            [0.6f32, 0.3, 0.8].map(crate::color::srgb_decode),
        );
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        e.defringe[0] = 1.;
        let out = e.defringe_color(lab, hue);
        assert!(out[1].hypot(out[2]) < lab[1].hypot(lab[2]) * 0.5);
        // An Amount already set is kept.
        e.defringe[1] = 0.6;
        assert_eq!(e.pick_fringe([0.3, 0.7, 0.3]), Some(1));
        assert_eq!(e.defringe[1], 0.6);
        // Neither purple nor green, or no colour at all.
        let before = e.clone();
        assert_eq!(e.pick_fringe([0.8, 0.2, 0.2]), None);
        assert_eq!(e.pick_fringe([0.5, 0.5, 0.5]), None);
        assert_eq!(e, before);
    }
}
