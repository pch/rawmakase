//! RAWmakase's camera table, `data/cameras.toml`: per-model values that the raw
//! file does not carry, one row per camera body. See docs/cameras.md.
use crate::camera_data::{WhiteBalancePreset, WhiteBalancePresets};
use serde::Deserialize;
use std::sync::OnceLock;

/// Where a row's values come from.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Read from a DNG that Adobe software wrote for this camera.
    AdobeDng,
    /// Read from the camera's Adobe DCP.
    AdobeProfile,
    /// Fitted to Camera Raw renders.
    Fitted,
    /// Measured by hand, as the row's `how` says.
    Measured,
}

/// Adobe DNG Converter records a diagonal CameraCalibration, equal for both
/// illuminants, that is this model reference divided by the RAW's own `preset`,
/// each normalized to green. The preset differs between units of one model, so
/// the calibration does too; see docs/camera-parity.md.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UnitCalibration {
    pub preset: WhiteBalancePreset,
    pub reference: [f32; 3],
}

/// Whether Camera Raw moves a Fujifilm body's baseline with the raw's exposure
/// midpoint shift (DR200/DR400, extended ISO).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExposureShift {
    /// Every body measured so far after the X-Trans III generation.
    #[default]
    Followed,
    /// One value whatever the shift.
    Ignored,
}

/// One camera body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Make and model as the decoder names them.
    pub make: String,
    pub model: String,
    /// Other model names for the same body.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Exposure Camera Raw adds to an unedited photo with Adobe Standard, in EV.
    pub baseline_exposure: f32,
    /// Adobe's calibration of an individual camera unit, from the preset that unit
    /// stored in its RAW. Only meaningful with the Adobe profile calibration signature.
    #[serde(default)]
    pub unit_calibration: Option<UnitCalibration>,
    /// Fujifilm only: see [`ExposureShift`].
    #[serde(default)]
    pub fujifilm_exposure_shift: ExposureShift,
    /// Fujifilm only: the body's exposure midpoint shift at DR100 and base ISO, which
    /// the row holds at, when it is not the sensor's usual one (-0.72 EV for X-Trans,
    /// 0 for Bayer); the GFX bodies record -0.49.
    #[serde(default)]
    pub fujifilm_dr100_shift: Option<f32>,
    /// The body's lowest native ISO, when it has extended ISO settings below it
    /// (Sony's 50, 64 and 80 under 100): Camera Raw takes a stop off there.
    #[serde(default)]
    pub lowest_iso: Option<f32>,
    /// Canon only: the baseline of the body's sRAW and mRAW, Adobe's small RAW
    /// BaselineExposure, which differs from the body's.
    #[serde(default)]
    pub sraw_baseline_exposure: Option<f32>,
    /// The black and white level of the sample `baseline_exposure` was read
    /// from, as Adobe wrote them, and its samples' maximum: a body whose white
    /// Adobe takes by ISO speed (Canon) moves its BaselineExposure with the
    /// white, so a photo of other levels gets the difference
    /// (`camera_profiles::reference`).
    #[serde(default)]
    pub levels: Option<[f32; 3]>,
    pub source: Source,
    /// When the values were checked (YYYY-MM-DD) and on what.
    pub checked: String,
    pub sample: String,
    /// How a measured value was measured.
    #[serde(default)]
    pub how: String,
}

impl Camera {
    fn is(&self, model: &str) -> bool {
        self.model.eq_ignore_ascii_case(model)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(model))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    camera: Vec<Camera>,
}

const TABLE: &str = include_str!("../../../data/cameras.toml");

/// `data/adobe-camera-names.toml`: names Adobe's camera profiles use for cameras
/// the decoder names otherwise, beyond what its canonical names cover.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdobeNames {
    make: Vec<MakeName>,
    camera: Vec<CameraName>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MakeName {
    libraw: String,
    adobe: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CameraName {
    make: String,
    model: String,
    adobe: String,
    #[allow(dead_code)]
    why: String,
}

const ADOBE_NAMES: &str = include_str!("../../../data/adobe-camera-names.toml");

fn adobe_table() -> &'static AdobeNames {
    static NAMES: OnceLock<AdobeNames> = OnceLock::new();
    // A unit test parses the table, so a malformed row cannot ship.
    NAMES.get_or_init(|| toml::from_str(ADOBE_NAMES).expect("data/adobe-camera-names.toml"))
}

/// Names, as "Make Model", that Adobe's camera profiles may use for a camera the
/// decoder reports as `make` `model`, other than that one: the decoder's canonical
/// name for a body sold under several names (`normalized`), the same with Adobe's
/// spelling of the make, the table's names, and older Google phones'
/// "Model-Google-google".
pub fn adobe_names<'a>(
    make: &'a str,
    model: &'a str,
    normalized: (&'a str, &'a str),
) -> Vec<String> {
    let table = adobe_table();
    // The decoder leaves its canonical names empty for makers it doesn't know.
    let pick = |n: &'a str, own: &'a str| if n.trim().is_empty() { own } else { n };
    let normalized = (pick(normalized.0, make), pick(normalized.1, model));
    let mut names = vec![format!("{} {}", normalized.0, normalized.1)];
    for m in table
        .make
        .iter()
        .filter(|m| m.libraw.eq_ignore_ascii_case(make))
    {
        names.push(format!("{} {model}", m.adobe));
        names.push(format!("{} {}", m.adobe, normalized.1));
    }
    names.extend(
        table
            .camera
            .iter()
            .filter(|c| c.make.eq_ignore_ascii_case(make) && c.model.eq_ignore_ascii_case(model))
            .map(|c| c.adobe.clone()),
    );
    if make.eq_ignore_ascii_case("Google") {
        names.push(format!("{model}-Google-google"));
    }
    let own = format!("{make} {model}");
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.trim().to_string();
        if !n.is_empty()
            && !n.eq_ignore_ascii_case(&own)
            && !out.iter().any(|o| o.eq_ignore_ascii_case(&n))
        {
            out.push(n);
        }
    }
    out
}

fn parse(text: &str) -> Result<Vec<Camera>, toml::de::Error> {
    toml::from_str::<Table>(text).map(|t| t.camera)
}

/// Every row of the table.
pub fn all() -> &'static [Camera] {
    static CAMERAS: OnceLock<Vec<Camera>> = OnceLock::new();
    // A unit test parses the table, so a malformed row cannot ship.
    CAMERAS.get_or_init(|| parse(TABLE).expect("data/cameras.toml"))
}

/// The row for a camera, matching make and model (or an alias) without case.
fn find_in<'a>(rows: &'a [Camera], make: &str, model: &str) -> Option<&'a Camera> {
    rows.iter()
        .find(|c| c.make.eq_ignore_ascii_case(make) && c.is(model))
}

/// Where a baseline exposure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineOrigin {
    /// The camera's own row.
    Listed(Source),
    /// The median of the rows of the camera's make.
    MakeMedian,
    /// The median of every row: the make has none.
    TableMedian,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Baseline {
    pub ev: f32,
    pub origin: BaselineOrigin,
    pub exposure_shift: ExposureShift,
    pub dr100_shift: Option<f32>,
    pub lowest_iso: Option<f32>,
    /// The baseline of a small raw (sRAW, mRAW): the row's, else the median of the
    /// make's, else `ev`.
    pub sraw_ev: f32,
    /// The black, white and samples' maximum `ev` holds at, for a row that
    /// says them.
    pub levels: Option<[f32; 3]>,
}

/// Camera Raw's baseline exposure for a camera. A camera without a row takes the
/// median of its make's rows, which predicts held-out cameras better than the median
/// of all rows (see docs/cameras.md); a make without rows takes that overall median.
pub fn baseline_exposure(make: &str, model: &str) -> Baseline {
    baseline_in(all(), make, model)
}

/// Exact body calibration only: unlike exposure, this must never use a make median.
/// Identity when the row has no reference or the RAW lacks a valid preset; another
/// unit's correction is never borrowed.
pub fn neutral_calibration(make: &str, model: &str, presets: &WhiteBalancePresets) -> [f32; 3] {
    let Some(unit) = find_in(all(), make, model).and_then(|row| row.unit_calibration) else {
        return [1.; 3];
    };
    let Some(wb) = presets
        .get(unit.preset)
        .and_then(WhiteBalancePresets::valid)
    else {
        return [1.; 3];
    };
    let reference = unit.reference;
    let gains = std::array::from_fn(|c| {
        let gain = (f64::from(reference[c]) / f64::from(reference[1]))
            / (f64::from(wb[c]) / f64::from(wb[1]));
        // DNG Converter records these diagonal coefficients to four decimals.
        ((gain * 10000.).round() / 10000.) as f32
    });
    if gains.iter().all(|v| v.is_finite() && *v > 0. && *v < 4.) {
        gains
    } else {
        [1.; 3]
    }
}

fn baseline_in(rows: &[Camera], make: &str, model: &str) -> Baseline {
    let same_make = || rows.iter().filter(|c| c.make.eq_ignore_ascii_case(make));
    let sraw_median = median(same_make().filter_map(|c| c.sraw_baseline_exposure));
    if let Some(c) = find_in(rows, make, model) {
        return Baseline {
            ev: c.baseline_exposure,
            origin: BaselineOrigin::Listed(c.source),
            exposure_shift: c.fujifilm_exposure_shift,
            dr100_shift: c.fujifilm_dr100_shift,
            lowest_iso: c.lowest_iso,
            sraw_ev: c
                .sraw_baseline_exposure
                .or(sraw_median)
                .unwrap_or(c.baseline_exposure),
            levels: c.levels,
        };
    }
    if let Some(ev) = median(same_make().map(|c| c.baseline_exposure)) {
        return Baseline {
            ev,
            origin: BaselineOrigin::MakeMedian,
            exposure_shift: ExposureShift::Followed,
            dr100_shift: None,
            lowest_iso: None,
            sraw_ev: sraw_median.unwrap_or(ev),
            levels: None,
        };
    }
    let ev = median(rows.iter().map(|c| c.baseline_exposure)).unwrap_or(0.);
    Baseline {
        ev,
        origin: BaselineOrigin::TableMedian,
        exposure_shift: ExposureShift::Followed,
        dr100_shift: None,
        lowest_iso: None,
        sraw_ev: ev,
        levels: None,
    }
}

fn median(values: impl Iterator<Item = f32>) -> Option<f32> {
    let mut v: Vec<f32> = values.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adobe_names_table_parses_and_names_cameras_otherwise() {
        let table = adobe_table();
        assert!(!table.make.is_empty() && !table.camera.is_empty());
        for c in &table.camera {
            assert!(
                !c.why.is_empty()
                    && !c
                        .adobe
                        .eq_ignore_ascii_case(&format!("{} {}", c.make, c.model))
            );
        }
        // LibRaw's canonical name, the make Adobe spells otherwise, the table.
        assert_eq!(
            adobe_names("Panasonic", "DC-G95", ("Panasonic", "DC-G99")),
            ["Panasonic DC-G99"]
        );
        assert_eq!(
            adobe_names("OM Digital", "OM-1", ("OM Digital", "OM-1")),
            ["OM Digital Solutions OM-1"]
        );
        assert_eq!(
            adobe_names("Samsung", "SM-G965F", ("Samsung", "SM-G965F")),
            ["Samsung Galaxy S9+"]
        );
        assert_eq!(
            adobe_names("Google", "Pixel 2 XL", ("Google", "Pixel 2 XL")),
            ["Pixel 2 XL-Google-google"]
        );
        // Nothing for a camera Adobe names as LibRaw does, or one LibRaw doesn't know.
        assert!(adobe_names("RAWmakase", "Synthetic Chart", ("", "")).is_empty());
        assert!(adobe_names("Sony", "ILCE-7M4", ("Sony", "ILCE-7M4")).is_empty());
    }
    #[test]
    fn every_row_parses_and_is_complete() {
        let rows = parse(TABLE).unwrap();
        assert!(!rows.is_empty());
        let mut names = std::collections::BTreeSet::new();
        for c in &rows {
            let name = format!("{} {}", c.make, c.model);
            assert!(!c.make.is_empty() && !c.model.is_empty(), "{name}");
            assert!(c.baseline_exposure.abs() <= 3., "{name}");
            if let Some(u) = c.unit_calibration {
                assert!(
                    u.reference
                        .iter()
                        .all(|v| v.is_finite() && *v > 0. && *v < 65536. && v.fract() == 0.),
                    "{name}: a reference is the preset's integer multipliers"
                );
            }
            let d: Vec<&str> = c.checked.split('-').collect();
            assert!(
                d.len() == 3 && d.iter().all(|p| p.parse::<u32>().is_ok()) && d[0].len() == 4,
                "{name}: checked must be YYYY-MM-DD"
            );
            assert!(!c.sample.is_empty(), "{name}: say what it was checked on");
            assert!(
                c.source != Source::Measured || !c.how.is_empty(),
                "{name}: a measured row says how"
            );
            for model in std::iter::once(&c.model).chain(&c.aliases) {
                assert!(
                    names.insert(format!("{} {}", c.make, model).to_lowercase()),
                    "{} {model} is listed twice",
                    c.make
                );
            }
        }
    }
    #[test]
    fn unknown_fields_are_rejected() {
        let row = "[[camera]]\nmake = \"A\"\nmodel = \"B\"\nbaseline_exposure = 0.1\n\
                   source = \"fitted\"\nchecked = \"2026-10-05\"\nsample = \"s\"\n";
        assert!(parse(row).is_ok());
        assert!(parse(&format!("{row}baseline = 0.2\n")).is_err());
        assert!(parse(&row.replace("fitted", "guessed")).is_err());
    }
    fn row(make: &str, model: &str, ev: f32) -> Camera {
        Camera {
            make: make.into(),
            model: model.into(),
            aliases: vec![],
            baseline_exposure: ev,
            unit_calibration: None,
            fujifilm_exposure_shift: ExposureShift::Followed,
            fujifilm_dr100_shift: None,
            lowest_iso: None,
            sraw_baseline_exposure: None,
            levels: None,
            source: Source::Fitted,
            checked: "2026-10-05".into(),
            sample: "test".into(),
            how: String::new(),
        }
    }
    #[test]
    fn unlisted_cameras_take_their_makes_median_then_the_tables() {
        let mut rows = vec![
            row("Sony", "A", 0.2),
            row("Sony", "B", 0.4),
            row("Sony", "C", 0.35),
            row("Canon", "D", 0.6),
        ];
        rows[0].aliases = vec!["Alpha".into()];
        let listed = baseline_in(&rows, "SONY", "alpha");
        assert_eq!(listed.ev, 0.2);
        assert_eq!(listed.origin, BaselineOrigin::Listed(Source::Fitted));
        let make = baseline_in(&rows, "Sony", "E");
        assert_eq!((make.ev, make.origin), (0.35, BaselineOrigin::MakeMedian));
        let table = baseline_in(&rows, "Pentax", "K-3");
        assert_eq!(
            (table.ev, table.origin),
            (0.375, BaselineOrigin::TableMedian)
        );
        assert_eq!(baseline_in(&[], "Pentax", "K-3").ev, 0.);
    }
}
