//! Effects > Film Grain as a recipe stores it: RAWmakase's own grain of random-sized
//! grains at random positions, beside Camera Raw's Grain (`Effects::grain`). Lightroom
//! has no such setting, so it is not written to XMP. Rendering it is in
//! `develop/effects/film_grain.rs`.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilmGrain {
    /// Strength, 0..1; 0 renders nothing.
    pub amount: f32,
    /// Mean grain size, 0..1, as a share of the photo's long edge.
    pub size: f32,
    /// How much grain sizes vary, 0..1.
    pub roughness: f32,
    /// From monochrome grain (0) to a grain of its own in each dye layer (1).
    pub color: f32,
    pub seed: u32,
}
impl Default for FilmGrain {
    fn default() -> Self {
        Self {
            amount: 0.,
            size: 0.25,
            roughness: 0.5,
            color: 0.,
            seed: 42,
        }
    }
}
impl FilmGrain {
    /// Whether it renders nothing.
    pub fn is_off(&self) -> bool {
        self.amount == 0.
    }
    /// Whether every setting is at its default: such a recipe doesn't store it, so
    /// releases that predate it read the recipe unchanged.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// The Effects panel's reset: the sliders at their defaults, the seed kept.
    pub fn reset(&mut self) {
        *self = Self {
            seed: self.seed,
            ..Self::default()
        };
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [self.amount, self.size, self.roughness, self.color]
                .iter()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid film grain"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_keeps_the_seed_and_validation_bounds_every_slider() {
        let mut g = FilmGrain {
            amount: 0.4,
            size: 0.8,
            roughness: 0.1,
            color: 0.6,
            seed: 7,
        };
        g.validate().unwrap();
        g.reset();
        assert_eq!(
            g,
            FilmGrain {
                seed: 7,
                ..FilmGrain::default()
            }
        );
        for bad in [
            FilmGrain { amount: 1.5, ..g },
            FilmGrain { size: -0.1, ..g },
            FilmGrain { color: 1.2, ..g },
            FilmGrain {
                roughness: f32::NAN,
                ..g
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
}
