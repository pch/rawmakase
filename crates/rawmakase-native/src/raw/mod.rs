//! RAW files: opening them through LibRaw, reading their metadata (with the
//! DNG, RAF and lens details read on top), development into linear camera-space
//! pixels, the embedded preview, and the monitor colour transform. The values
//! they produce are [`crate::camera_data`]'s.
//! The native boundary itself is in [`ffi`].
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
mod ffi;
mod nikon_he;
use crate::camera_data::{
    CameraImage, Decode, Demosaic, HighlightTonePriority, Metadata, Source, WhiteBalancePresets,
};
pub(crate) use ffi::Sensor;
pub use ffi::{display_transform, srgb_profile, version};
/// Runs `work` on a thread of its own at background priority: a lower
/// scheduling priority, and LibRaw decodes on two OpenMP threads. Every worker
/// that reads or decodes photos behind the user's back starts here or in
/// [`background_pool`], so none competes with Develop for the CPU.
pub fn spawn_background(work: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        ffi::background_thread();
        work();
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
        .start_handler(|_| ffi::background_thread())
        .build()
}
pub struct Raw {
    handle: ffi::Handle,
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
impl Raw {
    /// LibRaw's facts about the file at `path`, and the crop a RAF header
    /// recommends. What the file says beyond them (lens tables, a DNG's profile
    /// and hints) and the imported lens profiles are `crate::photo::open`'s.
    pub fn open_file(path: &Path) -> Result<Self> {
        let path_ref = path;
        let (handle, m) = ffi::Handle::open(path)?;
        let text = |bytes: &[std::ffi::c_char]| {
            let bytes: Vec<u8> = bytes.iter().map(|&c| c as u8).collect();
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        let (make, model) = (text(&m.make), text(&m.model));
        let adobe_names = rawmakase_model::cameras::adobe_names(
            &make,
            &model,
            (&text(&m.normalized_make), &text(&m.normalized_model)),
        );
        let margin = [m.left_margin, m.top_margin];
        let facts = Facts {
            margins: margin,
            frame: [m.crop_left, m.crop_top, m.crop_width, m.crop_height],
            aspect: [m.aspect_left, m.aspect_top, m.aspect_width, m.aspect_height],
            normalized: [text(&m.normalized_make), text(&m.normalized_model)],
            dng_version: m.dng_version,
        };
        let (frame, aspect) = sensor_frame(
            m.dng_version == 0 && make.eq_ignore_ascii_case("sony"),
            panasonic_sensor(path),
            [m.crop_left, m.crop_top, m.crop_width, m.crop_height],
            [m.aspect_left, m.aspect_top, m.aspect_width, m.aspect_height],
            [m.width, m.height],
            margin,
        );
        let [crop_left, crop_top, crop_width, crop_height] = visible(frame, margin);
        let metadata = Metadata {
            make,
            model,
            width: m.width,
            height: m.height,
            raw_width: m.raw_width,
            raw_height: m.raw_height,
            crop_width,
            crop_height,
            crop_left,
            crop_top,
            camera_crop: camera_crop(frame, aspect, [m.width, m.height], margin),
            flip: m.flip,
            xtrans: m.xtrans != 0,
            fuji_dynamic_range: m.fuji_dynamic_range,
            highlight_tone_priority: HighlightTonePriority::from_libraw(m.highlight_tone_priority),
            sraw: m.sraw != 0,
            // LibRaw leaves -999 when the maker notes have no shift.
            fuji_exposure_shift: (m.fuji_exposure_shift > -100.).then_some(m.fuji_exposure_shift),
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal: m.focal,
            focal_35mm: m.focal_35mm,
            wb: m.wb,
            daylight_wb: m.daylight_wb,
            wb_presets: WhiteBalancePresets {
                daylight: WhiteBalancePresets::valid(m.preset_daylight),
                shade: WhiteBalancePresets::valid(m.preset_shade),
                d65: WhiteBalancePresets::valid(m.preset_d65),
                fine_weather: WhiteBalancePresets::valid(m.preset_fine_weather),
            },
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| m.matrix[r * 3 + c])),
            cam_xyz: std::array::from_fn(|r| std::array::from_fn(|c| m.cam_xyz[r * 3 + c])),
            lens: None,
            lens_model: text(&m.lens).trim().to_string(),
            lens_id: m.lens_id,
            adobe_names,
            baseline_exposure: None,
            dng_neutral_calibration: None,
            dng_matrix_profile_signature: None,
            source: Source::Raw,
            lens_profiles: Default::default(),
            lateral_ca: Default::default(),
            embedded_dcp: None,
        };
        let mut metadata = metadata;
        if let Some(crop) = fuji_crop(path_ref) {
            metadata.apply_default_crop(visible(crop, margin));
        }
        Ok(Self {
            handle,
            metadata,
            facts,
        })
    }
    /// The embedded JPEG preview, as stored.
    pub fn thumbnail(&mut self) -> Result<Vec<u8>> {
        self.handle.thumbnail()
    }
    /// The unpacked sensor data as development reads it, without developing.
    pub(crate) fn sensor(&self) -> Result<Sensor> {
        self.handle.sensor()
    }
    pub fn develop(self, decode: Decode, cancel: &AtomicBool) -> Result<CameraImage> {
        match decode {
            Decode::Half => self.develop_libraw(true, cancel),
            Decode::Full(Demosaic::Libraw) => self.develop_libraw(false, cancel),
            // LibRaw's demosaic when the file is not Bayer or X-Trans data.
            Decode::Full(Demosaic::Rawmakase) => match self.develop_cfa(cancel)? {
                Some(image) => Ok(image),
                None => self.develop_libraw(false, cancel),
            },
        }
    }
    /// Unpacked CFA data demosaiced by `crate::demosaic`; `None` when the file is not
    /// single-channel Bayer or X-Trans data.
    fn develop_cfa(&self, cancel: &AtomicBool) -> Result<Option<CameraImage>> {
        let Some(ffi::Cfa {
            width: w,
            height: h,
            pattern,
            mut data,
        }) = self.handle.cfa()?
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
        let pixels = crate::demosaic::demosaic(&crate::demosaic::Cfa {
            data: &data,
            width,
            height,
            pattern: &pattern,
        });
        ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        Ok(Some(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: self.metadata.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: clipped,
        }))
    }
    fn develop_libraw(self, fast: bool, cancel: &AtomicBool) -> Result<CameraImage> {
        let ffi::Developed {
            width: w,
            height: h,
            scale,
            clipped,
            pixels,
        } = self.handle.develop(fast, cancel)?;
        Ok(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: self.metadata,
            fast,
            scale_factor: scale,
            scale_clipped: clipped,
        })
    }
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
/// The sensor borders of a Panasonic or Leica RW2 (RWL) header, tags 4 to 7 (top,
/// left, bottom, right), as `[left, top, width, height]` in raw coordinates.
/// LibRaw replaces them with the camera's aspect-ratio crop (tags 47 to 50).
fn panasonic_sensor(path: &Path) -> Option<[u32; 4]> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"IIU\0" {
        return None;
    }
    f.seek(SeekFrom::Start(
        u32::from_le_bytes(head[4..].try_into().ok()?) as u64,
    ))
    .ok()?;
    let mut count = [0u8; 2];
    f.read_exact(&mut count).ok()?;
    let mut b = vec![0; usize::from(u16::from_le_bytes(count)).min(1024) * 12];
    f.read_exact(&mut b).ok()?;
    let mut border = [None; 4];
    for e in b.as_chunks::<12>().0 {
        let tag = u16::from_le_bytes([e[0], e[1]]);
        if (4..=7).contains(&tag) && u16::from_le_bytes([e[2], e[3]]) == 3 {
            border[usize::from(tag - 4)] = Some(u32::from(u16::from_le_bytes([e[8], e[9]])));
        }
    }
    let [Some(top), Some(left), Some(bottom), Some(right)] = border else {
        return None;
    };
    (right > left && bottom > top).then(|| [left, top, right - left, bottom - top])
}
/// The camera's recommended crop from the RAF header directory: tags 0x110 (top, left)
/// and 0x111 (height, width), big-endian. Lightroom uses it as the default crop.
fn fuji_crop(path: &Path) -> Option<[u32; 4]> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 100];
    f.read_exact(&mut head).ok()?;
    if !head.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let dir = u32::from_be_bytes(head[92..96].try_into().ok()?) as u64;
    let len = u32::from_be_bytes(head[96..100].try_into().ok()?) as usize;
    if !(4..=1 << 20).contains(&len) {
        return None;
    }
    let mut b = vec![0; len];
    f.seek(SeekFrom::Start(dir)).ok()?;
    f.read_exact(&mut b).ok()?;
    let be16 = |o: usize| Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?) as u32);
    let count = u32::from_be_bytes(b[..4].try_into().ok()?) as usize;
    let (mut o, mut origin, mut size) = (4, None, None);
    for _ in 0..count.min(256) {
        let (tag, n) = (be16(o)?, be16(o + 2)? as usize);
        if n == 4 && tag == 0x110 {
            origin = Some([be16(o + 6)?, be16(o + 4)?]);
        } else if n == 4 && tag == 0x111 {
            size = Some([be16(o + 6)?, be16(o + 4)?]);
        }
        o += 4 + n;
    }
    let ([left, top], [width, height]) = (origin?, size?);
    (width > 0 && height > 0).then_some([left, top, width, height])
}
pub fn thumbnail(raw: &mut Raw) -> anyhow::Result<image::RgbImage> {
    use image::{ImageDecoder, metadata::Orientation};
    let bytes = raw.thumbnail()?;
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))?;
    let mut orientation = decoder.orientation()?;
    if orientation == Orientation::NoTransforms {
        orientation = match raw.metadata.flip {
            3 => Orientation::Rotate180,
            5 => Orientation::Rotate270,
            6 => Orientation::Rotate90,
            _ => Orientation::NoTransforms,
        };
    }
    let mut im = image::DynamicImage::from_decoder(decoder)?;
    im.apply_orientation(orientation);
    Ok(im.to_rgb8())
}

#[cfg(test)]
mod tests {
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
    fn reads_highlight_tone_priority_from_libraw() {
        use super::HighlightTonePriority as H;
        assert_eq!(H::from_libraw(0), H::Off);
        assert_eq!(H::from_libraw(1), H::On);
        assert_eq!(H::from_libraw(2), H::Enhanced);
        assert_eq!(H::from_libraw(-1), H::Off);
    }
    /// A 64×48 uncompressed Canon TIFF claiming `model`, whose maker notes hold
    /// `notes`: (tag, type, count, value) with every value stored after the image.
    /// Written into a folder of its own: Windows does not let LibRaw open a
    /// `NamedTempFile` while it is held.
    fn canon_raw(
        model: &str,
        notes: &[(u16, u16, u32, Vec<u8>)],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let le = |v: u32, n: usize| v.to_le_bytes()[..n].to_vec();
        // IFD entries (tag, type, count, value) with values of up to four bytes.
        let ifd = |entries: &[(u16, u16, u32, Vec<u8>)]| {
            let mut b = le(entries.len() as u32, 2);
            for (tag, kind, count, value) in entries {
                b.extend(le(*tag as u32, 2));
                b.extend(le(*kind as u32, 2));
                b.extend(le(*count, 4));
                b.extend(value.iter().copied().chain([0; 4]).take(4));
            }
            b.extend(le(0, 4));
            b
        };
        let (make, model_at, exif, notes_at, image) = (192, 200, 300, 400, 1024);
        let (width, height) = (64u32, 48u32);
        let size = width * height * 2;
        let mut file = vec![0u8; (image + size) as usize];
        let mut entries = Vec::new();
        for (tag, kind, count, value) in notes {
            entries.push((*tag, *kind, *count, le(file.len() as u32, 4)));
            file.extend(value);
        }
        let mut put = |at: u32, bytes: &[u8]| {
            file[at as usize..at as usize + bytes.len()].copy_from_slice(bytes)
        };
        let model = format!("{model}\0");
        put(0, b"II*\0\x08\0\0\0");
        put(make, b"Canon\0");
        put(model_at, model.as_bytes());
        put(
            8,
            &ifd(&[
                (0x0100, 4, 1, le(width, 4)),
                (0x0101, 4, 1, le(height, 4)),
                (0x0102, 3, 1, le(16, 2)),
                (0x0103, 3, 1, le(1, 2)),
                (0x0106, 3, 1, le(32803, 2)),
                (0x010f, 2, 6, le(make, 4)),
                (0x0110, 2, model.len() as u32, le(model_at, 4)),
                (0x0111, 4, 1, le(image, 4)),
                (0x0115, 3, 1, le(1, 2)),
                (0x0116, 4, 1, le(height, 4)),
                (0x0117, 4, 1, le(size, 4)),
                (0x828d, 3, 2, [le(2, 2), le(2, 2)].concat()),
                (0x828e, 1, 4, vec![0, 1, 1, 2]),
                (0x8769, 4, 1, le(exif, 4)),
            ]),
        );
        put(exif, &ifd(&[(0x927c, 7, 18, le(notes_at, 4))]));
        put(notes_at, &ifd(&entries));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("canon.tif");
        std::fs::write(&path, file).unwrap();
        (dir, path)
    }
    /// A Canon EOS 60D TIFF whose maker notes hold only CustomFunctions2 with
    /// Highlight Tone Priority (record 0x0203) at `htp`, as bodies from the 40D
    /// to the 600D record it: LibRaw reads none of the 60D's and a CameraInfo byte
    /// of the 550D's that says Off when the custom function is on.
    fn canon_custom_functions_raw(htp: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        let le = |v: u32, n: usize| v.to_le_bytes()[..n].to_vec();
        let records = [(0x0201, 0), (0x0202, 0), (0x0203, htp)];
        let mut group = [2, 8 + 12 * records.len() as u32, records.len() as u32]
            .iter()
            .flat_map(|&v| le(v, 4))
            .collect::<Vec<_>>();
        group.extend(
            records
                .iter()
                .flat_map(|&(tag, v)| [tag, 1, v])
                .flat_map(|v| le(v, 4)),
        );
        let mut custom = le(8 + group.len() as u32, 2);
        custom.extend(le(0, 2));
        custom.extend(le(1, 4));
        custom.extend(group);
        let words = custom.len() as u32 / 4;
        canon_raw("Canon EOS 60D", &[(0x0099, 4, words, custom)])
    }
    /// A Canon TIFF claiming `model` whose ColorData (0x4001) is 3778 words of
    /// `version`, with the per-channel blacks (red, green, green, blue) where
    /// ColorData11 has them and where ColorData12 does.
    fn canon_color_data_raw(
        model: &str,
        version: u16,
        black11: [u16; 4],
        black12: [u16; 4],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let mut words = vec![0u16; 3778];
        words[0] = version;
        words[0x69..0x6d].copy_from_slice(&[2320, 1024, 1024, 1454]);
        words[0x27c..0x280].copy_from_slice(&black11);
        words[0x280..0x282].copy_from_slice(&[164, 172]);
        words[0x290..0x294].copy_from_slice(&black12);
        words[0x294..0x296].copy_from_slice(&[13200, 13995]);
        let bytes = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        canon_raw(model, &[(0x4001, 3, 3778, bytes)])
    }
    #[test]
    fn reads_black_from_canon_color_data_12_of_3778_words() {
        let black = |model, version, black11, black12| {
            let (_dir, path) = canon_color_data_raw(model, version, black11, black12);
            super::Raw::open_file(&path)
                .unwrap()
                .sensor()
                .unwrap()
                .black
        };
        // The EOS R6 Mark III (version 66) and R50 V (65) write ColorData12 in a
        // record as long as ColorData11; LibRaw reads it as ColorData11 and gets
        // the small values there (pink renders when the masked border cannot
        // correct them). Channels are LibRaw's: red, green, blue, second green.
        let garbage = [0, 58, 113, 145];
        assert_eq!(
            black("Canon EOS R6 Mark III", 66, garbage, [512; 4]),
            [512; 4]
        );
        assert_eq!(
            black("Canon EOS R50 V", 65, garbage, [2044, 2047, 2046, 2043]),
            [2044, 2047, 2043, 2046]
        );
        // ColorData11 itself (the R6 Mark II's version 48) keeps LibRaw's reading.
        assert_eq!(
            black("Canon EOS R6 Mark II", 48, [511, 512, 513, 514], [0; 4]),
            [511, 512, 514, 513]
        );
    }
    #[test]
    fn reads_highlight_tone_priority_from_canon_custom_functions() {
        use super::HighlightTonePriority as H;
        let open = |htp| {
            let (_dir, path) = canon_custom_functions_raw(htp);
            let raw = super::Raw::open_file(&path).unwrap();
            assert_eq!(
                (&*raw.metadata.make, &*raw.metadata.model),
                ("Canon", "EOS 60D")
            );
            raw.metadata.highlight_tone_priority
        };
        assert_eq!(open(1), H::On);
        assert_eq!(open(0), H::Off);
    }
    #[test]
    fn reads_fujifilm_default_crop() {
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(128, 0);
        let mut dir = 2u32.to_be_bytes().to_vec();
        for (tag, a, b) in [(0x110u16, 16u16, 16u16), (0x111, 4000, 6000)] {
            dir.extend(tag.to_be_bytes());
            dir.extend(4u16.to_be_bytes());
            dir.extend(a.to_be_bytes());
            dir.extend(b.to_be_bytes());
        }
        raf[92..96].copy_from_slice(&128u32.to_be_bytes());
        raf[96..100].copy_from_slice(&(dir.len() as u32).to_be_bytes());
        raf.extend(dir);
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), raf).unwrap();
        assert_eq!(super::fuji_crop(f.path()), Some([16, 16, 6000, 4000]));
        std::fs::write(f.path(), b"FUJIFILMCCD-RAW").unwrap();
        assert_eq!(super::fuji_crop(f.path()), None);
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
    fn reads_panasonic_sensor_borders() {
        let mut rw2 = b"IIU\0".to_vec();
        rw2.extend(8u32.to_le_bytes());
        let tags = [
            (4u16, 8u16),
            (5, 8),
            (6, 3656),
            (7, 5480),
            (0x2f, 8),
            (0x30, 312),
        ];
        rw2.extend((tags.len() as u16).to_le_bytes());
        for (tag, v) in tags {
            rw2.extend(tag.to_le_bytes());
            rw2.extend(3u16.to_le_bytes());
            rw2.extend(1u32.to_le_bytes());
            rw2.extend(u32::from(v).to_le_bytes());
        }
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), &rw2).unwrap();
        assert_eq!(super::panasonic_sensor(f.path()), Some([8, 8, 5472, 3648]));
        // Reversed borders, without every border, or in a TIFF, there is none.
        let right = rw2.len() - 3 * 12 + 8;
        let mut reversed = rw2.clone();
        reversed[right..right + 2].copy_from_slice(&0u16.to_le_bytes());
        std::fs::write(f.path(), &reversed).unwrap();
        assert_eq!(super::panasonic_sensor(f.path()), None);
        std::fs::write(f.path(), &rw2[..rw2.len() - 4 * 12]).unwrap();
        assert_eq!(super::panasonic_sensor(f.path()), None);
        rw2[2] = b'*';
        std::fs::write(f.path(), &rw2).unwrap();
        assert_eq!(super::panasonic_sensor(f.path()), None);
    }
    #[test]
    fn corrupt_raw_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a raw file").unwrap();
        assert!(super::Raw::open_file(f.path()).is_err());
    }
    /// A 64×64 NEF from `model` whose raw data, Nikon-compressed for LibRaw, starts with `data`.
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
        // A closed file: Windows' LibRaw cannot open one a NamedTempFile holds open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.nef");
        std::fs::write(&path, f).unwrap();
        (dir, path)
    }
    #[test]
    fn nikon_high_efficiency_raws_are_decoded_on_every_body() {
        use crate::camera_data::{Decode, Demosaic};
        use std::sync::atomic::AtomicBool;
        // Zeros after it: without maker notes, LibRaw's open reads a Nikon curve
        // length from file offset 2120, inside this small file's raw data.
        let stream = [
            super::nikon_he::tests::blank_codestream(64, 64),
            vec![0; 2048],
        ]
        .concat();
        // LibRaw knows the Z 8's High Efficiency raws but cannot decode them, and
        // reads the Z5_2's as lossless compressed: noise and a warning, not an error
        // (issue #411).
        for model in ["NIKON Z5_2", "NIKON Z 8"] {
            let (_dir, path) = nef(model, &stream);
            let sensor = super::Raw::open_file(&path).unwrap().sensor().unwrap();
            let value = super::nikon_he::tests::blank_value().to_le_bytes();
            let expected: [u8; 32] =
                <sha2::Sha256 as sha2::Digest>::digest(value.repeat(64 * 64)).into();
            assert_eq!(sensor.data_sha256, expected, "{model}");
            for decode in [
                Decode::Half,
                Decode::Full(Demosaic::Libraw),
                Decode::Full(Demosaic::Rawmakase),
            ] {
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
            for decode in [
                Decode::Half,
                Decode::Full(Demosaic::Libraw),
                Decode::Full(Demosaic::Rawmakase),
            ] {
                let (_dir, path) = nef(model, &JPEG_XS);
                // Opened, so the embedded preview can stand in.
                let raw = super::Raw::open_file(&path).unwrap();
                // LibRaw recycles itself after a failed unpack; trying again
                // fails the same way.
                for _ in 0..2 {
                    let error = raw.sensor().err().expect("unpacked");
                    assert!(error.to_string().contains("High Efficiency"), "{error}");
                }
                let error = raw
                    .develop(decode, &AtomicBool::new(false))
                    .err()
                    .unwrap_or_else(|| panic!("{model} developed with {decode:?}"));
                assert!(
                    error.to_string().contains("High Efficiency"),
                    "{model}, {decode:?}: {error}"
                );
            }
        }
        let (_dir, path) = nef("NIKON Z5_2", &[0; 4]);
        let raw = super::Raw::open_file(&path).unwrap();
        assert!(raw.develop(Decode::Half, &AtomicBool::new(false)).is_ok());
    }
    #[test]
    fn monitor_srgb_roundtrip() -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let p = d.path().join("srgb.icc");
        std::fs::write(&p, super::srgb_profile()?)?;
        let mut rgb = vec![12, 128, 240, 255, 0, 100];
        let before = rgb.clone();
        super::display_transform(&p, &mut rgb)?;
        for (a, b) in rgb.iter().zip(before) {
            assert!((i16::from(*a) - i16::from(b)).abs() <= 1);
        }
        Ok(())
    }
}
