//! Values shared by the operators that render a recipe.

/// The Sharpening sliders, in recipe units (Amount 1 is Lightroom's 150).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharpeningSliders {
    pub amount: f32,
    pub radius: f32,
    pub detail: f32,
    pub masking: f32,
}
impl SharpeningSliders {
    /// A new edit's sliders: Lightroom Classic's raw defaults (in every raw and DNG
    /// import of the catalogue sampled, process versions 2012 to 6).
    pub fn defaults() -> Self {
        Self {
            amount: 40. / 150.,
            radius: 1.,
            detail: 0.25,
            masking: 0.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharpening_defaults_are_lightroom_s() {
        let d = SharpeningSliders::defaults();
        // Lightroom's Amount 40 (of 150), Radius 1.0, Detail 25, Masking 0.
        assert_eq!(
            [d.amount, d.radius, d.detail, d.masking],
            [40. / 150., 1., 0.25, 0.]
        );
    }
}
