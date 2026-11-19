//! RAWmakase's record in an editable preview's DNGPrivateData (tag 50740): the
//! original's facts as its decode had them, which a DNG's standard tags carry
//! only in part or only approximately. This module is the format's one
//! definition and its reader; `rawmakase_export::export::editable_preview`
//! writes it, from the same types.
//!
//! The record holds no identity of the original: which file a copy was built
//! from is the catalog's row's to say (`catalog::editable_previews`), and the
//! copy's file name, which the catalog gives it from that identity, ties the
//! file to the row.
//!
//! # Layout
//!
//! `RAWmakase\0`, as DNGPrivateData records begin with their maker's name; the
//! format version, a big-endian `u16` (Adobe's own records are big-endian); then
//! the fields, as one JSON object, to the end of the tag's bytes.
//!
//! Every field is written by name, and one missing reads as the default stated
//! beside it in [`Record`]. Fields are this record's own, never a serialised
//! internal struct, so a change to [`Metadata`] or the lens types doesn't change
//! the format: the writer's conversions destructure them whole, so a field added
//! there doesn't compile until it is given a field here or listed as not stored.
//!
//! A record of a version this release doesn't know is refused, never read: the
//! copy is out of date, and is built again while the original is online. A later
//! release that adds a field whose absence would change how the copy renders
//! raises the version; one whose absence changes nothing (a fact shown, not
//! rendered) may keep it, and this release ignores the field. A value this
//! release doesn't know where it reads a name (a matrix source, say) is refused
//! too.
use anyhow::{Context, Result, bail, ensure};
use rawmakase_model::camera_data::{
    EmbeddedProfileStatus, HighlightTonePriority, MatrixSource, Metadata, NeutralCalibration,
    Source, WhiteBalancePresets,
};
use rawmakase_model::camera_profiles::{CameraProfile, fitted};
use rawmakase_model::optics::{GainGrid, LensCorrection, Radial};
use serde::{Deserialize, Serialize};

/// What every record starts with.
pub const MAGIC: &[u8; 10] = b"RAWmakase\0";
/// The format version this release writes and the only one it reads.
pub const VERSION: u16 = 1;
/// The largest record read: well beyond what is written (a DNG's embedded
/// profile and a fitted Camera Look are the large parts, together about
/// 2 MB at most), yet small enough that a damaged count can't ask for a huge
/// allocation.
pub const MAX_RECORD: u64 = 16 << 20;

/// Version 1's fields. Each missing field reads as its type's default, which is
/// [`Metadata::default`]'s unless said otherwise; the ones marked required are
/// checked when read ([`Record::parse`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The original's full decoded width and height, which the copy develops as
    /// a reduced image of. Required.
    pub width: u32,
    pub height: u32,
    /// The copy's own size, which its DNG decodes to. Required.
    pub copy_width: u32,
    pub copy_height: u32,
    pub raw_width: u32,
    pub raw_height: u32,
    /// The camera's default crop as `[left, top, width, height]` in the
    /// original's pixels, exactly as its decoder gave it, including the values
    /// a decoder writes for none.
    pub crop: [u32; 4],
    pub camera_crop: Option<[f32; 4]>,
    pub make: String,
    pub model: String,
    pub exif_make: String,
    pub exif_model: String,
    pub sensor_crop: Option<f32>,
    pub flip: i32,
    pub xtrans: bool,
    pub sraw: bool,
    pub monochrome: bool,
    pub fuji_dynamic_range: u32,
    pub fuji_exposure_shift: Option<f32>,
    pub highlight_tone_priority: Htp,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal: f32,
    pub focal_35mm: f32,
    /// In metres; 0 when unknown.
    pub focus_distance: f32,
    /// The camera focused at infinity, which JSON can't write as a number.
    pub focus_at_infinity: bool,
    pub wb: [f32; 3],
    pub daylight_wb: [f32; 3],
    pub wb_presets: Presets,
    /// Camera to sRGB, row by row.
    pub matrix: [[f32; 3]; 3],
    /// XYZ (D65) to camera, row by row.
    pub cam_xyz: [[f32; 3]; 3],
    pub matrix_source: MatrixFrom,
    pub camera_look: Option<String>,
    pub embedded_profile_status: ProfileStatus,
    /// A DNG original's embedded camera profile, in DCP form, as hexadecimal.
    pub embedded_profile: Option<String>,
    pub lens: Option<Lens>,
    pub lens_stand_in: Option<Lens>,
    pub lens_model: String,
    pub lens_id: u32,
    pub lens_info: Option<[f32; 4]>,
    pub adobe_names: Vec<String>,
    pub baseline_exposure: Option<f32>,
    pub dng_neutral_calibration: Option<Calibration>,
    pub dng_analog_balance: Option<[f32; 3]>,
    pub dng_matrix_profile_signature: Option<String>,
    /// Black, white and maximum of the sensor data.
    pub levels: Option<[u32; 3]>,
    /// Auto CA as measured on the original's full-size decode. Missing: not
    /// measured, so the copy measures its own.
    pub auto_ca: Option<AutoCa>,
    /// The photo's default Camera Look (fitted), for a camera without colour
    /// data, as an XMP sidecar carries it ([`fitted::carried`]). Its fit needs
    /// the original's embedded JPEG, so offline it comes only from here.
    pub default_look: Option<String>,
}

/// The camera's white balance presets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presets {
    pub daylight: Option<[f32; 3]>,
    pub shade: Option<[f32; 3]>,
    pub d65: Option<[f32; 3]>,
    pub fine_weather: Option<[f32; 3]>,
}

/// A DNG's camera calibration gains and the signature they were made for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub gains: [f32; 3],
    pub signature: String,
}

/// A lens correction, as [`LensCorrection`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lens {
    pub source: String,
    pub default_on: bool,
    pub vignetting: Option<Table>,
    /// The vignetting is part of the photo as Camera Raw measures it
    /// ([`LensCorrection::vignetting_measured`]).
    pub vignetting_measured: bool,
    pub distortion: Option<Table>,
    /// Red, then blue.
    pub chromatic: Option<[Table; 2]>,
    pub chromatic_at_source: bool,
    pub gain_map: Option<Gains>,
    /// The optical centre, in fractions of the image's width and height.
    /// Missing: the image's centre.
    pub centre: [f32; 2],
}
impl Default for Lens {
    fn default() -> Self {
        Self {
            source: String::new(),
            default_on: false,
            vignetting: None,
            vignetting_measured: false,
            distortion: None,
            chromatic: None,
            chromatic_at_source: false,
            gain_map: None,
            centre: LensCorrection::image_centre(),
        }
    }
}

/// A radial table, as [`Radial`]. Both lists are required.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub knots: Vec<f32>,
    pub values: Vec<f32>,
}

/// A gain map, as [`GainGrid`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gains {
    /// `[top, left, bottom, right]`. Missing: the whole image.
    pub area: [f32; 4],
    pub after_warp: bool,
    /// Rows, then columns.
    pub points: [u32; 2],
    pub origin: [f32; 2],
    pub spacing: [f32; 2],
    pub planes: u32,
    pub gains: Vec<f32>,
}
impl Default for Gains {
    fn default() -> Self {
        Self {
            area: [0., 0., 1., 1.],
            after_warp: false,
            points: [0; 2],
            origin: [0.; 2],
            spacing: [0.; 2],
            planes: 0,
            gains: Vec::new(),
        }
    }
}

/// Auto CA's measurement: red and blue, or neither when it found nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoCa {
    pub red: Option<Table>,
    pub blue: Option<Table>,
}

/// Canon's Highlight Tone Priority. Missing: `off`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Htp {
    #[default]
    Off,
    On,
    Enhanced,
}

/// Where `cam_xyz` came from. Missing: `absent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixFrom {
    #[default]
    Absent,
    AdobeDng,
    CompatObserved,
    File,
    FileTungstenOnly,
    MakerNote,
    Neutral,
}

/// What a DNG original's embedded profile was to it. Missing: `none`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    #[default]
    None,
    Unreadable,
    MatrixOnly,
    OtherCamera,
    Usable,
}

impl Record {
    /// The record DNGPrivateData `bytes` hold. An error for anything but a
    /// complete record of this version: another maker's data, a version this
    /// release doesn't know, damage or a field missing that is required.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_RECORD,
            "An editable preview's record is larger than one may be"
        );
        let rest = bytes
            .strip_prefix(MAGIC)
            .context("Not an editable preview: its DNG holds no RAWmakase record")?;
        let (version, json) = rest
            .split_first_chunk::<2>()
            .context("An editable preview's record is cut short")?;
        let version = u16::from_be_bytes(*version);
        ensure!(version != 0, "An editable preview's record is damaged");
        ensure!(
            version <= VERSION,
            "This editable preview's record is version {version}, which only a later release of RAWmakase reads (this one reads version {VERSION})"
        );
        let record: Self =
            serde_json::from_slice(json).context("An editable preview's record is damaged")?;
        ensure!(
            record.width > 0
                && record.height > 0
                && record.copy_width > 0
                && record.copy_height > 0,
            "An editable preview's record has no size"
        );
        Ok(record)
    }

    /// The original's metadata as its decode had it, apart from the imported lens
    /// profiles that fit, which the caller matches. An error for a table, gain map
    /// or profile no reader of the original would have made.
    pub(super) fn metadata(&self) -> Result<Metadata> {
        let d = Metadata::default();
        let [crop_left, crop_top, crop_width, crop_height] = self.crop;
        let lateral_ca = std::sync::Arc::new(std::sync::OnceLock::new());
        if let Some(AutoCa { red, blue }) = &self.auto_ca {
            let measured = match (red, blue) {
                (None, None) => None,
                (Some(red), Some(blue)) => Some([red.radial()?, blue.radial()?]),
                _ => bail!("An editable preview's Auto CA has one channel"),
            };
            let _ = lateral_ca.set(measured);
        }
        let Presets {
            daylight,
            shade,
            d65,
            fine_weather,
        } = self.wb_presets.clone();
        Ok(Metadata {
            make: self.make.clone(),
            model: self.model.clone(),
            exif_make: self.exif_make.clone(),
            exif_model: self.exif_model.clone(),
            sensor_crop: self.sensor_crop,
            width: self.width,
            height: self.height,
            raw_width: self.raw_width,
            raw_height: self.raw_height,
            crop_width,
            crop_height,
            crop_left,
            crop_top,
            camera_crop: self.camera_crop,
            flip: self.flip,
            xtrans: self.xtrans,
            fuji_dynamic_range: self.fuji_dynamic_range,
            highlight_tone_priority: match self.highlight_tone_priority {
                Htp::Off => HighlightTonePriority::Off,
                Htp::On => HighlightTonePriority::On,
                Htp::Enhanced => HighlightTonePriority::Enhanced,
            },
            sraw: self.sraw,
            monochrome: self.monochrome,
            fuji_exposure_shift: self.fuji_exposure_shift,
            iso: self.iso,
            shutter: self.shutter,
            aperture: self.aperture,
            focal: self.focal,
            focal_35mm: self.focal_35mm,
            focus_distance: if self.focus_at_infinity {
                f32::INFINITY
            } else {
                self.focus_distance
            },
            wb: self.wb,
            daylight_wb: self.daylight_wb,
            wb_presets: WhiteBalancePresets {
                daylight,
                shade,
                d65,
                fine_weather,
            },
            matrix: self.matrix,
            cam_xyz: self.cam_xyz,
            matrix_source: match self.matrix_source {
                MatrixFrom::Absent => MatrixSource::Absent,
                MatrixFrom::AdobeDng => MatrixSource::AdobeDng,
                MatrixFrom::CompatObserved => MatrixSource::CompatObserved,
                MatrixFrom::File => MatrixSource::File,
                MatrixFrom::FileTungstenOnly => MatrixSource::FileTungstenOnly,
                MatrixFrom::MakerNote => MatrixSource::MakerNote,
                MatrixFrom::Neutral => MatrixSource::Neutral,
            },
            camera_look: self.camera_look.clone(),
            embedded_profile_status: match self.embedded_profile_status {
                ProfileStatus::None => EmbeddedProfileStatus::None,
                ProfileStatus::Unreadable => EmbeddedProfileStatus::Unreadable,
                ProfileStatus::MatrixOnly => EmbeddedProfileStatus::MatrixOnly,
                ProfileStatus::OtherCamera => EmbeddedProfileStatus::OtherCamera,
                ProfileStatus::Usable => EmbeddedProfileStatus::Usable,
            },
            lens: self.lens.as_ref().map(Lens::correction).transpose()?,
            lens_stand_in: self
                .lens_stand_in
                .as_ref()
                .map(Lens::correction)
                .transpose()?,
            lens_model: self.lens_model.clone(),
            lens_id: self.lens_id,
            lens_info: self.lens_info,
            adobe_names: self.adobe_names.clone(),
            baseline_exposure: self.baseline_exposure,
            dng_neutral_calibration: self.dng_neutral_calibration.as_ref().map(
                |Calibration { gains, signature }| NeutralCalibration {
                    gains: *gains,
                    signature: signature.clone(),
                },
            ),
            dng_analog_balance: self.dng_analog_balance,
            dng_matrix_profile_signature: self.dng_matrix_profile_signature.clone(),
            source: Source::Raw,
            hdr_source: None,
            levels: self.levels,
            lens_profiles: d.lens_profiles,
            lateral_ca,
            embedded_dcp: self
                .embedded_profile
                .as_deref()
                .map(unhex)
                .transpose()?
                .map(std::sync::Arc::from),
            coverage: None,
        })
    }

    /// The photo's default Camera Look (fitted), checked as a sidecar's is, for
    /// the photo `m` describes.
    pub(super) fn default_look(&self, m: &Metadata) -> Result<Option<CameraProfile>> {
        self.default_look
            .as_deref()
            .map(|text| {
                fitted::from_carried(text, m)
                    .context("An editable preview's Camera Look (fitted) can't be read")
            })
            .transpose()
    }
}

impl Lens {
    /// The correction, checked as every reader of one checks it.
    fn correction(&self) -> Result<LensCorrection> {
        let l = LensCorrection {
            source: self.source.clone(),
            default_on: self.default_on,
            vignetting: self.vignetting.as_ref().map(Table::radial).transpose()?,
            vignetting_measured: self.vignetting_measured,
            distortion: self.distortion.as_ref().map(Table::radial).transpose()?,
            chromatic: match &self.chromatic {
                Some([r, b]) => Some([r.radial()?, b.radial()?]),
                None => None,
            },
            chromatic_at_source: self.chromatic_at_source,
            gain_map: self.gain_map.as_ref().map(Gains::grid),
            centre: self.centre,
        };
        ensure!(
            l.validate(),
            "An editable preview's lens correction is not a valid one"
        );
        Ok(l)
    }
}

impl Table {
    fn radial(&self) -> Result<Radial> {
        Radial::new(self.knots.clone(), self.values.clone())
            .context("An editable preview's lens table is not a valid one")
    }
}

impl Gains {
    /// The grid; [`Lens::correction`] checks it.
    fn grid(&self) -> GainGrid {
        GainGrid {
            area: self.area,
            after_warp: self.after_warp,
            points: self.points.map(|v| v as usize),
            origin: self.origin,
            spacing: self.spacing,
            planes: self.planes as usize,
            gains: self.gains.clone(),
        }
    }
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    ensure!(
        text.len().is_multiple_of(2),
        "An editable preview's embedded profile is damaged"
    );
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .context("An editable preview's embedded profile is damaged")
        })
        .collect()
}
