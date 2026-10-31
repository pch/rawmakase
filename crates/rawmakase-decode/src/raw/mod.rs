//! RAW files: opening them with RAWmakase's own decoders (`rawmakase-raw`,
//! through [`own`]), reading their metadata, development into linear
//! camera-space pixels and the embedded preview. The values they produce are
//! [`crate::camera_data`]'s.
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
mod own;
mod priority;
use crate::camera_data::{
    CameraImage, Decode, HighlightTonePriority, Metadata, Source, WhiteBalancePresets,
};
/// The version of the decoder photos open with.
pub fn version() -> String {
    rawmakase_raw::VERSION.to_string()
}
/// What a developed image depends on in the decoder, for identities of
/// developed images kept across runs (stored previews): the decoding and the
/// demosaic and associated lens sampling, bumped when these change the output.
/// 2: Directional X-Trans blending, stripe chromaticity reconstruction and
/// matching lens-CA sampling protection (#547).
pub fn developed() -> String {
    "rawmakase-raw 2".into()
}
/// Runs `work` on a thread of its own at background priority: a lower
/// scheduling priority, and parallel work (RAWmakase's decoders) on two
/// background threads, never the shared pool.
/// Every worker that reads or decodes photos behind the user's back starts
/// here or in [`background_pool`], so none competes with Develop for the CPU;
/// one that takes a wider pool of its own does so only while Develop is not
/// in front (the Library's edited previews).
pub fn spawn_background(work: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // `work` runs on the pool's threads, which lower their own priority
        // (once: on Linux they would inherit this thread's). The pool's threads
        // end when it is dropped, after `work`, without being joined.
        match background_pool(2, "background") {
            Ok(pool) => pool.install(work),
            Err(_) => {
                background_thread();
                work()
            }
        }
    })
}
/// A pool of `threads` named `name-0`, `name-1`… at background priority, as
/// [`spawn_background`].
pub fn background_pool(
    threads: usize,
    name: &'static str,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("{name}-{i}"))
        .start_handler(|_| background_thread())
        .build()
}
/// Runs the calling thread at background priority from now on.
fn background_thread() {
    priority::lower();
}
/// A camera's make and model as its file writes them.
pub use rawmakase_raw::FileNames;
/// The make and model the RAW file at `path` writes, before RAWmakase names
/// its camera (Lightroom's Make and Model file-name tokens): read from its
/// header, nothing decoded. For files whose EXIF `crate::exif` can't read,
/// such as a CR3.
pub fn file_camera_names(path: &Path) -> Result<FileNames> {
    if let Some(undecoded) = undecoded(path) {
        return Ok(undecoded.names);
    }
    let bytes = std::fs::read(path)?;
    Ok(rawmakase_raw::file_camera_names(&bytes)?)
}
/// The maker a file's make names, as RAWmakase spells it everywhere
/// ("SONY" is "Sony").
pub fn maker_name(file_make: &str) -> String {
    rawmakase_raw::maker_name(file_make)
}
/// When the CR3, CRW, MRW or X3F at `path` was taken, in the form and by
/// the fallbacks of `exif::capture_time`, which can't read these files'
/// dates: read from the few parts of the file that hold them, nothing
/// decoded. `None` for other files and files without a date.
pub fn capture_time(path: &Path) -> Option<String> {
    let dates = rawmakase_raw::capture_dates(&mut std::fs::File::open(path).ok()?);
    rawmakase_interop::exif::capture_time_of(
        [dates.original, dates.digitized, dates.modified].map(|s| (s.date, s.subseconds)),
    )
}
/// The camera's EXIF of the CR3, MRW or X3F at `path`, which
/// `exif::read` can't find: the TIFFs the container keeps it in, read by
/// `exif::read_tiffs`; nothing else is read. `None` for other files.
pub(crate) fn camera_exif(path: &Path) -> Option<rawmakase_interop::exif::CameraExif> {
    let tiffs = rawmakase_raw::exif_tiffs(&mut std::fs::File::open(path).ok()?);
    rawmakase_interop::exif::read_tiffs(
        tiffs.main.as_deref(),
        tiffs.exif.as_deref(),
        tiffs.gps.as_deref(),
    )
}
/// The XMP packet the camera embeds in the CR3 at `path`, found by
/// `rawmakase_raw::embedded_xmp`; nothing decoded. `None` for other files.
pub(crate) fn embedded_xmp(path: &Path) -> Option<Vec<u8>> {
    rawmakase_raw::embedded_xmp(&mut std::fs::File::open(path).ok()?)
}
/// What RAWmakase reads of a RAW whose sensor data it doesn't decode, an
/// X3F: its photo info and its make and model as the file writes them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Undecoded {
    /// No dimensions: the preview's are not the photo's.
    pub info: rawmakase_model::metadata::PhotoInfo,
    pub names: FileNames,
}
/// The info and names of the X3F at `path`, from its preview's EXIF, else
/// (the SD9 and SD10, whose preview has none) its property list, read by
/// the rules of `exif::photo_info` and `camera_names`; nothing decoded.
/// `None` for other files and an X3F that can't be read (not even in
/// part: what can't be read now is read again later).
pub fn undecoded(path: &Path) -> Option<Undecoded> {
    use rawmakase_interop::exif::{CameraExif, Field, tag};
    let x3f = rawmakase_raw::x3f_metadata(&mut std::fs::File::open(path).ok()?)?;
    let properties = x3f.properties;
    let preview = x3f.exif.as_deref();
    let exif = rawmakase_interop::exif::read_tiffs(preview, None, None).unwrap_or_else(|| {
        // The property list as the EXIF tags it stands in for.
        let rational =
            |tag, v: f64| Field::rational(tag, (v * 1_000_000.).round() as u32, 1_000_000);
        let mut main = Vec::new();
        for (tag, text) in [
            (tag::MAKE, &properties.make),
            (tag::MODEL, &properties.model),
        ] {
            if !text.is_empty() {
                main.push(Field::ascii(tag, text));
            }
        }
        let mut exif = Vec::new();
        exif.extend(properties.exposure.map(|v| rational(tag::EXPOSURE_TIME, v)));
        exif.extend(properties.aperture.map(|v| rational(tag::F_NUMBER, v)));
        exif.extend(
            properties
                .iso
                .map(|v| Field::short(tag::ISO, v.min(65535.) as u16)),
        );
        exif.extend(properties.focal.map(|v| rational(tag::FOCAL_LENGTH, v)));
        CameraExif {
            main,
            exif,
            gps: Vec::new(),
        }
    });
    let (make, model) = exif.camera_names();
    Some(Undecoded {
        info: exif.photo_info().unwrap_or_default(),
        names: FileNames { make, model },
    })
}
/// The EXIF directory's date tags of the CR3, CRW, MRW or X3F at `path`
/// (DateTimeOriginal and DateTimeDigitized with their subseconds), from the
/// records [`capture_time`] reads: for the stand-in an export writes for a
/// RAW without EXIF of its own, a CRW. None for other files.
pub fn capture_date_tags(path: &Path) -> Vec<rawmakase_interop::exif::Field> {
    use rawmakase_interop::exif::{Field, tag};
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let dates = rawmakase_raw::capture_dates(&mut file);
    let mut tags = Vec::new();
    for (stamp, date, subseconds) in [
        (
            dates.original,
            tag::DATE_TIME_ORIGINAL,
            tag::SUBSEC_TIME_ORIGINAL,
        ),
        (
            dates.digitized,
            tag::DATE_TIME_DIGITIZED,
            tag::SUBSEC_TIME_DIGITIZED,
        ),
    ] {
        if let Some(text) = stamp.date {
            tags.push(Field::ascii(date, &text));
            tags.extend(stamp.subseconds.map(|s| Field::ascii(subseconds, &s)));
        }
    }
    tags
}
pub struct Raw {
    // Boxed: the decoder's state is large, and a `Raw` travels in enums.
    own: Box<own::Own>,
    pub metadata: Metadata,
    pub facts: Facts,
}
/// What the decoder read about a file beyond [`Metadata`], before any of
/// RAWmakase's own crop rules: for decode fingerprints, which compare decoders.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Facts {
    /// Where the visible area starts in the raw data: left, top.
    pub margins: [u32; 2],
    /// The decoder's default crop and the camera's aspect-ratio crop, `[left,
    /// top, width, height]` in raw coordinates (0xffff positions when unknown).
    pub frame: [u32; 4],
    pub aspect: [u32; 4],
    /// The canonical make and model of a body sold under several names.
    pub normalized: [String; 2],
    /// The DNG version, 0 for a camera's own format.
    pub dng_version: u32,
}
/// Unpacked sensor data for RAWmakase's own demosaic.
pub(crate) struct Cfa {
    pub width: u32,
    pub height: u32,
    /// The colour pattern as a `PATTERN × PATTERN` tile: 0 red, 1 green, 2 blue.
    pub pattern: [u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN],
    /// `width × height` values, (raw − black) / (white − black), not white balanced.
    pub data: Vec<f32>,
}
/// The unpacked sensor data with the levels development reads, for decode
/// fingerprints. Channels are LibRaw's: 0 red, 1 green, 2 blue, 3 second green.
pub(crate) struct Sensor {
    /// 1 for a mosaic, 3 or 4 for small RAW files.
    pub channels: u32,
    /// Where the visible area starts in the raw buffer: left, top.
    pub origin: [u32; 2],
    /// The size of the area development reads, from `origin`.
    pub area: [u32; 2],
    /// SHA-256 of the values development reads (the visible area; the stored
    /// rotated area of a Super CCD), `channels` per pixel, row by row,
    /// little-endian. Not the borders: LibRaw leaves rows and columns past some
    /// decoders' data unwritten, so they hold whatever the memory held.
    pub data_sha256: [u8; 32],
    pub black: [u32; 4],
    /// The black pattern added to `black`: rows, columns and every value.
    pub black_pattern: (u32, u32, Vec<u32>),
    pub white: u32,
    pub linear_white: [u32; 4],
    /// The colour of each visible pixel over one period of the mosaic; empty
    /// without one.
    pub pattern: Vec<Vec<u8>>,
}
/// The decoder's header facts: what [`Metadata`] is made of.
pub(crate) struct Header {
    pub make: String,
    pub model: String,
    pub normalized: [String; 2],
    /// The make and model as the file writes them (`RawInfo::file_make`), which
    /// Lensfun's camera entries use.
    pub file_names: [String; 2],
    /// The crop mode's crop factor over the sensor's (`RawInfo::sensor_crop`).
    pub sensor_crop: Option<f32>,
    pub width: u32,
    pub height: u32,
    pub raw_width: u32,
    pub raw_height: u32,
    pub margins: [u32; 2],
    pub frame: [u32; 4],
    pub aspect: [u32; 4],
    /// Panasonic's sensor borders, and a RAF header's recommended crop, in raw
    /// coordinates.
    pub sensor_borders: Option<[u32; 4]>,
    pub recommended_crop: Option<[u32; 4]>,
    pub flip: i32,
    pub xtrans: bool,
    pub fuji_dynamic_range: u32,
    pub highlight_tone_priority: i32,
    pub sraw: bool,
    pub fuji_exposure_shift: f32,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal: f32,
    pub focal_35mm: f32,
    /// As shot and daylight, relative to green.
    pub wb: [f32; 3],
    pub daylight_wb: [f32; 3],
    /// Daylight, shade, D65 and fine weather.
    pub presets: [[f32; 3]; 4],
    pub matrix: [[f32; 3]; 3],
    pub cam_xyz: [[f32; 3]; 3],
    pub matrix_source: rawmakase_model::camera_data::MatrixSource,
    pub camera_look: Option<String>,
    pub lens: String,
    pub lens_id: u32,
    pub dng_version: u32,
    /// Black and white known on opening; see `Metadata::levels`.
    pub levels: Option<[u32; 3]>,
    /// A linear raw DNG's AnalogBalance relative to green; 1 otherwise.
    pub analog_balance: [f32; 3],
    pub monochrome: bool,
}
impl Raw {
    /// The photo's facts as RAWmakase's decoders read them. What the file says
    /// beyond them (lens tables, a DNG's profile and hints) and the imported
    /// lens profiles are `crate::photo::open`'s.
    pub fn open_file(path: &Path) -> Result<Self> {
        let (own, h) = own::Own::open(path)?;
        let adobe_names = rawmakase_model::cameras::adobe_names(
            &h.make,
            &h.model,
            (&h.normalized[0], &h.normalized[1]),
        );
        let margin = h.margins;
        let facts = Facts {
            margins: margin,
            frame: h.frame,
            aspect: h.aspect,
            normalized: h.normalized.clone(),
            dng_version: h.dng_version,
        };
        let ([crop_left, crop_top, crop_width, crop_height], camera_crop) = crops(&h);
        let mut metadata = Metadata {
            exif_make: h.file_names[0].clone(),
            exif_model: h.file_names[1].clone(),
            sensor_crop: h.sensor_crop,
            width: h.width,
            height: h.height,
            raw_width: h.raw_width,
            raw_height: h.raw_height,
            crop_width,
            crop_height,
            crop_left,
            crop_top,
            camera_crop,
            flip: h.flip,
            xtrans: h.xtrans,
            fuji_dynamic_range: h.fuji_dynamic_range,
            highlight_tone_priority: HighlightTonePriority::from_libraw(h.highlight_tone_priority),
            sraw: h.sraw,
            monochrome: h.monochrome,
            // -999 when the maker notes have no shift.
            fuji_exposure_shift: (h.fuji_exposure_shift > -100.).then_some(h.fuji_exposure_shift),
            iso: h.iso,
            shutter: h.shutter,
            aperture: h.aperture,
            focal: h.focal,
            focal_35mm: h.focal_35mm,
            wb: h.wb,
            daylight_wb: h.daylight_wb,
            wb_presets: WhiteBalancePresets {
                daylight: WhiteBalancePresets::valid(h.presets[0]),
                shade: WhiteBalancePresets::valid(h.presets[1]),
                d65: WhiteBalancePresets::valid(h.presets[2]),
                fine_weather: WhiteBalancePresets::valid(h.presets[3]),
            },
            matrix: h.matrix,
            cam_xyz: h.cam_xyz,
            matrix_source: h.matrix_source,
            camera_look: h.camera_look,
            embedded_profile_status: Default::default(),
            lens: None,
            lens_stand_in: None,
            lens_model: h.lens.trim().to_string(),
            lens_id: h.lens_id,
            adobe_names,
            baseline_exposure: None,
            dng_neutral_calibration: None,
            dng_analog_balance: (h.analog_balance != [1.; 3]).then_some(h.analog_balance),
            dng_matrix_profile_signature: None,
            source: Source::Raw,
            lens_profiles: Default::default(),
            lensfun: Default::default(),
            lateral_ca: Default::default(),
            embedded_dcp: None,
            levels: h.levels,
            make: h.make,
            model: h.model,
        };
        if let Some(crop) = h.recommended_crop {
            metadata.apply_default_crop(visible(crop, margin));
        }
        Ok(Self {
            own: Box::new(own),
            metadata,
            facts,
        })
    }
    /// The colour space the camera's maker notes give its JPEGs (Nikon's and
    /// Olympus's ColorSpace), for a preview whose EXIF does not say it.
    pub fn maker_preview_space(&self) -> Option<MakerPreviewSpace> {
        self.own.preview_space()
    }
    /// The embedded JPEG preview, as stored.
    pub fn thumbnail(&mut self) -> Result<Vec<u8>> {
        self.own.thumbnail()
    }
    /// The embedded preview a thumbnail `edge` pixels on its long side is
    /// best made from, as stored: the smallest that reaches `edge` and has
    /// the largest one's shape, else the largest.
    pub fn thumbnail_reaching(&mut self, edge: u32) -> Result<Vec<u8>> {
        self.own.thumbnail_reaching(edge)
    }
    /// The unpacked sensor data as development reads it, without developing.
    pub(crate) fn sensor(&self) -> Result<Sensor> {
        self.own.sensor()
    }
    pub fn develop(self, decode: Decode, cancel: &AtomicBool) -> Result<CameraImage> {
        // Small RAWs develop at full size in either mode, as they always have.
        if let Some((width, height, pixels)) = self.own.rgb(self.metadata.wb, cancel)? {
            ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
            return Ok(CameraImage {
                recovered: Default::default(),
                width,
                height,
                pixels,
                metadata: self.metadata,
                fast: decode == Decode::Half,
                // Developed in floating point: nothing is scaled down to
                // integers, so nothing saturates.
                scale_factor: 1.,
                scale_clipped: 0,
            });
        }
        let cfa = self.own.cfa(cancel)?;
        self.develop_mosaic(cfa, decode == Decode::Half, cancel)?
            .ok_or_else(|| anyhow::anyhow!("Sensor data RAWmakase cannot develop"))
    }
    /// The mosaic white balanced and demosaiced, or at half size (`half`,
    /// `demosaic::half`); `None` without a mosaic.
    fn develop_mosaic(
        &self,
        cfa: Option<Cfa>,
        half: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<CameraImage>> {
        let Some(Cfa {
            width: w,
            height: h,
            pattern,
            mut data,
        }) = cfa
        else {
            return Ok(None);
        };
        ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        let wb = self.metadata.wb;
        let (width, height) = (w as usize, h as usize);
        let clipped = data
            .par_chunks_mut(width)
            .enumerate()
            .map(|(y, row)| {
                let mut clipped = 0u32;
                for (x, v) in row.iter_mut().enumerate() {
                    clipped += u32::from(*v >= 0.999);
                    *v *= wb[pattern[(y % crate::demosaic::PATTERN) * crate::demosaic::PATTERN
                        + x % crate::demosaic::PATTERN] as usize];
                }
                clipped
            })
            .sum();
        let cfa = crate::demosaic::Cfa {
            data: &data,
            width,
            height,
            pattern: &pattern,
            white: wb,
        };
        let (w, h, pixels) = if half {
            let (w, h, pixels) = crate::demosaic::half(&cfa);
            (w as u32, h as u32, pixels)
        } else {
            (w, h, crate::demosaic::demosaic(&cfa))
        };
        ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        Ok(Some(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: self.metadata.clone(),
            fast: half,
            scale_factor: 1.,
            scale_clipped: clipped,
        }))
    }
}
/// Sensor data that is not a mosaic developed: `rows` of `channels` samples
/// per pixel (red, green, blue first), each channel (raw − black) / (white −
/// black), white balanced by `wb`; what LibRaw's development made of small
/// RAWs, to within its 16-bit rounding.
pub(crate) fn develop_rgb(
    rows: &[&[u16]],
    channels: usize,
    black: [u32; 4],
    white: u32,
    wb: [f32; 3],
) -> Vec<[f32; 3]> {
    let scale: [f32; 3] = std::array::from_fn(|c| wb[c] / (white as f32 - black[c] as f32).max(1.));
    rows.par_iter()
        .flat_map_iter(|row| {
            row.chunks_exact(channels).map(move |p| {
                std::array::from_fn(|c| ((f32::from(p[c]) - black[c] as f32) * scale[c]).max(0.))
            })
        })
        .collect()
}
/// The frame of a header: its default crop in the decoded image
/// (`[left, top, width, height]`, as [`Metadata`]'s `crop_*`, before a RAF
/// header's recommended crop) and the camera's aspect-ratio crop in it.
fn crops(h: &Header) -> ([u32; 4], Option<[f32; 4]>) {
    let (frame, aspect) = sensor_frame(
        h.dng_version == 0 && h.make.eq_ignore_ascii_case("sony"),
        h.sensor_borders,
        h.frame,
        h.aspect,
        [h.width, h.height],
        h.margins,
    );
    (
        visible(frame, h.margins),
        camera_crop(frame, aspect, [h.width, h.height], h.margins),
    )
}
/// LibRaw's crop `[left, top, width, height]` in raw coordinates, in the decoded
/// image's, which start `margin` into the raw data. A crop without a position
/// (LibRaw's 0xffff) or starting in the margin keeps its size and is centred.
fn visible([left, top, width, height]: [u32; 4], [x, y]: [u32; 2]) -> [u32; 4] {
    let at = |v: u32, m: u32| {
        if v < UNSET && v >= m { v - m } else { UNSET }
    };
    [at(left, x), at(top, y), width, height]
}
/// LibRaw's mark for a crop position it does not know.
const UNSET: u32 = 0xffff;
/// The in-camera aspect-ratio crop `aspect` as `[left, top, right, bottom]`
/// fractions of the default crop `frame`, both raw-coordinate rectangles; `None`
/// unless both lie in the decoded image of `size` (after `margin`) and the aspect
/// crop cuts more than 1% from a side's length, so rounding is not a crop.
fn camera_crop(
    frame: [u32; 4],
    aspect: [u32; 4],
    size: [u32; 2],
    margin: [u32; 2],
) -> Option<[f32; 4]> {
    let fits = |r: [u32; 4]| {
        let [left, top, width, height] = visible(r, margin);
        width > 0
            && height > 0
            && left.checked_add(width).is_some_and(|v| v <= size[0])
            && top.checked_add(height).is_some_and(|v| v <= size[1])
    };
    if !fits(frame) || !fits(aspect) {
        return None;
    }
    let [fl, ft, fw, fh] = frame.map(|v| v as f32);
    let [al, at, aw, ah] = aspect.map(|v| v as f32);
    let crop = [
        (al - fl) / fw,
        (at - ft) / fh,
        (al + aw - fl) / fw,
        (at + ah - ft) / fh,
    ]
    .map(|v| v.clamp(0., 1.));
    let (w, h) = (crop[2] - crop[0], crop[3] - crop[1]);
    (w >= 0.01 && h >= 0.01 && w.min(h) < 0.99).then_some(crop)
}
/// The frame and the in-camera aspect-ratio crop, both raw-coordinate rectangles,
/// for files where LibRaw's default crop `frame` already is the aspect crop and
/// `aspect` is unset: Panasonic and Leica's RW2 crop tags replace the sensor
/// borders `sensor`, and a Sony ARW's default crop (`sony`, not a DNG's, which is
/// the frame) is the aspect ratio's. Camera Raw's frame is then the sensor's, and
/// the aspect ratio its starting crop. Other files keep LibRaw's rectangles.
fn sensor_frame(
    sony: bool,
    sensor: Option<[u32; 4]>,
    frame: [u32; 4],
    aspect: [u32; 4],
    size: [u32; 2],
    margin: [u32; 2],
) -> ([u32; 4], [u32; 4]) {
    if aspect[0] != UNSET || aspect[1] != UNSET {
        return (frame, aspect);
    }
    let widened = match sensor {
        Some(sensor) => Some((sensor, frame)),
        None if sony => sony_frame(frame, size, margin),
        None => None,
    };
    // Only a frame the crop is a camera crop of: otherwise (the same rectangle, a
    // pixel's rounding, borders outside the decoded image) LibRaw's is kept.
    widened
        .filter(|&(full, crop)| camera_crop(full, crop, size, margin).is_some())
        .unwrap_or((frame, aspect))
}
/// A Sony default crop `frame` at another ratio than the sensor's (3:2, or 4:3 on
/// the small-sensor compacts, whichever is nearer the decoded image of `size`),
/// widened along the side the ratio cuts to the sensor's ratio, as the frame and
/// the crop placed in raw coordinates (centred when LibRaw has no position).
/// `None` when the crop is at the sensor's ratio (within 1%) or the frame would
/// leave the decoded image.
fn sony_frame(
    [left, top, width, height]: [u32; 4],
    [w, h]: [u32; 2],
    [mx, my]: [u32; 2],
) -> Option<([u32; 4], [u32; 4])> {
    if width == 0 || height == 0 || width > w || height > h {
        return None;
    }
    let decoded = w as f32 / h as f32;
    let sensor = [1.5f32, 4. / 3.]
        .into_iter()
        .min_by(|a, b| (a - decoded).abs().total_cmp(&(b - decoded).abs()))?;
    let ratio = width as f32 / height as f32 / sensor;
    if (0.99..=1.01).contains(&ratio) {
        return None;
    }
    let (fw, fh) = if ratio > 1. {
        (width, (width as f32 / sensor).round() as u32)
    } else {
        ((height as f32 * sensor).round() as u32, height)
    };
    if fw > w || fh > h {
        return None;
    }
    // The crop's position in raw coordinates, centred when unknown; the frame is
    // centred on it and kept inside the decoded image.
    let place = |v: u32, len: u32, m: u32, span: u32| {
        if v < UNSET && v >= m {
            v
        } else {
            m + (span - len) / 2
        }
    };
    let (left, top) = (place(left, width, mx, w), place(top, height, my, h));
    let around = |v: u32, len: u32, flen: u32, m: u32, span: u32| {
        v.saturating_sub((flen - len) / 2).clamp(m, m + span - flen)
    };
    let frame = [
        around(left, width, fw, mx, w),
        around(top, height, fh, my, h),
        fw,
        fh,
    ];
    Some((frame, [left, top, width, height]))
}
/// The embedded preview, upright: a JPEG by its EXIF orientation, else the
/// RAW's; a JPEG XL (Lightroom's DNG previews) by its codestream's, else the
/// RAW's.
pub fn thumbnail(raw: &mut Raw) -> anyhow::Result<image::RgbImage> {
    let bytes = raw.thumbnail()?;
    preview_image(bytes, raw.metadata.flip)
}
/// A camera JPEG's colour space as its maker notes give it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakerPreviewSpace {
    Srgb,
    AdobeRgb,
}
/// Whether an embedded preview's `bytes` (`Raw::thumbnail`'s) are JPEG XL, which
/// Lightroom writes into DNGs, rather than a camera's JPEG.
pub fn is_jpeg_xl(bytes: &[u8]) -> bool {
    rawmakase_raw::is_jpeg_xl(bytes, 0)
}
/// The embedded preview for a thumbnail `edge` pixels on its long side,
/// upright as [`thumbnail`]'s: from the smallest preview that reaches `edge`
/// (see [`Raw::thumbnail_reaching`]), so a full-size JPEG is decoded only
/// when nothing smaller will do.
pub fn thumbnail_reaching(raw: &mut Raw, edge: u32) -> anyhow::Result<image::RgbImage> {
    let bytes = raw.thumbnail_reaching(edge)?;
    preview_image(bytes, raw.metadata.flip)
}
/// The most pixels a JPEG XL preview may claim: a 100-megapixel camera's.
/// Its header is read before any pixel, so a hostile one is refused before
/// the image is allocated.
const PREVIEW_PIXELS: u64 = 120_000_000;
/// An embedded preview's `bytes` decoded upright (see [`thumbnail`]); `flip`
/// is the RAW's orientation.
fn preview_image(bytes: Vec<u8>, flip: i32) -> anyhow::Result<image::RgbImage> {
    use image::{ImageDecoder, metadata::Orientation};
    let by_raw = match flip {
        3 => Orientation::Rotate180,
        5 => Orientation::Rotate270,
        6 => Orientation::Rotate90,
        _ => Orientation::NoTransforms,
    };
    let (mut im, orientation) = if rawmakase_raw::is_jpeg_xl(&bytes, 0) {
        // jxl-oxide renders the codestream's own orientation; the RAW's only
        // applies to one without.
        let header = jxl_oxide::JxlImage::builder()
            .read(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Could not read the embedded preview: {e}"))?;
        let (w, h) = (header.width(), header.height());
        anyhow::ensure!(
            u64::from(w) * u64::from(h) <= PREVIEW_PIXELS,
            "Embedded preview of {w} × {h} pixels"
        );
        let own = header.image_header().metadata.orientation;
        let decoder = jxl_oxide::integration::JxlDecoder::new(std::io::Cursor::new(bytes))?;
        let orientation = if own == 1 {
            by_raw
        } else {
            Orientation::NoTransforms
        };
        (image::DynamicImage::from_decoder(decoder)?, orientation)
    } else {
        let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))?;
        let orientation = match decoder.orientation()? {
            Orientation::NoTransforms => by_raw,
            exif => exif,
        };
        (image::DynamicImage::from_decoder(decoder)?, orientation)
    };
    im.apply_orientation(orientation);
    Ok(im.to_rgb8())
}

#[cfg(test)]
mod tests {
    /// A JPEG XL preview (Lightroom's DNGs embed one, of 1024 pixels, beside
    /// a 256-pixel JPEG) decodes, turned by the RAW's orientation:
    /// `rawmakase-raw`'s 8 × 4 lossless test codestream, whose pixel (x, y)
    /// is (x × 8191 + y × 3, y × 16000 + x, 65535 − x × 7000) of 65535.
    #[test]
    fn jpeg_xl_previews_decode_upright() {
        let jxl = include_bytes!("../../../rawmakase-raw/tests/data/jxl-int16-lossless.jxl");
        let im = super::preview_image(jxl.to_vec(), 0).unwrap();
        assert_eq!(im.dimensions(), (8, 4));
        let level = |v: u32| (f64::from(v) / 257.).round() as u8;
        for (x, y) in [(0, 0), (7, 3), (3, 1)] {
            let want = [x * 8191 + y * 3, y * 16000 + x, 65535 - x * 7000].map(level);
            assert_eq!(im.get_pixel(x, y).0, want, "({x}, {y})");
        }
        // Orientation 6: turned a quarter clockwise, so 4 × 8.
        let turned = super::preview_image(jxl.to_vec(), 6).unwrap();
        assert_eq!(turned.dimensions(), (4, 8));
        assert_eq!(turned.get_pixel(3, 0), im.get_pixel(0, 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn background_threads_are_ten_steps_nicer_than_their_parent() {
        // Field 19 of /proc/thread-self/stat, after the parenthesised name.
        fn nice() -> i32 {
            let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
            let fields = stat.rsplit(')').next().unwrap();
            fields.split_whitespace().nth(16).unwrap().parse().unwrap()
        }
        let parent = nice();
        let (tx, rx) = std::sync::mpsc::channel();
        super::spawn_background(move || tx.send(nice()).unwrap());
        assert_eq!(rx.recv().unwrap(), (parent + 10).min(19));
        assert_eq!(nice(), parent);
    }
    #[test]
    fn background_work_runs_its_parallel_work_on_two_background_threads() {
        let (tx, rx) = std::sync::mpsc::channel();
        super::spawn_background(move || {
            use rayon::prelude::*;
            let names: std::collections::BTreeSet<String> = (0..64)
                .into_par_iter()
                .map(|_| std::thread::current().name().unwrap_or("").to_string())
                .collect();
            tx.send((rayon::current_num_threads(), names)).unwrap();
        })
        .join()
        .unwrap();
        let (threads, names) = rx.recv().unwrap();
        assert_eq!(threads, 2);
        assert!(
            names.iter().all(|n| n.starts_with("background-")),
            "{names:?}"
        );
    }
    #[test]
    fn small_raws_develop_by_their_levels_and_white_balance() {
        let row: &[u16] = &[100, 1100, 2100, 0, 16383, 16383, 16383, 0];
        let pixels = super::develop_rgb(&[row], 4, [100, 100, 100, 100], 16383, [2., 1., 1.5]);
        let scale = 1. / 16283.;
        assert_eq!(pixels.len(), 2);
        for (got, want) in pixels[0]
            .iter()
            .zip([0., 1000. * scale, 2000. * scale * 1.5])
        {
            assert!((got - want).abs() < 1e-7, "{pixels:?}");
        }
        assert!((pixels[1][0] - 2.).abs() < 1e-6 && (pixels[1][1] - 1.).abs() < 1e-6);
    }
    #[test]
    fn reads_highlight_tone_priority_from_libraw() {
        use super::HighlightTonePriority as H;
        assert_eq!(H::from_libraw(0), H::Off);
        assert_eq!(H::from_libraw(1), H::On);
        assert_eq!(H::from_libraw(2), H::Enhanced);
        assert_eq!(H::from_libraw(-1), H::Off);
    }
    /// LibRaw's rectangles for a Canon EOS M6 Mark II CR3 shot at 1:1: SensorInfo's
    /// 6960×4640 frame and the AspectInfo square, in raw coordinates, with a visible
    /// image of 6984×4660 at (144, 72). Camera Raw crops it to the middle 2/3 of
    /// the frame (DNG Converter's DefaultUserCrop: 0, 1/6, 1, 5/6).
    #[test]
    fn canon_aspect_ratio_crop_is_a_crop_of_the_sensor_frame() {
        let (frame, square) = ([156, 84, 6960, 4640], [1316, 84, 4640, 4640]);
        let (size, margin) = ([6984, 4660], [144, 72]);
        assert_eq!(super::visible(frame, margin), [12, 12, 6960, 4640]);
        let crop = super::camera_crop(frame, square, size, margin).unwrap();
        for (got, want) in crop.iter().zip([1. / 6., 0., 5. / 6., 1.]) {
            assert!((got - want).abs() < 1e-6, "{crop:?}");
        }
        // 16:9 (DefaultUserCrop 0.0793, 0, 0.9207, 1).
        let wide = super::camera_crop(frame, [156, 452, 6960, 3904], size, margin).unwrap();
        assert!((wide[1] - 0.0793).abs() < 1e-4 && (wide[3] - 0.9207).abs() < 1e-4);
        // 3:2, and a crop LibRaw only rounded to even rows (PowerShot G15), crop nothing.
        assert_eq!(super::camera_crop(frame, frame, size, margin), None);
        let g15 = super::camera_crop(
            [128, 35, 4000, 3000],
            [128, 36, 4000, 2999],
            [4048, 3047],
            [104, 12],
        );
        assert_eq!(g15, None);
    }
    #[test]
    fn crops_without_a_position_or_outside_the_image_are_not_camera_crops() {
        let unset = [super::UNSET, super::UNSET, 0, 0];
        let (size, margin) = ([5200, 3904], [0, 0]);
        // Olympus 16:9: LibRaw's second inset crop inside the 4:3 frame.
        let frame = [8, 8, 5184, 3888];
        let wide = super::camera_crop(frame, [8, 494, 5184, 2916], size, margin).unwrap();
        assert_eq!(wide, [0., 0.125, 1., 0.875]);
        assert_eq!(super::camera_crop(frame, unset, size, margin), None);
        // Sony records only a size: its frame is centred, and nothing is cropped.
        let sony = [super::UNSET, super::UNSET, 6000, 4000];
        assert_eq!(super::visible(sony, [12, 8]), sony);
        assert_eq!(super::camera_crop(sony, frame, size, margin), None);
        // A crop reaching past the decoded image is not trusted.
        assert_eq!(
            super::camera_crop(frame, [8, 494, 5300, 2916], size, margin),
            None
        );
        // A position inside the masked margin is unknown.
        assert_eq!(
            super::visible([4, 4, 10, 10], [8, 8]),
            [super::UNSET, super::UNSET, 10, 10]
        );
    }
    /// Camera Raw's starting crop (its XMP `CropLeft`… on the default settings) of
    /// files whose LibRaw default crop is the camera's aspect ratio, from LibRaw's
    /// rectangles and the RW2 sensor borders of each file.
    #[test]
    fn panasonic_and_sony_aspect_ratios_are_crops_of_the_sensor_frame() {
        let crop = |make, sensor, frame, size| {
            let (frame, aspect) = super::sensor_frame(
                make == "Sony",
                sensor,
                frame,
                [super::UNSET; 4],
                size,
                [0, 0],
            );
            super::camera_crop(frame, aspect, size, [0, 0])
        };
        let near = |got: Option<[f32; 4]>, want: [f32; 4], within: f32| {
            let got = got.expect("a camera crop");
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < within),
                "{got:?} is not {want:?}"
            );
        };
        // DMC-FZ2500 at 4:3 and DMC-G85 at 1:1: CropLeft…CropBottom inside the sensor.
        let fz2500 = [8, 8, 5472, 3648];
        let got = crop(
            "Panasonic",
            Some(fz2500),
            [312, 8, 4864, 3648],
            [5488, 3664],
        );
        near(got, [0.055556, 0., 0.944444, 1.], 1e-5);
        let got = crop(
            "Panasonic",
            Some([8, 8, 4592, 3448]),
            [592, 20, 3424, 3424],
            [4608, 3464],
        );
        near(got, [0.127178, 0.00348, 0.872822, 0.99652], 1e-5);
        // LibRaw's frame stays when the crop tags are the sensor borders (the
        // sensor's own ratio), differ by its rounding (DMC-FZ50) or the borders
        // leave the decoded image (a DC-GH6 high-resolution 1:1 shot).
        let unset = super::UNSET;
        for (sensor, frame, size, margin) in [
            (fz2500, fz2500, [5488, 3664], [0, 0]),
            (
                [16, 7, 3648, 2736],
                [16, 8, 3648, 2735],
                [3671, 2748],
                [4, 0],
            ),
            (
                [4, 4, 11552, 8672],
                [1444, 4, 8672, 8672],
                [11544, 8664],
                [8, 8],
            ),
        ] {
            let got = super::sensor_frame(false, Some(sensor), frame, [unset; 4], size, margin);
            assert_eq!(got, (frame, [unset; 4]));
        }
        // ILCE-7M2 at 16:9 (a centred size only), ILCE-7RM3A at 16:9 and DSC-HX99's
        // 4:3 sensor at 16:9.
        near(
            crop("Sony", None, [unset, unset, 6000, 3376], [6024, 4024]),
            [0., 0.078, 1., 0.922],
            1e-5,
        );
        near(
            crop("Sony", None, [0, 422, 7952, 4472], [7968, 5320]),
            [0., 0.078431, 1., 0.921569],
            5e-4,
        );
        near(
            crop("Sony", None, [12, 478, 4896, 2752], [4928, 3708]),
            [0., 0.125272, 1., 0.874728],
            1e-5,
        );
        // A crop at the decoded image's edge keeps the frame inside it.
        let edge = super::sony_frame([0, 0, 6000, 3376], [6024, 4024], [0, 0]);
        assert_eq!(edge, Some(([0, 0, 6000, 4000], [0, 0, 6000, 3376])));
        // Sony crops at the sensor's ratio stay the frame, also in the DSC-RX1RM3's
        // smaller image sizes, whose decoded image is not 3:2.
        for (frame, size) in [
            ([unset, unset, 6000, 4000], [6024, 4024]),
            ([16, 10, 4752, 3168], [5120, 3584]),
            ([22, 14, 6640, 4432], [7168, 4608]),
            ([0, 20, 9504, 6336], [9568, 6376]),
        ] {
            let got = super::sensor_frame(true, None, frame, [unset; 4], size, [0, 0]);
            assert_eq!(got, (frame, [unset; 4]));
        }
        // An aspect crop LibRaw found (Olympus, Canon) is kept, whatever the make.
        let olympus = ([8, 8, 5184, 3888], [8, 494, 5184, 2916]);
        let got = super::sensor_frame(
            true,
            Some(fz2500),
            olympus.0,
            olympus.1,
            [5200, 3904],
            [0, 0],
        );
        assert_eq!(got, olympus);
    }
    #[test]
    fn corrupt_raw_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a raw file").unwrap();
        assert!(super::Raw::open_file(f.path()).is_err());
    }
    /// A 64×64 NEF from `model` whose raw data, tagged Nikon-compressed, starts with `data`.
    fn nef(model: &str, data: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        fn dir(f: &mut Vec<u8>, entries: &[(u16, u16, u32, Vec<u8>)]) {
            let mut extra = f.len() + 2 + 12 * entries.len() + 4;
            let mut values: Vec<u8> = Vec::new();
            f.extend((entries.len() as u16).to_le_bytes());
            for (tag, kind, count, value) in entries {
                f.extend([tag.to_le_bytes(), kind.to_le_bytes()].concat());
                f.extend(count.to_le_bytes());
                if value.len() <= 4 {
                    f.extend(&value[..]);
                    f.extend(vec![0; 4 - value.len()]);
                } else {
                    f.extend((extra as u32).to_le_bytes());
                    values.extend(value);
                    extra += value.len();
                }
            }
            f.extend(0u32.to_le_bytes());
            f.extend(values);
        }
        let text = |s: &str| [s.as_bytes(), &[0]].concat();
        let long = |v: u32| v.to_le_bytes().to_vec();
        let short = |v: u16| v.to_le_bytes().to_vec();
        let (make, model) = (text("NIKON CORPORATION"), text(model));
        let mut f = b"II*\0".to_vec();
        f.extend(8u32.to_le_bytes());
        // The raw directory follows the first one and its two strings.
        let sub = 8 + 2 + 3 * 12 + 4 + make.len() + model.len();
        let data_offset = sub + 2 + 9 * 12 + 4;
        let raw: Vec<u8> = data
            .iter()
            .copied()
            .chain((0..4096).map(|i| i as u8))
            .collect();
        dir(
            &mut f,
            &[
                (0x10f, 2, make.len() as u32, make.clone()),
                (0x110, 2, model.len() as u32, model.clone()),
                (0x14a, 4, 1, long(sub as u32)),
            ],
        );
        assert_eq!(f.len(), sub);
        dir(
            &mut f,
            &[
                (0xfe, 4, 1, long(0)),
                (0x100, 4, 1, long(64)),
                (0x101, 4, 1, long(64)),
                (0x102, 3, 1, short(14)),
                (0x103, 3, 1, short(34713)),
                (0x106, 3, 1, short(32803)),
                (0x111, 4, 1, long(data_offset as u32)),
                (0x115, 3, 1, short(1)),
                (0x117, 4, 1, long(raw.len() as u32)),
            ],
        );
        assert_eq!(f.len(), data_offset);
        f.extend(raw);
        // A closed file, written and closed before it is opened.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.nef");
        std::fs::write(&path, f).unwrap();
        (dir, path)
    }
    #[test]
    fn nikon_high_efficiency_raws_are_decoded_on_every_body() {
        use crate::camera_data::{Decode, Demosaic};
        use std::sync::atomic::AtomicBool;
        // Zeros after it, inside this small file's raw data.
        let stream = [
            rawmakase_raw::nikon_high_efficiency_test_support::blank_codestream(64, 64),
            vec![0; 2048],
        ]
        .concat();
        // Both bodies' High Efficiency raws decode, the Z5_2's too (issue #411).
        for model in ["NIKON Z5_2", "NIKON Z 8"] {
            let (_dir, path) = nef(model, &stream);
            let sensor = super::Raw::open_file(&path).unwrap().sensor().unwrap();
            let value =
                rawmakase_raw::nikon_high_efficiency_test_support::blank_value().to_le_bytes();
            let expected: [u8; 32] =
                <sha2::Sha256 as sha2::Digest>::digest(value.repeat(64 * 64)).into();
            assert_eq!(sensor.data_sha256, expected, "{model}");
            for decode in [Decode::Half, Decode::Full(Demosaic::Rawmakase)] {
                let raw = super::Raw::open_file(&path).unwrap();
                let image = raw
                    .develop(decode, &AtomicBool::new(false))
                    .unwrap_or_else(|e| panic!("{model}, {decode:?}: {e}"));
                // The blank codestream is one value throughout.
                let first = image.pixels[0];
                assert!(first.iter().all(|&v| v > 0.0), "{model}, {decode:?}");
                for p in &image.pixels {
                    for c in 0..3 {
                        assert!((p[c] - first[c]).abs() <= 1e-4, "{model}, {decode:?}");
                    }
                }
            }
        }
    }
    #[test]
    fn broken_nikon_high_efficiency_raws_are_errors() {
        use crate::camera_data::{Decode, Demosaic};
        use std::sync::atomic::AtomicBool;
        // JPEG XS's start of codestream and capabilities marker, then noise.
        const JPEG_XS: [u8; 4] = [0xff, 0x10, 0xff, 0x50];
        for model in ["NIKON Z5_2", "NIKON Z 8"] {
            for decode in [Decode::Half, Decode::Full(Demosaic::Rawmakase)] {
                let (_dir, path) = nef(model, &JPEG_XS);
                // Opened, so the embedded preview can stand in.
                let raw = super::Raw::open_file(&path).unwrap();
                // Trying again after a failed unpack fails the same way.
                for _ in 0..2 {
                    let error = raw.sensor().err().expect("unpacked");
                    assert!(error.to_string().contains("JPEG XS"), "{error}");
                }
                let error = raw
                    .develop(decode, &AtomicBool::new(false))
                    .err()
                    .unwrap_or_else(|| panic!("{model} developed with {decode:?}"));
                assert!(
                    error.to_string().contains("JPEG XS"),
                    "{model}, {decode:?}: {error}"
                );
            }
        }
        let (_dir, path) = nef("NIKON Z5_2", &[0; 4]);
        let raw = super::Raw::open_file(&path).unwrap();
        assert!(raw.develop(Decode::Half, &AtomicBool::new(false)).is_ok());
    }
}
