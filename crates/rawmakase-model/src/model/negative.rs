//! Film negative conversion as a recipe stores it: a scan of a colour or black & white
//! negative turned into a positive before white balance. RAWmakase's own setting:
//! Lightroom has none, so XMP neither reads nor writes it. How it renders is
//! `develop::negative`'s.
//!
//! The values are the film's, measured in the decoded camera values of the scan (a
//! RAW's camera RGB, or a rendered file's linear ProPhoto RGB): the same values the
//! renderer converts. The model works in density above the film base,
//! `d = log10(base / v)` per channel, so the base is 0 and denser film (a brighter
//! scene) is larger. `dmin` and `dmax` bound the densities the frame uses: `dmax`
//! renders as the scene's white, `dmin` as its darkest tone.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// The kind of film a scan is of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmType {
    /// A colour negative, orange mask and all: each channel is converted with its own
    /// bounds.
    #[default]
    Colour,
    /// A black & white negative: one density, the channels' mean, so the scan's colour
    /// (a toned base, the light source) leaves no cast.
    BlackWhite,
}

/// The largest film base a recipe may hold, in camera values: far above the white of
/// any decode, which a base picked on unclipped film cannot reach.
pub const MAX_BASE: f32 = 100.;
/// The largest density magnitude `dmin` and `dmax` may hold: ten decades, far beyond
/// any film.
pub const MAX_DENSITY: f32 = 10.;
/// The smallest density range (`dmax - dmin`) per channel. The range divides, so a
/// narrower one would stretch the scan's noise into the whole tonal range.
pub const MIN_DENSITY_RANGE: f32 = 0.05;

/// A film negative conversion (see the module documentation).
///
/// A field from a later release is refused rather than dropped, as in the recipe's
/// other settings: the conversion would otherwise render differently from how it was
/// made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NegativeConversion {
    pub film: FilmType,
    /// The film base (the unexposed film, orange mask and all) in decoded camera
    /// values: density 0.
    pub base: [f32; 3],
    /// The density above the base that renders as the frame's darkest tone, per
    /// channel.
    pub dmin: [f32; 3],
    /// The density above the base that renders as the frame's white (1.0 before white
    /// balance), per channel.
    pub dmax: [f32; 3],
}

impl NegativeConversion {
    /// A conversion of `film` that nothing has measured yet: the base at the decode's
    /// white (1.0, where a camera's green channel clips), so densities are of the scan
    /// itself, and densities 0 to 1.5 above it as the frame. A scan's densest
    /// highlights are about that high (the base's own 0.2 to 0.5, plus about 1 for a
    /// normal scene), so the frame shows a recognisable positive, still tinted by the
    /// mask, until the film base picker and Auto measure it.
    pub fn new(film: FilmType) -> Self {
        Self {
            film,
            base: [1.; 3],
            dmin: [0.; 3],
            dmax: [1.5; 3],
        }
    }

    /// This conversion for `film` instead. The base is the scan's and is kept. Black &
    /// white film has one density, so bounds measured per channel become their mean
    /// (one pair for every channel, which renders without a cast); a colour negative
    /// keeps them.
    pub fn with_film(&self, film: FilmType) -> Self {
        let mean = |v: [f32; 3]| [(v[0] + v[1] + v[2]) / 3.; 3];
        let (dmin, dmax) = match film {
            FilmType::Colour => (self.dmin, self.dmax),
            FilmType::BlackWhite => (mean(self.dmin), mean(self.dmax)),
        };
        Self {
            film,
            dmin,
            dmax,
            ..self.clone()
        }
    }

    /// Refuses values the conversion can't render: a base that isn't a positive
    /// finite camera value, densities that aren't finite, and a channel whose
    /// `dmax - dmin` is below [`MIN_DENSITY_RANGE`].
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.base
                .iter()
                .all(|v| v.is_finite() && *v > 0. && *v <= MAX_BASE),
            "Invalid film base"
        );
        ensure!(
            (0..3).all(|c| {
                let (lo, hi) = (self.dmin[c], self.dmax[c]);
                lo.is_finite()
                    && hi.is_finite()
                    && lo.abs() <= MAX_DENSITY
                    && hi.abs() <= MAX_DENSITY
                    && hi - lo >= MIN_DENSITY_RANGE
            }),
            "Invalid film density range"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_conversion_is_valid_for_either_film() {
        for film in [FilmType::Colour, FilmType::BlackWhite] {
            let n = NegativeConversion::new(film);
            assert_eq!(n.film, film);
            n.validate().unwrap();
        }
    }

    #[test]
    fn validation_refuses_values_the_conversion_cannot_render() {
        let ok = NegativeConversion::new(FilmType::Colour);
        type Change = (&'static str, fn(&mut NegativeConversion));
        let bad: [Change; 10] = [
            ("zero base", |n| n.base[1] = 0.),
            ("negative base", |n| n.base[0] = -0.5),
            ("NaN base", |n| n.base[2] = f32::NAN),
            ("infinite base", |n| n.base[2] = f32::INFINITY),
            ("huge base", |n| n.base[2] = 1e6),
            ("dmax below dmin", |n| {
                n.dmin[1] = 1.;
                n.dmax[1] = 0.5
            }),
            ("dmax equal to dmin", |n| n.dmax[0] = n.dmin[0]),
            ("range below the minimum", |n| n.dmax[2] = 0.01),
            ("NaN density", |n| n.dmin[2] = f32::NAN),
            ("infinite density", |n| n.dmax[1] = f32::INFINITY),
        ];
        for (name, change) in bad {
            let mut n = ok.clone();
            change(&mut n);
            assert!(n.validate().is_err(), "{name}");
        }
        // Negative densities are fine: a frame may be thinner than the base picked.
        let mut n = ok;
        n.dmin = [-0.1, 0., 0.05];
        n.base = [0.02, 0.8, 3.];
        n.validate().unwrap();
    }

    /// Changing the film keeps the scan's base; black & white gets one pair of bounds
    /// for every channel, and stays valid.
    #[test]
    fn changing_the_film_keeps_the_base() {
        let colour = NegativeConversion {
            base: [0.9, 0.55, 0.3],
            dmin: [0.05, 0.1, 0.15],
            dmax: [1.1, 1.3, 1.5],
            ..NegativeConversion::new(FilmType::Colour)
        };
        let bw = colour.with_film(FilmType::BlackWhite);
        assert_eq!(bw.film, FilmType::BlackWhite);
        assert_eq!(bw.base, colour.base);
        assert!(bw.dmin.iter().all(|d| (d - 0.1).abs() < 1e-6), "{bw:?}");
        assert!(bw.dmax.iter().all(|d| (d - 1.3).abs() < 1e-6), "{bw:?}");
        bw.validate().unwrap();
        let back = bw.with_film(FilmType::Colour);
        assert_eq!(
            back,
            NegativeConversion {
                film: FilmType::Colour,
                ..bw
            }
        );
        assert_eq!(colour.with_film(FilmType::Colour), colour);
    }

    /// The stored names: renaming one would make every saved conversion unreadable.
    #[test]
    fn a_conversion_keeps_its_stored_names() {
        let n = NegativeConversion::new(FilmType::BlackWhite);
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "film": "black_white",
                "base": [1.0, 1.0, 1.0],
                "dmin": [0.0, 0.0, 0.0],
                "dmax": [1.5, 1.5, 1.5],
            })
        );
        assert_eq!(serde_json::from_value::<NegativeConversion>(v).unwrap(), n);
        assert_eq!(
            serde_json::to_value(FilmType::Colour).unwrap(),
            serde_json::json!("colour")
        );
    }
}
