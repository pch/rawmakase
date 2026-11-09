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
/// same scene values the first time a render needs it; and what renders measure on
/// the same scene values with one more setting, by that setting.
#[derive(Debug)]
pub(crate) struct Measured {
    pub(crate) measures: PhotoMeasures,
    pub(crate) haze: std::sync::OnceLock<std::sync::Arc<super::Haze>>,
    /// The Contrast pivot by the Dehaze it was measured at.
    pivots: Kept<f32>,
    /// The Shadows/Highlights map's base by the Exposure it was built at.
    maps: Kept<std::sync::Arc<crate::develop::local_tone::MapBase>>,
    /// The copy's camera colour (white balance, the profile's matrix and HueSatMap,
    /// calibration), which the map's base exposes.
    colors: std::sync::OnceLock<std::sync::Arc<Vec<[f32; 3]>>>,
}
impl Measured {
    pub(crate) fn new(measures: PhotoMeasures) -> Self {
        Self {
            measures,
            haze: Default::default(),
            pivots: Kept::default(),
            maps: Kept::default(),
            colors: Default::default(),
        }
    }
    /// The copy's camera colour, made by `make` unless kept. Not initialised in place: a
    /// render that waited on it could be the task its parallel work runs; renders
    /// making it at the same time make the same colours.
    pub(crate) fn colors(
        &self,
        make: impl FnOnce() -> Vec<[f32; 3]>,
    ) -> std::sync::Arc<Vec<[f32; 3]>> {
        if let Some(c) = self.colors.get() {
            return c.clone();
        }
        let colors = std::sync::Arc::new(make());
        self.colors.get_or_init(|| colors).clone()
    }
    /// How many pivots and map bases are kept.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> [usize; 2] {
        [
            self.pivots.0.lock().unwrap().len(),
            self.maps.0.lock().unwrap().len(),
        ]
    }
    /// The Shadows/Highlights map's base at Exposure `exposure`, built by `build` unless
    /// kept.
    pub(crate) fn map_base(
        &self,
        exposure: f32,
        build: impl FnOnce() -> crate::develop::local_tone::MapBase,
    ) -> std::sync::Arc<crate::develop::local_tone::MapBase> {
        self.maps.get_or(exposure, || std::sync::Arc::new(build()))
    }
    /// The Contrast pivot at Dehaze `dehaze`, measured by `measure` unless kept.
    pub(crate) fn pivot(&self, dehaze: f32, measure: impl FnOnce() -> f32) -> f32 {
        self.pivots.get_or(dehaze, measure)
    }
}
/// Values by the setting they were made with, the last few used.
#[derive(Debug)]
struct Kept<T>(std::sync::Mutex<Vec<(u32, T)>>);
impl<T> Default for Kept<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}
impl<T: Clone> Kept<T> {
    /// Values kept: a slider's few last positions.
    const LEN: usize = 4;
    /// The value for `setting`, made by `make` unless kept. Not locked while making,
    /// whose parallel work may run other renders' tasks; renders making it at the same
    /// time make the same value.
    fn get_or(&self, setting: f32, make: impl FnOnce() -> T) -> T {
        let key = setting.to_bits();
        let lock = || {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        if let Some((_, v)) = lock().iter().find(|(k, _)| *k == key) {
            return v.clone();
        }
        let value = make();
        let mut kept = lock();
        kept.retain(|(k, _)| *k != key);
        kept.insert(0, (key, value.clone()));
        kept.truncate(Self::LEN);
        value
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
    /// The level positive Whites follows in an HDR render (log2) at `exposure` stops:
    /// twice the 99th percentile of luminance, as in SDR, but not limited by the sensor's
    /// white ([`super::hdr_whites`]). On the twelve HDR validation photos it predicts the
    /// level Camera Raw's Whites +100 renders at to 0.17 stop (standard deviation); the
    /// 98th and 99.5th percentiles do no better.
    pub(crate) fn hdr_whites_level(&self, exposure: f32) -> f32 {
        self.p99 + 1. + exposure
    }
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
