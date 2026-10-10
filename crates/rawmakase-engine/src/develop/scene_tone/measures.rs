//! What the scene tone stage measures of a photo (docs/scene-tone-stage.md, the
//! measurement-input table).

/// A photo's measures, taken once from its reduced copy at the scene stage's input with
/// the user's Exposure at 0, so they are the same for every preview size, region and
/// export. All are log2 scene values; Exposure moves them all by its stops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PhotoMeasures {
    /// The sensor's white for a neutral: the level at which its last channel clips.
    pub(crate) sensor_white: f32,
    /// The brightest channel's maximum on a copy reduced to `MAX_EDGE`: bright areas
    /// count, specks do not.
    pub(crate) max: f32,
    /// The darkest luminance on the reduced copy.
    pub(crate) min: f32,
    /// Its luminance's 0.1th percentile: a darkest level a few dark pixels do not move,
    /// which Camera Raw's default black follows.
    pub(crate) dark: f32,
    /// Its luminance's 99th percentile: the level Dehaze's response is relative to, and
    /// a stop below the level Whites stretches toward.
    pub(crate) p99: f32,
}
/// What the stage cache keeps of a photo's measurement copy for the settings the
/// measures read: the measures, and the haze positive Dehaze removes, measured on the
/// same scene values the first time a render needs it.
#[derive(Debug)]
pub(crate) struct Measured {
    pub(crate) measures: PhotoMeasures,
    pub(crate) haze: std::sync::OnceLock<std::sync::Arc<super::Haze>>,
}
impl Measured {
    pub(crate) fn new(measures: PhotoMeasures) -> Self {
        Self {
            measures,
            haze: Default::default(),
        }
    }
}
/// Long edge of the copy the maximum is taken on: on a 920-pixel probe Camera Raw
/// counts a bright spot fully from about 8 pixels across, and not at 2.
pub(crate) const MAX_EDGE: u32 = 128;

impl Default for PhotoMeasures {
    /// A photo whose white is the sensor's at 1 and that reaches it.
    fn default() -> Self {
        Self {
            sensor_white: 0.,
            max: 0.,
            min: -12.,
            dark: -12.,
            p99: -1.,
        }
    }
}
impl PhotoMeasures {
    /// The white point (log2 W*) at `exposure` stops: the sensor's white, limited to
    /// twice the photo's maximum but not below 1; with the sensor's white below 1
    /// (negative Exposure), that white but not below 0.5, so highlights expand by at
    /// most a stop.
    pub(crate) fn white_point(&self, exposure: f32) -> f32 {
        let sensor = self.sensor_white + exposure;
        if sensor < 0. {
            sensor.max(-1.)
        } else {
            sensor.min((self.max + exposure + 1.).max(0.))
        }
    }
    /// The level Whites stretches toward (log2) at `exposure` stops: twice the 99th
    /// percentile of luminance, at most the sensor's white (fitted on training photos;
    /// docs/scene-tone-stage.md#whites-and-blacks).
    pub(crate) fn whites_top(&self, exposure: f32) -> f32 {
        (self.p99 + 1.).min(self.sensor_white) + exposure
    }
    /// The black key (log2 of the photo's darkest level) at `exposure` stops.
    pub(crate) fn black_key(&self, exposure: f32) -> f32 {
        self.min + exposure
    }
    /// The level the default black follows (log2, the 0.1th percentile of luminance) at
    /// `exposure` stops.
    pub(crate) fn dark_key(&self, exposure: f32) -> f32 {
        self.dark + exposure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(sensor: f32, max: f32, exposure: f32) -> f32 {
        PhotoMeasures {
            sensor_white: sensor.log2(),
            max: max.log2(),
            min: -12.,
            dark: -12.,
            p99: -1.,
        }
        .white_point(exposure)
        .exp2()
    }

    /// Whites stretches toward twice the photo's 99th percentile of luminance, not its
    /// maximum: on 22 training photos that level reproduces Camera Raw's Whites +100 to
    /// 0.1 stops on average, the maximum to 1.0.
    #[test]
    fn whites_stretch_toward_twice_the_99th_percentile() {
        let measures = |max: f32, p99: f32| PhotoMeasures {
            sensor_white: 1.,
            max,
            min: -12.,
            dark: -12.,
            p99,
        };
        // A specular highlight at the sensor's white does not hold Whites back.
        assert_eq!(measures(1., -3.).whites_top(0.), -2.);
        // Never above the sensor's white; Exposure moves it.
        assert_eq!(measures(1., 0.5).whites_top(0.), 1.);
        assert_eq!(measures(1., -3.).whites_top(-1.), -3.);
    }

    #[test]
    fn the_white_point_follows_the_rule_measured_on_probes() {
        // A photo reaching its sensor's white keeps it as the white point.
        assert!((white(4., 4., 0.) - 4.).abs() < 1e-5);
        // A dim photo: twice its maximum.
        assert!((white(4., 1., 0.) - 2.).abs() < 1e-5);
        // Never below 1 while the sensor's white is above it.
        assert!((white(4., 0.25, 0.) - 1.).abs() < 1e-5);
        // Negative Exposure: the sensor's white, expanded by at most a stop.
        assert!((white(1., 1., -1.) - 0.5).abs() < 1e-5);
        assert!((white(1., 1., -2.) - 0.5).abs() < 1e-5);
        assert!((white(1., 1., -0.5) - 0.5f32.sqrt()).abs() < 1e-5);
        // Exposure moves the sensor's white and the photo together.
        assert!((white(2., 2., 1.) - 4.).abs() < 1e-5);
    }
}
