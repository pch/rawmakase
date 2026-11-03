//! Camera reference metadata, not fitted image corrections.
//! X100F values were read from Lightroom-generated DNGs of DSCF7845 (ISO 400)
//! and DSCF7853 (ISO 200), both DR100. See docs/records/macos-lightroom-validation.md.
//! Per-camera baseline exposures live in data/cameras.toml (crate::cameras).
use crate::camera_data::{HighlightTonePriority, Metadata};
/// Camera Raw's default exposure for this photo: a DNG's own BaselineExposure,
/// otherwise the camera table's value (data/cameras.toml). A rendered file has none.
pub fn baseline_exposure(m: &Metadata) -> f32 {
    if m.is_rendered() {
        return 0.;
    }
    if let Some(dng) = m.baseline_exposure {
        return dng;
    }
    let row = crate::cameras::baseline_exposure(&m.make, &m.model);
    // Adobe gives small raws a baseline of their own (see data/cameras.toml).
    let ev = if m.sraw {
        row.sraw_ev
    } else {
        row.ev + white_shift(m, &row)
    };
    let mut table = ev + fujifilm_shift(m, &row);
    // Below the lowest native ISO the camera exposes more and pulls the raw back.
    if row
        .lowest_iso
        .is_some_and(|lowest| m.iso > 0. && m.iso < lowest - 1.)
    {
        table -= 1.;
    }
    // Table rows hold for photos without Highlight Tone Priority, which Camera Raw
    // brightens by the stop the camera held back.
    match m.highlight_tone_priority {
        HighlightTonePriority::Off => table,
        HighlightTonePriority::On | HighlightTonePriority::Enhanced => table + 1.,
    }
}
/// Adobe's BaselineExposure for a body whose white level it takes by ISO speed
/// (Canon) moves with that white, so the photo's brightness stays referenced
/// to the samples' full range: a photo whose white, over its black, is
/// another part of that range than the row's sample's takes the difference,
/// log2((white − black) / (maximum − black)) less the sample's. On the 325
/// Canon files of the test dataset Adobe DNG Converter converted, 323 come
/// within 0.1 EV of its BaselineExposure (the white alone, over the black,
/// leaves the bodies whose black moves with ISO speed 0.2 EV off). 0 without
/// both levels.
fn white_shift(m: &Metadata, row: &crate::cameras::Baseline) -> f32 {
    let (Some(photo), Some(sample)) = (m.levels, row.levels) else {
        return 0.;
    };
    let part = |[black, white, maximum]: [f32; 3]| {
        let (range, full) = (white - black, maximum - black);
        (range > 0. && full > 0.).then(|| (range / full).log2())
    };
    match (part(photo.map(|v| v as f32)), part(sample)) {
        (Some(photo), Some(sample)) => photo - sample,
        _ => 0.,
    }
}
/// Camera Raw's baseline for a Fujifilm raw is a per-body constant minus the raw's
/// exposure midpoint shift, which moves a stop for each DR step (DR200 is often not
/// recorded otherwise) and for extended ISO. Table rows hold at the body's DR100 shift:
/// the row's own, else the sensor's usual one.
fn fujifilm_shift(m: &Metadata, row: &crate::cameras::Baseline) -> f32 {
    let Some(shift) = m.fuji_exposure_shift else {
        return 0.;
    };
    if !m.make.eq_ignore_ascii_case("Fujifilm")
        || row.exposure_shift == crate::cameras::ExposureShift::Ignored
    {
        return 0.;
    }
    let usual = if m.xtrans { -0.72 } else { 0. };
    row.dr100_shift.unwrap_or(usual) - shift
}
pub fn neutral_calibration(m: &Metadata) -> [f32; 3] {
    crate::cameras::neutral_calibration(&m.make, &m.model, &m.wb_presets)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn camera(make: &str, model: &str) -> Metadata {
        Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        }
    }
    fn fujifilm(model: &str, xtrans: bool, shift: Option<f32>) -> Metadata {
        Metadata {
            xtrans,
            fuji_exposure_shift: shift,
            ..camera("Fujifilm", model)
        }
    }
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }
    #[test]
    fn fujifilm_baselines_follow_the_exposure_shift() {
        // Rows hold at the usual DR100 shift: -0.72 for X-Trans, -0.49 for GFX.
        let row = |model| crate::cameras::baseline_exposure("Fujifilm", model).ev;
        let x_e5 = fujifilm("X-E5", true, Some(-0.72));
        assert_eq!(baseline_exposure(&x_e5), row("X-E5"));
        // DR200 (the raw often does not say so) shifts a stop: Camera Raw adds it back.
        let x_e5 = fujifilm("X-E5", true, Some(-1.72));
        assert!(close(baseline_exposure(&x_e5), row("X-E5") + 1.));
        // Extended ISO 100 on an ISO 160 body shifts the other way.
        let x_t30 = fujifilm("X-T30", true, Some(0.28));
        assert!(close(baseline_exposure(&x_t30), row("X-T30") - 1.));
        // GFX rows give their own DR100 shift, -0.49.
        let gfx = fujifilm("GFX100 II", false, Some(-1.72));
        assert!(close(baseline_exposure(&gfx), row("GFX100 II") + 1.23));
        // A Bayer body without one assumes 0, whatever its name.
        let unlisted = fujifilm("GFX200", false, Some(-1.));
        assert!(close(baseline_exposure(&unlisted), row("GFX200") + 1.));
        // Small Bayer bodies record no shift at DR100.
        let xf10 = Metadata {
            fuji_dynamic_range: 65535,
            ..fujifilm("XF10", false, Some(0.))
        };
        assert_eq!(baseline_exposure(&xf10), row("XF10"));
        // Without a shift in the raw, the row as it is.
        assert_eq!(baseline_exposure(&fujifilm("X-T5", true, None)), -0.1);
        // Camera Raw keeps one value for the X-Trans III bodies whatever the shift.
        let x_t2 = fujifilm("X-T2", true, Some(-1.72));
        assert_eq!(baseline_exposure(&x_t2), 0.15);
        assert_eq!(
            baseline_exposure(&fujifilm("X100F", true, Some(-1.72))),
            0.15
        );
        assert_eq!(
            neutral_calibration(&fujifilm("Unlisted body", true, None)),
            [1.; 3]
        );
    }
    #[test]
    fn sony_extended_low_iso_takes_a_stop_off() {
        // Adobe DNG Converter 18.7: A7R III at ISO 50 and 80 -0.85, at 100 +0.15;
        // the A7S III's ISO 80 is native and keeps its row.
        let at = |model, iso| Metadata {
            iso,
            ..camera("Sony", model)
        };
        assert_eq!(baseline_exposure(&at("ILCE-7RM3", 100.)), 0.15);
        assert!(close(baseline_exposure(&at("ILCE-7RM3", 80.)), -0.85));
        assert!(close(baseline_exposure(&at("ILCE-7RM3", 50.)), -0.85));
        assert!(close(baseline_exposure(&at("ILCE-7M3", 64.)), -0.65));
        assert_eq!(baseline_exposure(&at("ILCE-7SM3", 80.)), 0.15);
        // A raw without an ISO keeps the row.
        assert_eq!(baseline_exposure(&at("ILCE-7RM3", 0.)), 0.15);
    }
    /// Adobe takes a Canon's white by ISO speed and moves its BaselineExposure
    /// with it: the EOS 1100D's is -0.5 at its ISO 160 sample's white of
    /// 11222 (black 2047), 0 at ISO 400's 15000 (Adobe DNG Converter 18.7).
    #[test]
    fn canon_exposure_follows_the_white_adobe_takes_by_iso() {
        let at = |levels| Metadata {
            levels,
            ..camera("Canon", "EOS 1100D")
        };
        assert!(close(
            baseline_exposure(&at(Some([2047, 11222, 16383]))),
            -0.5
        ));
        let iso_400 = baseline_exposure(&at(Some([2047, 15000, 16383])));
        assert!(
            close(iso_400, -0.5 + (12953f32 / 9175.).log2()),
            "{iso_400}"
        );
        assert!(iso_400.abs() < 0.01);
        // The EOS 200D's black rises with ISO speed: 0 at ISO 100 (511 to
        // 11892), Adobe's 0.32 at ISO 400 (2048 to 14338); the part of the
        // 14-bit range each white takes gives 0.26.
        let iso_400 = baseline_exposure(&Metadata {
            levels: Some([2048, 14338, 16383]),
            ..camera("Canon", "EOS 200D")
        });
        let want = (12290f32 / 14335.).log2() - (11381f32 / 15872.).log2();
        assert!(
            close(iso_400, want) && (want - 0.258).abs() < 0.001,
            "{iso_400}"
        );
        // Without the photo's levels, or for a row without its sample's, the
        // row alone.
        assert!(close(baseline_exposure(&at(None)), -0.5));
        let other = Metadata {
            levels: Some([512, 9000, 16383]),
            ..camera("Sony", "ILCE-7M4")
        };
        assert_eq!(baseline_exposure(&other), 0.35);
        // Small RAWs keep their own baseline.
        let sraw = Metadata {
            sraw: true,
            ..at(Some([2047, 15000, 16383]))
        };
        assert!(close(baseline_exposure(&sraw), 0.25));
    }
    #[test]
    fn canon_sraw_takes_its_own_baseline() {
        // Adobe gives sRAW and mRAW a BaselineExposure of their own, one per body
        // (from ISO 100 to 3200, both sizes).
        let sraw = |model| Metadata {
            sraw: true,
            ..camera("Canon", model)
        };
        assert_eq!(baseline_exposure(&camera("Canon", "EOS 5D Mark IV")), 0.15);
        assert!(close(baseline_exposure(&sraw("EOS 5D Mark IV")), 0.35));
        assert!(close(baseline_exposure(&sraw("EOS 5DS R")), 0.25));
        // A Canon body without one takes the median of the sRAW values.
        assert!(close(baseline_exposure(&sraw("EOS 40D")), 0.25));
        // Highlight Tone Priority adds its stop to the sRAW value too.
        let htp = Metadata {
            highlight_tone_priority: HighlightTonePriority::On,
            ..sraw("EOS 5DS R")
        };
        assert!(close(baseline_exposure(&htp), 1.25));
    }
    #[test]
    fn olympus_units_use_their_5300k_preset() {
        use crate::camera_data::WhiteBalancePresets;
        // Two E-M5 Mark III units: their calibrations differ exactly as their
        // stored 5300 K presets do (data/camera-calibration-facts.json).
        let unit = |v| Metadata {
            wb_presets: WhiteBalancePresets {
                fine_weather: Some(v),
                ..Default::default()
            },
            ..camera("Olympus", "E-M5 Mark III")
        };
        assert_eq!(
            neutral_calibration(&unit([460., 256., 540.])),
            [1.0478, 1., 0.8741]
        );
        assert_eq!(
            neutral_calibration(&unit([512., 256., 462.])),
            [0.9414, 1., 1.0216]
        );
    }
    #[test]
    fn canon_and_fujifilm_units_use_their_own_presets() {
        use crate::camera_data::WhiteBalancePresets;
        // Anonymous native presets of separate units, with the CameraCalibration
        // Adobe DNG Converter 18.7 recorded for each; see data/camera-calibration-facts.json.
        let unit = |make: &str, model: &str, wb_presets| Metadata {
            wb_presets,
            ..camera(make, model)
        };
        let daylight = |v| WhiteBalancePresets {
            daylight: Some(v),
            ..Default::default()
        };
        let d65 = |v| WhiteBalancePresets {
            d65: Some(v),
            ..Default::default()
        };
        for (m, expected) in [
            (
                unit("Canon", "EOS 800D", daylight([1872., 1024., 1649.])),
                [1.0112, 1., 1.0291],
            ),
            (
                unit("Canon", "EOS 800D", daylight([1876., 1024., 1623.])),
                [1.0091, 1., 1.0456],
            ),
            (
                unit("Fujifilm", "X-T5", d65([658., 302., 473.])),
                [0.9347, 1., 1.055],
            ),
            (
                unit("Fujifilm", "X-T5", d65([646., 302., 494.])),
                [0.952, 1., 1.0101],
            ),
            // Lightroom's DNGs of the owner's X100F, and another X100F.
            (
                unit("Fujifilm", "X100F", d65([681., 302., 484.])),
                [0.9883, 1., 1.031],
            ),
            (
                unit("Fujifilm", "X100F", d65([679., 302., 500.])),
                [0.9912, 1., 0.998],
            ),
            // Bayer bodies' doubled multipliers give the same ratios.
            (
                unit("Fujifilm", "X-T200", d65([1118., 512., 852.])),
                [0.9338, 1., 0.9695],
            ),
        ] {
            assert_eq!(neutral_calibration(&m), expected, "{} {}", m.make, m.model);
        }
        // Some Canon bodies' reference is their Shade preset, not Daylight.
        let presets = WhiteBalancePresets {
            daylight: Some([1827., 1024., 1686.]),
            shade: Some([2106., 1024., 1464.]),
            ..Default::default()
        };
        assert_eq!(
            neutral_calibration(&unit("Canon", "EOS 200D", presets)),
            [1.0247, 1., 1.026]
        );
        // Without the row's preset, identity rather than another unit's correction.
        let other = daylight([658., 302., 473.]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X-T5", other)),
            [1.; 3]
        );
        // Signs that would cancel in the ratios are still invalid.
        let negative = d65([-681., -302., -484.]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X100F", negative)),
            [1.; 3]
        );
        let none = WhiteBalancePresets::default();
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X100F", none)),
            [1.; 3]
        );
        // Adobe calibrates neither Nikon bodies nor these Fujifilm ones.
        let nikon = daylight([2000., 1024., 1500.]);
        assert_eq!(neutral_calibration(&unit("Nikon", "Z 6_2", nikon)), [1.; 3]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X-T50", d65([644., 302., 498.]))),
            [1.; 3]
        );
    }
    #[test]
    fn baseline_covers_measured_cameras() {
        assert_eq!(baseline_exposure(&camera("SONY", "ILCE-7M2")), 0.35);
        assert_eq!(baseline_exposure(&camera("Sony", "ILCE-7M4")), 0.35);
        assert_eq!(baseline_exposure(&camera("Canon", "EOS R7")), 0.4);
        assert_eq!(baseline_exposure(&camera("Nikon", "Z 30")), 0.3);
        // An unlisted body follows its make.
        assert_eq!(
            baseline_exposure(&camera("Canon", "EOS R1")),
            crate::cameras::baseline_exposure("Canon", "EOS R1").ev
        );
        // The Canon median, 0.15 EV with the older bodies' rows.
        assert!(baseline_exposure(&camera("Canon", "EOS R1")) > 0.1);
    }
    #[test]
    fn canon_highlight_tone_priority_adds_a_stop() {
        let mut m = camera("Canon", "EOS R8");
        let normal = baseline_exposure(&m);
        m.highlight_tone_priority = HighlightTonePriority::On;
        assert_eq!(baseline_exposure(&m), normal + 1.);
        m.highlight_tone_priority = HighlightTonePriority::Enhanced;
        assert_eq!(baseline_exposure(&m), normal + 1.);
        // A DNG's own value already includes it.
        m.baseline_exposure = Some(1.27);
        assert_eq!(baseline_exposure(&m), 1.27);
    }
    #[test]
    fn dngs_use_their_own_baseline() {
        let mut dng = camera("Sony", "ILCE-7M4");
        dng.baseline_exposure = Some(-0.2);
        assert_eq!(baseline_exposure(&dng), -0.2);
        dng.baseline_exposure = Some(0.);
        assert_eq!(baseline_exposure(&dng), 0.);
    }
}
