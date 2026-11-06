//! How an HDR export encodes an HDR master (`rendered::HdrImage`: linear ProPhoto,
//! 1 = graphics white): its gamut and transfer function, chosen apart, and the
//! encoding arithmetic (docs/hdr.md#hdr-export).
//!
//! Graphics white is 203 cd/m² (ITU-R BT.2408) in every encoding: PQ writes absolute
//! luminance, HLG maps 203 cd/m² on its 1,000 cd/m² reference display to signal 0.75,
//! and linear float keeps 1.0 = graphics white, as Photoshop's 32-bit documents do.
// Published constants (matrices, ST 2084 and BT.2100) kept as they are given.
#![allow(clippy::excessive_precision)]
use serde::{Deserialize, Serialize};

/// Graphics white, in cd/m².
pub const GRAPHICS_WHITE_NITS: f32 = 203.;
/// HLG's nominal peak: BT.2100's reference display, at which its system gamma is 1.2.
pub const HLG_PEAK_NITS: f32 = 1000.;

/// An HDR export's primaries, all with D65 white.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrGamut {
    #[default]
    Rec2020,
    DisplayP3,
    Rec709,
}

/// An HDR export's transfer function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrTransfer {
    /// SMPTE ST 2084, absolute luminance to 10,000 cd/m².
    #[default]
    Pq,
    /// ARIB STD-B67 / BT.2100 HLG, for a 1,000 cd/m² reference display.
    Hlg,
    /// Linear light in 32-bit float, 1.0 = graphics white.
    Linear,
}

/// An HDR export's encoding. A field only a later release knows makes it unreadable
/// here, so an export settings file keeps it as that release's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HdrOutput {
    pub gamut: HdrGamut,
    pub transfer: HdrTransfer,
    /// A gain map (ISO 21496-1) over the SDR rendition, in place of HDR pixels: the
    /// file shows its SDR base where a reader or display has no HDR, and the master
    /// where it has. `transfer` does not apply; the base is sRGB, and `gamut` must be
    /// Rec. 709. Omitted when off, so the encodings saved before it read the same.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gain_map: bool,
}

/// Linear ProPhoto RGB (D50) to linear XYZ D65: ProPhoto's matrix and Bradford's
/// adaptation, from the published chromaticities.
const PROPHOTO_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.755_603_2, 0.112_784_94, 0.082_081_84],
    [0.268_337_96, 0.715_126_76, 0.016_535_34],
    [0.003_910_04, -0.012_918_7, 1.097_838_7],
];

impl HdrGamut {
    pub const ALL: [Self; 3] = [Self::Rec2020, Self::DisplayP3, Self::Rec709];
    pub fn label(self) -> &'static str {
        match self {
            Self::Rec2020 => "Rec. 2020",
            Self::DisplayP3 => "Display P3",
            Self::Rec709 => "Rec. 709",
        }
    }
    /// The gamut's CICP colour primaries (ITU-T H.273).
    pub fn cicp(self) -> u8 {
        match self {
            Self::Rec2020 => 9,
            Self::DisplayP3 => 12,
            Self::Rec709 => 1,
        }
    }
    /// Linear XYZ D65 to this gamut's linear RGB.
    fn xyz_to_rgb(self) -> [[f32; 3]; 3] {
        match self {
            Self::Rec2020 => [
                [1.716_651_2, -0.355_670_8, -0.253_366_3],
                [-0.666_684_35, 1.616_481_2, 0.015_768_545],
                [0.017_639_857, -0.042_770_613, 0.942_103_1],
            ],
            Self::DisplayP3 => [
                [2.493_497, -0.931_383_6, -0.402_710_8],
                [-0.829_489, 1.762_664_1, 0.023_624_686],
                [0.035_845_83, -0.076_172_39, 0.956_884_5],
            ],
            Self::Rec709 => [
                [3.240_97, -1.537_383_2, -0.498_610_76],
                [-0.969_243_6, 1.875_967_5, 0.041_555_06],
                [0.055_630_08, -0.203_976_96, 1.056_971_5],
            ],
        }
    }
    /// Linear ProPhoto RGB (D50) to this gamut's linear RGB.
    pub fn prophoto_to_rgb(self) -> [[f32; 3]; 3] {
        super::mul_matrix(self.xyz_to_rgb(), PROPHOTO_TO_XYZ_D65)
    }
    /// The gamut whose CICP colour primaries are `code` (ITU-T H.273), among those
    /// HDR files are decoded in: BT.709, BT.2020 and Display P3 (SMPTE EG 432-1).
    pub fn from_cicp(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.cicp() == code)
    }
    /// The luminance weights of this gamut's linear RGB.
    pub fn luminance(self) -> [f32; 3] {
        let m = super::inverse(self.xyz_to_rgb());
        m[1]
    }
}

impl HdrTransfer {
    pub const ALL: [Self; 3] = [Self::Pq, Self::Hlg, Self::Linear];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pq => "PQ",
            Self::Hlg => "HLG",
            Self::Linear => "Linear (32-bit float)",
        }
    }
    /// The transfer's CICP transfer characteristics (ITU-T H.273); linear is 8.
    pub fn cicp(self) -> u8 {
        match self {
            Self::Pq => 16,
            Self::Hlg => 18,
            Self::Linear => 8,
        }
    }
}

impl HdrOutput {
    /// Encodes one pixel of an HDR master (linear ProPhoto, 1 = graphics white) whose
    /// channels may reach `peak`: in the gamut, out-of-gamut channels clipped at 0 and
    /// each held to `peak` by the HDR limit's roll-off (a saturated colour can have a
    /// channel above it here that it had not in ProPhoto), then the transfer: PQ and
    /// HLG to 0–1 signal values, linear unchanged.
    pub fn encode(self, prophoto: [f32; 3], peak: f32) -> [f32; 3] {
        let rgb = super::mul(self.gamut.prophoto_to_rgb(), prophoto).map(|v| {
            if v.is_finite() {
                roll_off(v.max(0.), peak)
            } else {
                0.
            }
        });
        match self.transfer {
            HdrTransfer::Pq => rgb.map(|v| pq_encode(v * GRAPHICS_WHITE_NITS)),
            HdrTransfer::Hlg => hlg_encode(rgb, self.gamut.luminance()),
            HdrTransfer::Linear => rgb,
        }
    }
}

impl HdrOutput {
    /// The light an encoded pixel holds: its transfer inverted, then its gamut's
    /// linear RGB converted to linear ProPhoto (D50), 1 = graphics white, signed
    /// values kept. The inverse of [`HdrOutput::encode`] inside what it represents;
    /// what `encode` clipped (out-of-gamut channels, PQ above 10,000 cd/m², HLG
    /// channels above 1 after its inverse OOTF) and rolled off doesn't come back.
    pub fn decode(self, signal: [f32; 3]) -> [f32; 3] {
        let rgb = match self.transfer {
            HdrTransfer::Pq => signal.map(|v| pq_decode(v) / GRAPHICS_WHITE_NITS),
            HdrTransfer::Hlg => hlg_decode(signal, self.gamut.luminance()),
            HdrTransfer::Linear => signal,
        };
        super::mul(super::inverse(self.gamut.prophoto_to_rgb()), rgb)
    }
}

/// BT.2100 HLG signal `rgb` as display light, 1 = graphics white: the inverse OETF
/// to scene light, then the OOTF of HLG's reference display (system gamma 1.2 on
/// the luminance `weights` give, in the signal's own primaries), the inverse of
/// [`hlg_encode`].
pub fn hlg_decode(rgb: [f32; 3], weights: [f32; 3]) -> [f32; 3] {
    const GAMMA: f32 = 1.2;
    let scene = rgb.map(hlg_inverse_oetf);
    let y: f32 = (0..3).map(|c| weights[c] * scene[c]).sum();
    let gain = if y > 0. { y.powf(GAMMA - 1.) } else { 0. };
    scene.map(|v| v * gain * HLG_PEAK_NITS / GRAPHICS_WHITE_NITS)
}

/// Where the HDR limit's roll-off starts, as a fraction of the peak. Camera Raw 18.7
/// renders a linear ramp unchanged to just below 2^limit × graphics white and scene
/// 16 at limit 4 as 15.84: a knee at 97% with slope 1 reproduces both.
pub const LIMIT_KNEE: f32 = 0.97;

/// `v` (linear, 1 = graphics white) rolled off to at most `peak`: the HDR limit
/// (`model::hdr`) as an HDR render and its export apply it to each channel, last. Values below the knee pass unchanged.
pub fn roll_off(v: f32, peak: f32) -> f32 {
    let knee = LIMIT_KNEE * peak;
    if v.is_nan() {
        0.
    } else if v <= knee {
        v
    } else {
        let room = peak - knee;
        peak - room * (-(v - knee) / room).exp()
    }
}

/// SMPTE ST 2084's inverse EOTF: absolute luminance (cd/m²) to signal (0–1).
pub fn pq_encode(nits: f32) -> f32 {
    let (m1, m2) = (2610. / 16384., 2523. / 4096. * 128.);
    let (c1, c2, c3) = (3424. / 4096., 2413. / 128., 2392. / 128.);
    let y = (nits / 10_000.).clamp(0., 1.);
    let p = y.powf(m1);
    ((c1 + c2 * p) / (1. + c3 * p)).powf(m2)
}

/// SMPTE ST 2084's EOTF: signal (0–1) to absolute luminance (cd/m²).
pub fn pq_decode(signal: f32) -> f32 {
    let (m1, m2) = (2610. / 16384., 2523. / 4096. * 128.);
    let (c1, c2, c3) = (3424. / 4096., 2413. / 128., 2392. / 128.);
    let p = signal.clamp(0., 1.).powf(1. / m2);
    10_000. * ((p - c1).max(0.) / (c2 - c3 * p)).powf(1. / m1)
}

/// BT.2100 HLG of display light `rgb` (linear, 1 = graphics white), for its reference
/// display of [`HLG_PEAK_NITS`]: the inverse OOTF with system gamma 1.2 (on the
/// luminance given by `weights`) to scene light, then the OETF. Light above the
/// reference display's peak is clipped.
pub fn hlg_encode(rgb: [f32; 3], weights: [f32; 3]) -> [f32; 3] {
    const GAMMA: f32 = 1.2;
    let display = rgb.map(|v| (v * GRAPHICS_WHITE_NITS / HLG_PEAK_NITS).clamp(0., 1.));
    let y: f32 = (0..3).map(|c| weights[c] * display[c]).sum();
    let scene = if y > 0. {
        let gain = y.powf((1. - GAMMA) / GAMMA);
        display.map(|v| (v * gain).min(1.))
    } else {
        [0.; 3]
    };
    scene.map(hlg_oetf)
}

/// BT.2100 HLG's OETF: normalised scene light (0–1) to signal (0–1).
pub fn hlg_oetf(e: f32) -> f32 {
    let (a, b) = (0.178_832_77, 0.284_668_92);
    let c = 0.559_910_73;
    if e <= 1. / 12. {
        (3. * e).sqrt()
    } else {
        a * (12. * e - b).ln() + c
    }
}

/// BT.2100 HLG's inverse OETF: a signal (0–1) to normalised scene light (0–1).
pub fn hlg_inverse_oetf(signal: f32) -> f32 {
    let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
    let e = signal.clamp(0., 1.);
    if e <= 0.5 {
        e * e / 3.
    } else {
        (((e - c) / a).exp() + b) / 12.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_white_lands_on_each_transfers_reference() {
        let white = [1.; 3];
        let pq = HdrOutput {
            gamut: HdrGamut::Rec2020,
            transfer: HdrTransfer::Pq,
            gain_map: false,
        };
        // BT.2408: 203 cd/m² is PQ 0.5806 and HLG 0.75.
        for v in pq.encode(white, 16.) {
            assert!((v - 0.5806).abs() < 1e-3, "{v}");
        }
        let hlg = HdrOutput {
            transfer: HdrTransfer::Hlg,
            ..pq
        };
        for v in hlg.encode(white, 16.) {
            assert!((v - 0.75).abs() < 2e-3, "{v}");
        }
        let linear = HdrOutput {
            transfer: HdrTransfer::Linear,
            ..pq
        };
        for v in linear.encode(white, 16.) {
            assert!((v - 1.).abs() < 2e-3, "{v}");
        }
    }

    /// Light inside what each encoding represents comes back from it: in the gamut,
    /// below the knee, under PQ's ceiling and, for HLG, with every channel at most 1
    /// after its inverse OOTF. Pure Rec. 2020 red at 4× white is past that last
    /// bound in HLG: it clips there, and comes back dimmer.
    #[test]
    fn decoding_undoes_encoding_inside_the_encodings_domain() {
        let to_prophoto = |gamut: HdrGamut, rgb: [f32; 3]| {
            super::super::mul(super::super::inverse(gamut.prophoto_to_rgb()), rgb)
        };
        for gamut in HdrGamut::ALL {
            for transfer in HdrTransfer::ALL {
                let output = HdrOutput {
                    gamut,
                    transfer,
                    ..Default::default()
                };
                for rgb in [
                    [0.18; 3],
                    [1.; 3],
                    [3., 3., 3.],
                    [0.6, 0.3, 0.1],
                    [0.002; 3],
                ] {
                    let light = to_prophoto(gamut, rgb);
                    let back = output.decode(output.encode(light, 16.));
                    for c in 0..3 {
                        assert!(
                            (back[c] - light[c]).abs() <= 1e-3 * light[c].abs().max(0.01),
                            "{gamut:?} {transfer:?} {rgb:?}: {back:?} against {light:?}"
                        );
                    }
                }
            }
        }
        let hlg = HdrOutput {
            gamut: HdrGamut::Rec2020,
            transfer: HdrTransfer::Hlg,
            ..Default::default()
        };
        let red = to_prophoto(HdrGamut::Rec2020, [4., 0., 0.]);
        let back = hlg.gamut.prophoto_to_rgb();
        let back = super::super::mul(back, hlg.decode(hlg.encode(red, 16.)));
        assert!(
            (back[0] - 3.77).abs() < 0.02 && back[1].abs() < 1e-3,
            "{back:?}"
        );
    }

    #[test]
    fn hlg_inverse_oetf_undoes_the_oetf() {
        for e in [0., 0.01, 1. / 12., 0.2, 0.5, 1.] {
            let back = hlg_inverse_oetf(hlg_oetf(e));
            assert!((back - e).abs() < 1e-5, "{e}: {back}");
        }
    }

    #[test]
    fn pq_round_trips_and_reaches_its_ceiling() {
        for nits in [0.005, 1., 100., 203., 1000., 4000., 10_000.] {
            let back = pq_decode(pq_encode(nits));
            assert!((back / nits - 1.).abs() < 1e-3, "{nits}: {back}");
        }
        assert_eq!(pq_encode(20_000.), 1.);
        assert!(pq_encode(-1.) < 1e-6 && pq_encode(-1.) == pq_encode(0.));
    }

    #[test]
    fn hlg_clips_at_its_reference_peak_and_keeps_neutrals_neutral() {
        let w = HdrGamut::Rec2020.luminance();
        assert!((w.iter().sum::<f32>() - 1.).abs() < 1e-3);
        let peak = hlg_encode([HLG_PEAK_NITS / GRAPHICS_WHITE_NITS; 3], w);
        assert!(peak.iter().all(|v| (v - 1.).abs() < 1e-3), "{peak:?}");
        let over = hlg_encode([20.; 3], w);
        assert!(over.iter().all(|v| (v - 1.).abs() < 1e-3));
        let grey = hlg_encode([0.18; 3], w);
        assert!(grey[0] == grey[1] && grey[1] == grey[2]);
    }

    #[test]
    fn the_gamuts_keep_prophoto_white_neutral() {
        for gamut in HdrGamut::ALL {
            let white = super::super::mul(gamut.prophoto_to_rgb(), [1.; 3]);
            assert!(
                white.iter().all(|v| (v - 1.).abs() < 2e-3),
                "{gamut:?} {white:?}"
            );
        }
    }
}
