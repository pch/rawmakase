//! What a camera captured, as values: a RAW's metadata as RAWmakase keeps it,
//! the demosaic and decode choices, and the decoded camera-space image. Reading
//! them through LibRaw is [`crate::raw`]'s; nothing here needs native code.
use serde::{Deserialize, Serialize};

/// A DNG's illuminant-independent diagonal camera calibration. The signature
/// must match the selected profile before these gains may be used.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NeutralCalibration {
    pub gains: [f32; 3],
    pub signature: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub make: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub raw_width: u32,
    pub raw_height: u32,
    /// The camera's default crop (Adobe's DNG DefaultCrop) in decoded pixels: the
    /// frame image space, Lightroom's crop and its positions are relative to.
    pub crop_width: u32,
    pub crop_height: u32,
    pub crop_left: u32,
    pub crop_top: u32,
    /// The aspect ratio chosen in the camera (1:1, 4:3, 16:9…), as `[left, top,
    /// right, bottom]` fractions of the default crop. Like Camera Raw's default user
    /// crop, it is the crop a photo's settings start from, not part of the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_crop: Option<[f32; 4]>,
    pub flip: i32,
    pub xtrans: bool,
    #[serde(default)]
    pub fuji_dynamic_range: u32,
    /// Canon Highlight Tone Priority, from the maker notes; `Off` for other makes.
    #[serde(default)]
    pub highlight_tone_priority: HighlightTonePriority,
    /// Fujifilm's exposure midpoint shift in EV (maker note ExpoMidPointShift): about
    /// −0.7 at DR100, a stop lower for each DR step up, a stop higher at extended
    /// low ISO. `None` when the raw has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuji_exposure_shift: Option<f32>,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal: f32,
    /// Focal length in 35mm equivalent (EXIF FocalLengthIn35mmFilm); 0 when unknown.
    #[serde(default)]
    pub focal_35mm: f32,
    pub wb: [f32; 3],
    pub daylight_wb: [f32; 3],
    /// Sony's per-unit daylight preset, read from the native RAW maker notes.
    /// Unlike `daylight_wb`, this is not calculated from a color matrix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sony_daylight_wb: Option<[f32; 3]>,
    pub matrix: [[f32; 3]; 3],
    /// LibRaw's XYZ(D65)-to-camera matrix, equivalent to a DNG ColorMatrix. Zero when unknown.
    #[serde(default)]
    pub cam_xyz: [[f32; 3]; 3],
    /// Built-in lens correction stored by the camera, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<crate::optics::LensCorrection>,
    /// Lens model as recorded by the camera, e.g. "FE 55mm F1.8 ZA".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lens_model: String,
    /// DNG BaselineExposure (0 when the DNG has none); `None` for other formats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_exposure: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dng_neutral_calibration: Option<NeutralCalibration>,
    /// Signature of a DNG's matrix-only profile, retained when no forward profile parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dng_matrix_profile_signature: Option<String>,
    /// Imported Adobe lens profiles that fit this camera, Enable Profile Corrections'
    /// choices; rebuilt on open.
    #[serde(skip)]
    pub lens_profiles: crate::optics::lcp::PhotoProfiles,
    /// Lateral chromatic aberration measured from the decoded image, shared by every
    /// image made from it (see `crate::lens::auto_ca::prime`).
    #[serde(skip)]
    pub lateral_ca: std::sync::Arc<std::sync::OnceLock<Option<[crate::optics::Radial; 2]>>>,
    /// The camera profile a DNG embeds, in DCP form, when it reads and fits this
    /// camera (`camera_profiles::builtin` reads it); rebuilt from the file on open.
    #[serde(skip)]
    pub embedded_dcp: Option<std::sync::Arc<[u8]>>,
}
impl Metadata {
    /// Adobe's default crop (DNG DefaultCrop, or the RAF header's crop, which is
    /// 2 px larger per side than LibRaw's), as `[left, top, width, height]`;
    /// ignored unless it fits the image.
    pub fn apply_default_crop(&mut self, [left, top, width, height]: [u32; 4]) {
        if left.checked_add(width).is_some_and(|r| r <= self.width)
            && top.checked_add(height).is_some_and(|b| b <= self.height)
        {
            self.crop_left = left;
            self.crop_top = top;
            self.crop_width = width;
            self.crop_height = height;
        }
    }
}
/// Canon Highlight Tone Priority: the camera exposes a stop darker to keep
/// highlights, and Camera Raw brightens the photo by that stop again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightTonePriority {
    #[default]
    Off,
    On,
    /// "Enhanced" (D+2) on recent bodies. No sample has been measured yet.
    Enhanced,
}
impl HighlightTonePriority {
    /// From LibRaw's `makernotes.canon.HighlightTonePriority`.
    pub fn from_libraw(v: i32) -> Self {
        match v {
            1 => Self::On,
            2 => Self::Enhanced,
            _ => Self::Off,
        }
    }
}
/// Which demosaic full-size development uses: the app's preference (Preferences >
/// Performance), passed to each job that decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Demosaic {
    /// RAWmakase's own demosaic of LibRaw-unpacked data (`crate::demosaic`): about
    /// 2–5× faster, with equal or better detail against Adobe renders.
    #[default]
    Rawmakase,
    /// LibRaw's AHD (Bayer) and 1-pass Markesteijn (X-Trans).
    Libraw,
}
impl Demosaic {
    /// This preference, unless RAWMAKASE_LIBRAW_DEMOSAIC=1 forces LibRaw for the
    /// whole run; the variable is read once.
    pub fn effective(self) -> Self {
        static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let forced = *FORCED.get_or_init(|| {
            std::env::var_os("RAWMAKASE_LIBRAW_DEMOSAIC").is_some_and(|v| v != "0")
        });
        if forced { Self::Libraw } else { self }
    }
}
/// What [`Raw::develop`] makes of the sensor data. A job captures it when it is
/// created and keys its decode-cache entry with the same value, so a preference
/// changed while the job runs can never file one demosaic under another's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    /// A half-size draft from LibRaw's fast half-size path, whatever the demosaic.
    Half,
    /// Every pixel, demosaiced this way.
    Full(Demosaic),
}
impl Decode {
    /// Full size, with `preferred` unless the environment forces LibRaw.
    pub fn full(preferred: Demosaic) -> Self {
        Self::Full(preferred.effective())
    }
}
impl crate::metadata::PhotoInfo {
    /// From a RAW's metadata, as LibRaw reads it.
    pub fn from_metadata(m: &Metadata) -> Self {
        let positive = |v: f32| (v > 0.).then_some(v as f64);
        let text = |t: &str| (!t.trim().is_empty()).then(|| t.trim().to_string());
        // LibRaw's flip 5 and 6 are quarter turns.
        // The camera's default crop is the frame shown, when it has one.
        let (w, h) = if m.crop_width > 0 && m.crop_height > 0 {
            (m.crop_width, m.crop_height)
        } else {
            (m.width, m.height)
        };
        let (w, h) = if matches!(m.flip, 5 | 6) {
            (h, w)
        } else {
            (w, h)
        };
        Self {
            camera: text(&m.model).or_else(|| text(&m.make)),
            lens: text(&m.lens_model),
            focal: positive(m.focal),
            aperture: positive(m.aperture),
            exposure: positive(m.shutter),
            iso: positive(m.iso),
            dimensions: (w > 0 && h > 0).then_some((w, h)),
        }
    }
}
#[derive(Clone)]
pub struct CameraImage {
    pub width: u32,
    pub height: u32,
    pub recovered: std::sync::OnceLock<std::sync::Arc<CameraImage>>,
    pub pixels: Vec<[f32; 3]>,
    pub metadata: Metadata,
    pub fast: bool,
    pub scale_factor: f32,
    pub scale_clipped: u32,
}
