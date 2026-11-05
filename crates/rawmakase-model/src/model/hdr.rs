//! High Dynamic Range editing, as Lightroom and Camera Raw save it: whether the edit
//! renders in HDR, its HDR limit, and the adjustments of the SDR rendition derived
//! from the HDR master (docs/records/hdr-implementation-plan.md).
//!
//! These are saved settings only. Display headroom and the view-only SDR preview are
//! not part of an edit. Values set while HDR is off are kept, so switching HDR back
//! on restores them, as Camera Raw does.
//!
//! Rendering an active HDR edit is not supported yet. [`HdrSettings::check_renderable`]
//! is the one guard every entry point uses to refuse one (XMP, Lightroom's Develop
//! settings, presets, saved edits and automation all validate the recipe), so an active
//! HDR edit is preserved and reported rather than rendered as SDR.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// The HDR limit's range, in stops above graphics white (`crs:HDRMaxValue`). Camera
/// Raw 18.7 renders a limit of `L` with nothing brighter than 2^`L` times graphics
/// white. It keeps 1 and 8 and replaces 0.99 and 10 with [`DEFAULT_LIMIT`]; values
/// between 8 and 10 were not measured and are refused.
pub const LIMIT_RANGE: std::ops::RangeInclusive<f32> = 1.0..=8.0;
/// The HDR limit Camera Raw 18.7 uses when a sidecar has none or an invalid one.
pub const DEFAULT_LIMIT: f32 = 2.3;

/// Whether this release renders active HDR edits. Raised only when the CPU HDR master
/// and its SDR rendition reach every caller that renders an edit.
const RENDERS_HDR: bool = false;

/// An edit's HDR settings: its mode, limit and SDR rendition.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HdrSettings {
    /// Lightroom's HDR switch (`crs:HDREditMode`).
    pub enabled: bool,
    /// The HDR limit, in stops above graphics white (`crs:HDRMaxValue`).
    pub limit: f32,
    /// The SDR rendition's adjustments. They never change the HDR master.
    pub sdr: SdrRendition,
}

/// The SDR rendition's sliders, as Camera Raw's `crs:SDR*` values divided by 100
/// (−1 to 1, 0 by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SdrRendition {
    pub brightness: f32,
    pub contrast: f32,
    pub clarity: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blend: f32,
}

impl Default for HdrSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: DEFAULT_LIMIT,
            sdr: SdrRendition::default(),
        }
    }
}

impl SdrRendition {
    /// The sliders in the order of Camera Raw's keys,
    /// [`crate::model::camera_raw::SDR_RENDITION`].
    pub fn values_mut(&mut self) -> [&mut f32; 7] {
        [
            &mut self.brightness,
            &mut self.contrast,
            &mut self.clarity,
            &mut self.highlights,
            &mut self.shadows,
            &mut self.whites,
            &mut self.blend,
        ]
    }
    /// [`Self::values_mut`], read.
    pub fn values(&self) -> [f32; 7] {
        let mut copy = *self;
        copy.values_mut().map(|v| *v)
    }
}

impl HdrSettings {
    /// HDR off with every value at its default: not saved, so releases that predate HDR
    /// read the recipe unchanged.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Whether the edit renders in HDR. Values kept while HDR is off do not count: a
    /// release that predates HDR keeps them as unknown settings and renders the edit
    /// correctly as SDR.
    pub fn is_active(&self) -> bool {
        self.enabled
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.limit.is_finite() && LIMIT_RANGE.contains(&self.limit),
            "HDR limit out of bounds"
        );
        ensure!(
            self.sdr
                .values()
                .iter()
                .all(|v| v.is_finite() && (-1. ..=1.).contains(v)),
            "Invalid SDR rendition adjustment"
        );
        Ok(())
    }
    /// Refuses an active HDR edit while this release cannot render one. The edit is
    /// kept as it was: callers preserve it and report the error.
    pub fn check_renderable(&self) -> Result<()> {
        ensure!(
            RENDERS_HDR || !self.is_active(),
            "HDR editing is preserved but not supported yet"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_kept_while_off_are_not_active_but_are_not_default() {
        let mut hdr = HdrSettings::default();
        assert!(hdr.is_default() && !hdr.is_active());
        hdr.limit = 3.;
        hdr.sdr.brightness = 0.3;
        assert!(!hdr.is_default() && !hdr.is_active());
        assert!(hdr.validate().is_ok() && hdr.check_renderable().is_ok());
        hdr.enabled = true;
        assert!(hdr.is_active());
        assert!(hdr.validate().is_ok());
        assert!(hdr.check_renderable().is_err());
    }

    #[test]
    fn limits_and_sliders_outside_camera_raws_ranges_are_invalid() {
        for limit in [0.99, 8.01, f32::NAN, f32::INFINITY] {
            let hdr = HdrSettings {
                limit,
                ..Default::default()
            };
            assert!(hdr.validate().is_err(), "{limit}");
        }
        for value in [1.01, -1.01, f32::NAN] {
            let mut hdr = HdrSettings::default();
            hdr.sdr.blend = value;
            assert!(hdr.validate().is_err(), "{value}");
        }
    }
}
