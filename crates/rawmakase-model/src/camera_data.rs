//! What a camera captured, as values: a RAW's metadata as RAWmakase keeps it,
//! the demosaic and decode choices, and the decoded camera-space image. Reading
//! them from a file is the RAW decoders'; nothing here needs native code.
use serde::{Deserialize, Serialize};

/// A DNG's illuminant-independent diagonal camera calibration. The signature
/// must match the selected profile before these gains may be used.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NeutralCalibration {
    pub gains: [f32; 3],
    pub signature: String,
}

/// What kind of file a photo's pixels come from: a camera RAW, or a
/// rendered JPEG, PNG or TIFF decoded through its colour profile. A rendered file's
/// develop [`CameraImage`] holds its pseudo-scene: linear ProPhoto values whose
/// Camera Raw default rendering is the file (see `develop::rendered`), not the
/// file's own values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Raw,
    Rendered,
}

impl Source {
    pub fn is_raw(&self) -> bool {
        *self == Source::Raw
    }
}

/// Where a RAW's colour matrix (`Metadata::cam_xyz`) comes from. A fact of the
/// file and the build's camera table, read again on open and never saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MatrixSource {
    /// None: no camera table row, and no matrix in the file or its maker notes.
    #[default]
    Absent,
    /// The camera table's matrix from Adobe DNG Converter's output.
    AdobeDng,
    /// The camera table's matrix from RAWmakase's earlier decoder, for a camera
    /// Adobe has none for.
    CompatObserved,
    /// The DNG's own ColorMatrix tags.
    File,
    /// The DNG's own ColorMatrix, of a tungsten calibration alone.
    FileTungstenOnly,
    /// The camera's maker notes (Olympus), with its D65 levels.
    MakerNote,
    /// A monochrome DNG's neutral colour, by definition.
    Neutral,
}

/// What a DNG's embedded camera profile is to its photo: whether it has one, and
/// whether RAWmakase can use it. Read again on open, never saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EmbeddedProfileStatus {
    /// No embedded profile (every RAW but a DNG, and DNGs without one).
    #[default]
    None,
    /// Profile tags that do not parse as a camera profile, nor as colour matrices.
    Unreadable,
    /// Colour matrices without a forward matrix: matrix data, not a profile.
    MatrixOnly,
    /// A profile made for another camera than the photo's.
    OtherCamera,
    /// A profile for this camera (`Metadata::embedded_dcp`).
    Usable,
}

/// A white-balance preset a camera stores for itself, as R, G, B multipliers.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WhiteBalancePreset {
    Daylight,
    Shade,
    D65,
    /// Olympus's 5300 K entry, which LibRaw names Fine Weather.
    #[serde(rename = "fine-weather")]
    FineWeather,
}

/// The presets of a native RAW that individual-unit calibration uses: Sony and
/// most Canon bodies record theirs as Daylight, some Canon bodies as Shade,
/// Fujifilm as the D65 entry of its standard-illuminant table, and Olympus and
/// OM System as the 5300 K entry (Fine Weather). Each is `None` when the RAW has
/// none or it is not positive and finite.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct WhiteBalancePresets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daylight: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shade: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d65: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_weather: Option<[f32; 3]>,
}
impl WhiteBalancePresets {
    pub fn get(&self, preset: WhiteBalancePreset) -> Option<[f32; 3]> {
        match preset {
            WhiteBalancePreset::Daylight => self.daylight,
            WhiteBalancePreset::Shade => self.shade,
            WhiteBalancePreset::D65 => self.d65,
            WhiteBalancePreset::FineWeather => self.fine_weather,
        }
    }
    /// Keeps a preset only when every multiplier is positive and finite.
    pub fn valid(v: [f32; 3]) -> Option<[f32; 3]> {
        v.iter().all(|v| v.is_finite() && *v > 0.).then_some(v)
    }
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub make: String,
    pub model: String,
    /// The make and model as the file itself writes them (EXIF Make and Model, e.g.
    /// "NIKON CORPORATION", "NIKON D6"), for matching Lensfun's camera entries, which
    /// use them; `make` and `model` are RAWmakase's names. Read again on open, not
    /// saved; empty when not read.
    #[serde(skip)]
    pub exif_make: String,
    #[serde(skip)]
    pub exif_model: String,
    /// The frame's crop factor over the sensor's when the camera recorded only part
    /// of it in a crop mode (Canon's APS-C mode: 1.6); read again on open, not saved.
    #[serde(skip)]
    pub sensor_crop: Option<f32>,
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
    /// A Canon sRAW or mRAW (or Nikon sRAW): stored as YCbCr, which the decoder
    /// converts to camera RGB on a scale of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sraw: bool,
    /// A monochrome DNG (monochrome sensors, film scanners): developed as grey,
    /// R = G = B, through a neutral camera matrix. It has no white balance, so
    /// Temp and Tint do not apply (`Recipe::update_wb`), as in Lightroom.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub monochrome: bool,
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
    /// White-balance presets the camera stored in a native RAW's maker notes.
    /// Unlike `daylight_wb`, these are not calculated from a color matrix.
    #[serde(default, skip_serializing_if = "WhiteBalancePresets::is_empty")]
    pub wb_presets: WhiteBalancePresets,
    pub matrix: [[f32; 3]; 3],
    /// LibRaw's XYZ(D65)-to-camera matrix, equivalent to a DNG ColorMatrix. Zero when unknown.
    #[serde(default)]
    pub cam_xyz: [[f32; 3]; 3],
    /// Where `cam_xyz` comes from; read again on open, not saved.
    #[serde(skip)]
    pub matrix_source: MatrixSource,
    /// The look the camera's JPEGs were made with, by the maker's name for it; read
    /// again on open, not saved.
    #[serde(skip)]
    pub camera_look: Option<String>,
    /// What the DNG's embedded camera profile is to this photo; read again on open,
    /// not saved.
    #[serde(skip)]
    pub embedded_profile_status: EmbeddedProfileStatus,
    /// Built-in lens correction stored by the camera, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<crate::optics::LensCorrection>,
    /// The camera's fuller lens data, used in place of an Adobe lens profile when
    /// Enable Profile Corrections is on and none is imported (Sony's tables; see
    /// `rawmakase_decode`'s `raw::lens::Builtin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_stand_in: Option<crate::optics::LensCorrection>,
    /// Lens model as recorded by the camera, e.g. "FE 55mm F1.8 ZA". A file that
    /// records only `lens_id` takes the name of the imported lens profile that
    /// identifies it (see `rawmakase_decode::photo::open`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lens_model: String,
    /// The maker's numeric lens ID from the maker notes (Nikon's `LensIDNumber`,
    /// Pentax's two `LensType` bytes as 4 << 8 | 252, as the lens profiles'
    /// `stCamera:LensID` holds them too); 0 when the file records none. Cameras that
    /// write no lens model leave the ID as the only lens identity.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lens_id: u32,
    /// The lens's focal range and maximum apertures (shortest and longest focal
    /// length, maximum aperture at each) as the maker notes give them, Nikon's
    /// `Lens` tag: what Adobe's lens profiles hold as `stCamera:LensInfo`. It tells
    /// apart the lenses that share a `lens_id`, and names a lens the file doesn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_info: Option<[f32; 4]>,
    /// Other names, as "Make Model", that Adobe's camera profiles may use for this
    /// camera: LibRaw's canonical name for a body sold under several names, and
    /// `crate::cameras::adobe_names`' table. See `CameraProfile::ensure_camera`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adobe_names: Vec<String>,
    /// DNG BaselineExposure (0 when the DNG has none); `None` for other formats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_exposure: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dng_neutral_calibration: Option<NeutralCalibration>,
    /// A linear raw DNG's AnalogBalance relative to green, when not 1: gains
    /// its writer applied to the samples, which `wb` is of. A white set by
    /// temperature is the profile's camera neutral times these gains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dng_analog_balance: Option<[f32; 3]>,
    /// Signature of a DNG's matrix-only profile, retained when no forward profile parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dng_matrix_profile_signature: Option<String>,
    /// Raw unless the photo is a JPEG, PNG or TIFF. Raw is not written, so RAW
    /// metadata keeps its saved form; metadata from earlier releases reads as Raw.
    #[serde(default, skip_serializing_if = "Source::is_raw")]
    pub source: Source,
    /// For an HDR file (a rendered PQ, HLG or linear-light file), its transfer and
    /// the peak of its light; `None` for every other photo, and in metadata saved
    /// before HDR files opened. The light itself is [`CameraImage::hdr`]: this fact
    /// alone renders nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr_source: Option<HdrSource>,
    /// The sensor data's mean black, white and maximum (the samples' bit depth's)
    /// where the decoder knows them on opening (Canon's mosaics, whose white
    /// Adobe takes by ISO speed): the camera exposure follows the white as
    /// Adobe's BaselineExposure does (`camera_profiles::reference`). Read again
    /// on open, not saved.
    #[serde(skip)]
    pub levels: Option<[u32; 3]>,
    /// Imported Adobe lens profiles that fit this camera, Enable Profile Corrections'
    /// choices; rebuilt on open.
    #[serde(skip)]
    pub lens_profiles: crate::optics::lcp::PhotoProfiles,
    /// Lateral chromatic aberration measured from the decoded image, shared by every
    /// image made from it (see [`crate::lens::auto_ca::prime`]). Each decode has its
    /// own: an open, a decode-cache load. It is primed from the decoded image
    /// before any reduced or recovered copy of it is rendered.
    #[serde(skip)]
    pub lateral_ca: std::sync::Arc<std::sync::OnceLock<Option<[crate::optics::Radial; 2]>>>,
    /// The camera profile a DNG embeds, in DCP form, when it reads and fits this
    /// camera (`camera_profiles::builtin` reads it); rebuilt from the file on open.
    #[serde(skip)]
    pub embedded_dcp: Option<std::sync::Arc<[u8]>>,
    /// Where the file's image is empty, from a DNG's transparency mask
    /// (Lightroom's panoramas: the corners their stitch leaves); rendering
    /// shows white there, as Lightroom does. Read again on open, not saved.
    #[serde(skip)]
    pub coverage: Option<std::sync::Arc<Coverage>>,
}

/// How much of each decoded pixel the file's image covers, 0 (none) to 255,
/// row by row over the decoded image's grid (the visible area, not
/// oriented), from a DNG's transparency mask.
pub struct Coverage {
    pub width: u32,
    pub height: u32,
    pub values: Vec<u8>,
}

impl std::fmt::Debug for Coverage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Coverage({} × {})", self.width, self.height)
    }
}

impl Coverage {
    /// `values` over `width` × `height` pixels; `None` when they don't fill it.
    pub fn new(width: u32, height: u32, values: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && values.len() == width as usize * height as usize).then_some(
            Self {
                width,
                height,
                values,
            },
        )
    }
    /// Whether position (`x`, `y`) of an image of `width` × `height` pixels
    /// decoded from the file (pixel centres at whole numbers: a reduced copy's
    /// positions scale to the mask's) lies where the file has no image: the
    /// nearest pixel of the mask less than half covered.
    pub fn empty(&self, x: f32, y: f32, width: u32, height: u32) -> bool {
        let at = |v: f32, size: u32, own: u32| {
            (((v + 0.5) / size.max(1) as f32 * own as f32).floor() as i64).clamp(0, own as i64 - 1)
                as usize
        };
        let (cx, cy) = (at(x, width, self.width), at(y, height, self.height));
        self.values[cy * self.width as usize + cx] < 128
    }
}
impl Metadata {
    pub fn is_rendered(&self) -> bool {
        self.source == Source::Rendered
    }

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
fn is_zero(v: &u32) -> bool {
    *v == 0
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
/// Which demosaic full-size development uses, passed to each job that decodes.
/// RAWmakase's own is the only one; the preference RAWmakase once offered
/// (LibRaw's AHD and Markesteijn, saved as `"Libraw"`) reads as it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Demosaic {
    /// RAWmakase's own demosaic of the unpacked sensor data (`crate::demosaic`).
    #[default]
    #[serde(alias = "Libraw")]
    Rawmakase,
}
/// What `rawmakase_decode`'s `Raw::develop` makes of the sensor data. A job captures it when it is
/// created and keys its decode-cache entry with the same value, so a preference
/// changed while the job runs can never file one demosaic under another's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    /// A half-size draft: each pixel a 2 × 2 block of the mosaic.
    Half,
    /// Every pixel, demosaiced this way.
    Full(Demosaic),
}
impl Decode {
    /// Full size, with `preferred`.
    pub fn full(preferred: Demosaic) -> Self {
        Self::Full(preferred)
    }
}
impl crate::metadata::PhotoInfo {
    /// From a RAW's metadata, as the decoder reads it.
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
/// A decoded photo for developing. For a RAW, camera-space values; for a rendered
/// file ([`Source::Rendered`]), its pseudo-scene: linear ProPhoto values whose Camera
/// Raw default rendering is the file (see `develop::rendered`).
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
    /// An HDR file's own light beside its SDR pseudo-scene
    /// (docs/records/hdr-stage4-design.md); empty for every other image.
    pub hdr: HdrSlot,
}

/// What an HDR file is, as its metadata records it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HdrSource {
    /// The transfer its samples are encoded with; `Linear` for a gain map, whose
    /// light is reconstructed as linear light.
    pub transfer: crate::color::hdr_output::HdrTransfer,
    /// The light was reconstructed from an ISO 21496-1 gain map over the file's SDR
    /// base image, which stays its SDR rendition (docs/hdr.md#gain-map-files).
    /// Omitted when false, so the facts of other HDR files read as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gain_map: bool,
    /// The largest channel of the full-resolution light, 1 = graphics white.
    pub peak: f32,
}

/// An HDR file's light: linear ProPhoto RGB, 1 = graphics white (203 cd/m²), finite
/// and signed, oriented as its paired SDR image. Built by the decoder
/// ([`HdrLight::new`]) and, for previews, reduced from one ([`HdrLight::reduced`]);
/// immutable after.
#[derive(Debug)]
pub struct HdrLight {
    width: u32,
    height: u32,
    pixels: std::sync::Arc<[[f32; 3]]>,
    source: HdrSource,
}
impl HdrLight {
    /// `pixels` of a `width` × `height` file whose transfer is `transfer`; `None`
    /// when the count doesn't match. Non-finite values become 0.
    pub fn new(
        width: u32,
        height: u32,
        pixels: Vec<[f32; 3]>,
        transfer: crate::color::hdr_output::HdrTransfer,
    ) -> Option<Self> {
        Self::checked(width, height, pixels, transfer, false)
    }
    /// The light a gain map reconstructs over a `width` × `height` base image; as
    /// [`HdrLight::new`], with the gain map recorded in its [`HdrSource`].
    pub fn from_gain_map(width: u32, height: u32, pixels: Vec<[f32; 3]>) -> Option<Self> {
        use crate::color::hdr_output::HdrTransfer;
        Self::checked(width, height, pixels, HdrTransfer::Linear, true)
    }
    fn checked(
        width: u32,
        height: u32,
        mut pixels: Vec<[f32; 3]>,
        transfer: crate::color::hdr_output::HdrTransfer,
        gain_map: bool,
    ) -> Option<Self> {
        if pixels.len() != width as usize * height as usize {
            return None;
        }
        let mut peak = 0f32;
        for p in &mut pixels {
            for v in p.iter_mut() {
                if !v.is_finite() {
                    *v = 0.;
                }
                peak = peak.max(*v);
            }
        }
        Some(Self {
            width,
            height,
            pixels: pixels.into(),
            source: HdrSource {
                transfer,
                gain_map,
                peak,
            },
        })
    }
    /// This light reduced to `width` × `height` by `reduce`, which averages it as the
    /// paired SDR image is reduced; the full-resolution peak is kept, so defaults
    /// don't depend on a reduction. `None` when `reduce` returns the wrong count.
    pub fn reduced(
        &self,
        width: u32,
        height: u32,
        reduce: impl FnOnce(&[[f32; 3]]) -> Vec<[f32; 3]>,
    ) -> Option<Self> {
        let pixels = reduce(&self.pixels);
        (pixels.len() == width as usize * height as usize).then(|| Self {
            width,
            height,
            pixels: pixels.into(),
            source: self.source,
        })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }
    pub fn source(&self) -> HdrSource {
        self.source
    }
}

/// Where a decoded HDR file keeps its light. Empty by default, and a clone is
/// empty: a copy of an image is an SDR working image unless it is made through the
/// operations that keep the light ([`CameraImage::with_hdr`],
/// [`CameraImage::shares_hdr`]).
#[derive(Default)]
pub struct HdrSlot(Option<std::sync::Arc<HdrLight>>);
impl Clone for HdrSlot {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl CameraImage {
    /// This image's pixels per pixel of the photo it develops: below 1 for a
    /// reduced decode ([`fast`](Self::fast)), whose [`Metadata`] keeps the photo's
    /// size, and 1 for any other image. Effects sized in the photo's pixels
    /// (sharpening, grain) are scaled by it, so a reduced decode renders as its
    /// photo resized. Only a reduced decode is scaled: a full decode's metadata may
    /// give another size (margins, a turned sensor) without its pixels being smaller.
    pub fn photo_scale(&self) -> f32 {
        let photo = self.metadata.width.max(self.metadata.height);
        let own = self.width.max(self.height);
        if self.fast && photo > own && own > 0 {
            own as f32 / photo as f32
        } else {
            1.
        }
    }
    /// Whether position (`x`, `y`) of this image's pixels lies where the
    /// file has no image (its [`Metadata::coverage`]): rendering shows white
    /// there, as for an area a transform leaves uncovered.
    pub fn empty_at(&self, x: f32, y: f32) -> bool {
        self.metadata
            .coverage
            .as_ref()
            .is_some_and(|c| c.empty(x, y, self.width, self.height))
    }
    /// `self` paired with `light`, its HDR light, which records the source fact in
    /// the metadata; refused when their sizes differ.
    pub fn with_hdr(mut self, light: std::sync::Arc<HdrLight>) -> Option<Self> {
        if (light.width, light.height) != (self.width, self.height) {
            return None;
        }
        self.metadata.hdr_source = Some(light.source);
        self.hdr = HdrSlot(Some(light));
        Some(self)
    }
    /// `self` with the light `other` holds, shared: for an image that is `other`
    /// at its size (a preview that needed no reduction).
    pub fn shares_hdr(self, other: &CameraImage) -> Self {
        match other.hdr.0.clone() {
            Some(light) => self
                .with_hdr(light)
                .unwrap_or_else(|| unreachable!("an image sharing light is the same size")),
            None => self,
        }
    }
    /// The HDR file's light, while it still matches this image's size; `None` for
    /// every other image. A mismatch can only come from code that changed the
    /// image's size in place.
    pub fn hdr(&self) -> Option<&std::sync::Arc<HdrLight>> {
        let light = self.hdr.0.as_ref()?;
        let matches = (light.width, light.height) == (self.width, self.height);
        debug_assert!(matches, "an HDR light no longer the size of its image");
        matches.then_some(light)
    }
}

#[cfg(test)]
mod hdr_tests {
    use super::*;
    use crate::color::hdr_output::HdrTransfer;
    use std::sync::Arc;

    fn image(width: u32, height: u32) -> CameraImage {
        CameraImage {
            width,
            height,
            recovered: Default::default(),
            pixels: vec![[0.5; 3]; (width * height) as usize],
            metadata: Metadata {
                source: Source::Rendered,
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }

    /// The light pairs only with an image its size, records the source fact, and
    /// isn't copied into a clone, which is an SDR working image.
    #[test]
    // The clone is what is tested: it must drop the light.
    #[allow(clippy::redundant_clone)]
    fn hdr_light_pairs_by_size_and_clones_drop_it() {
        let light = HdrLight::new(
            2,
            1,
            vec![[4., 1., f32::NAN], [-0.5, 0.2, 0.1]],
            HdrTransfer::Pq,
        )
        .unwrap();
        assert_eq!(light.source().peak, 4.);
        assert_eq!(light.pixels()[0][2], 0., "non-finite becomes 0");
        assert_eq!(light.pixels()[1][0], -0.5, "signed values stay");
        assert!(HdrLight::new(2, 2, vec![[0.; 3]], HdrTransfer::Pq).is_none());
        let light = Arc::new(light);
        assert!(image(3, 1).with_hdr(light.clone()).is_none());
        let paired = image(2, 1).with_hdr(light.clone()).unwrap();
        assert!(paired.hdr().is_some());
        assert_eq!(paired.metadata.hdr_source, Some(light.source()));
        assert!(paired.clone().hdr().is_none());
        let shared = paired.clone().shares_hdr(&paired);
        assert!(Arc::ptr_eq(shared.hdr().unwrap(), &light));
    }

    /// A reduced light keeps the full-resolution peak and must have the size asked.
    #[test]
    fn a_reduced_light_keeps_the_full_peak() {
        let light = HdrLight::new(2, 1, vec![[8.; 3], [0.; 3]], HdrTransfer::Linear).unwrap();
        let half = light
            .reduced(1, 1, |p| vec![[(p[0][0] + p[1][0]) / 2.; 3]])
            .unwrap();
        assert_eq!(half.pixels(), &[[4.; 3]]);
        assert_eq!(half.source().peak, 8.);
        assert!(light.reduced(2, 2, |p| p.to_vec()).is_none());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn metadata_saved_before_sources_reads_as_raw() {
        // A Metadata serialised by an earlier release has no `source`.
        let mut v = serde_json::to_value(Metadata::default()).unwrap();
        v.as_object_mut().unwrap().remove("source");
        let m: Metadata = serde_json::from_value(v).unwrap();
        assert_eq!(m.source, Source::Raw);
        assert!(!m.is_rendered());
        let r = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let back: Metadata = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert!(back.is_rendered());
    }
}
