//! The C ABI of `native/raw.cpp`: its declarations, the metadata struct both sides
//! share by layout, and one safe wrapper per entry point that states what the call
//! relies on. Nothing else in the crate calls LibRaw directly.
//!
//! Contracts the native side keeps, which the `SAFETY` comments below rely on:
//!
//! - Every function that takes `err` writes at most [`ERR`] bytes into it,
//!   NUL-terminated, and only when it fails.
//! - `ora_open` returns a handle that stays valid until `ora_close`, which is the
//!   only way it is freed. Each handle is used from one thread at a time.
//! - `ora_develop` and `ora_cfa_open` report the size whose pixels the matching
//!   copy function then writes in full: `width × height × 3` floats for
//!   `ora_copy`, `width × height` for `ora_cfa_copy`.
//! - `ora_open` keeps the `HighEfficiency` decoder it is given and calls it only from
//!   the handle's unpack, with the codestream it read and LibRaw's raw buffer, both
//!   valid for that call.
//! - `ora_thumbnail` and `ora_sensor` point into the handle's own buffers, valid
//!   until the next call on that handle; `ora_sensor`'s data holds `height` rows
//!   of `pitch` values and its black levels `black_size` values.
use super::{Cfa, Header, Sensor};
use anyhow::{Result, ensure};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Mirrors `struct Metadata` in `native/raw.cpp` field for field. A change on either
/// side must be made on the other; `native_metadata_layout_matches` compares the
/// sizes at test time and the assertion below keeps this side from drifting alone.
#[repr(C)]
pub(super) struct NativeMetadata {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) raw_width: u32,
    pub(super) raw_height: u32,
    pub(super) crop_width: u32,
    pub(super) crop_height: u32,
    pub(super) crop_left: u32,
    pub(super) crop_top: u32,
    pub(super) flip: i32,
    pub(super) xtrans: i32,
    pub(super) fuji_dynamic_range: u32,
    pub(super) iso: f32,
    pub(super) shutter: f32,
    pub(super) aperture: f32,
    pub(super) focal: f32,
    pub(super) wb: [f32; 3],
    pub(super) daylight_wb: [f32; 3],
    pub(super) matrix: [f32; 9],
    pub(super) make: [c_char; 64],
    pub(super) model: [c_char; 64],
    pub(super) cam_xyz: [f32; 9],
    pub(super) lens: [c_char; 128],
    pub(super) focal_35mm: f32,
    pub(super) highlight_tone_priority: i32,
    pub(super) fuji_exposure_shift: f32,
    pub(super) preset_daylight: [f32; 3],
    pub(super) preset_shade: [f32; 3],
    pub(super) preset_d65: [f32; 3],
    pub(super) preset_fine_weather: [f32; 3],
    pub(super) left_margin: u32,
    pub(super) top_margin: u32,
    pub(super) aspect_left: u32,
    pub(super) aspect_top: u32,
    pub(super) aspect_width: u32,
    pub(super) aspect_height: u32,
    /// The maker's numeric lens ID (Nikon's `LensIDNumber`, Pentax's `LensType`); 0
    /// when there is none.
    pub(super) lens_id: u32,
    /// LibRaw's canonical name for bodies sold under several names.
    pub(super) normalized_make: [c_char; 64],
    pub(super) normalized_model: [c_char; 64],
    /// The DNG version, 0 for a camera's own format.
    pub(super) dng_version: u32,
    /// A small raw (Canon sRAW and mRAW, Nikon sRAW).
    pub(super) sraw: i32,
}
const _: () = assert!(std::mem::size_of::<NativeMetadata>() == 636);
/// Mirrors `struct Sensor` in `native/raw.cpp`, as [`NativeMetadata`] does `Metadata`;
/// `native_sensor_layout_matches` compares the sizes.
#[repr(C)]
struct NativeSensor {
    data: *const u16,
    width: u32,
    height: u32,
    pitch: u32,
    channels: u32,
    left: u32,
    top: u32,
    area_width: u32,
    area_height: u32,
    black: *const u32,
    base_black: u32,
    black_size: u32,
    white: u32,
    linear_white: [u32; 4],
    pattern_rows: u32,
    pattern_cols: u32,
    pattern: [u8; 256],
}

/// Size of the error buffers `native/raw.cpp`'s `message` writes into.
const ERR: usize = 512;

unsafe extern "C" {
    fn ora_version() -> *const c_char;
    fn ora_background_thread();
    fn ora_metadata_size() -> u32;
    fn ora_open(
        path: *const c_char,
        m: *mut NativeMetadata,
        high_efficiency: HighEfficiency,
        err: *mut c_char,
    ) -> *mut c_void;
    fn ora_close(h: *mut c_void);
    fn ora_develop(
        h: *mut c_void,
        fast: c_int,
        cancel: extern "C" fn(*mut c_void) -> c_int,
        ctx: *mut c_void,
        w: *mut u32,
        height: *mut u32,
        gain: *mut f32,
        scale: *mut f32,
        clipped: *mut u32,
        err: *mut c_char,
    ) -> c_int;
    fn ora_copy(h: *mut c_void, out: *mut f32);
    fn ora_cfa_open(
        h: *mut c_void,
        w: *mut u32,
        height: *mut u32,
        pattern: *mut u8,
        err: *mut c_char,
    ) -> c_int;
    fn ora_cfa_copy(h: *mut c_void, out: *mut f32);
    fn ora_sensor_size() -> u32;
    fn ora_sensor(h: *mut c_void, s: *mut NativeSensor, err: *mut c_char) -> c_int;
    fn ora_thumbnail(h: *mut c_void, data: *mut *mut u8, size: *mut u32, err: *mut c_char)
    -> c_int;
}

/// The header facts in LibRaw's metadata, with the Panasonic sensor borders
/// and RAF crop read from the file beside it.
pub(super) fn header(
    m: &NativeMetadata,
    sensor_borders: Option<[u32; 4]>,
    recommended_crop: Option<[u32; 4]>,
) -> Header {
    let text = |bytes: &[c_char]| {
        let bytes: Vec<u8> = bytes.iter().map(|&c| c as u8).collect();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    let matrix = |m: &[f32; 9]| std::array::from_fn(|r| std::array::from_fn(|c| m[r * 3 + c]));
    Header {
        make: text(&m.make),
        model: text(&m.model),
        normalized: [text(&m.normalized_make), text(&m.normalized_model)],
        width: m.width,
        height: m.height,
        raw_width: m.raw_width,
        raw_height: m.raw_height,
        margins: [m.left_margin, m.top_margin],
        frame: [m.crop_left, m.crop_top, m.crop_width, m.crop_height],
        aspect: [m.aspect_left, m.aspect_top, m.aspect_width, m.aspect_height],
        sensor_borders,
        recommended_crop,
        flip: m.flip,
        xtrans: m.xtrans != 0,
        fuji_dynamic_range: m.fuji_dynamic_range,
        highlight_tone_priority: m.highlight_tone_priority,
        sraw: m.sraw != 0,
        fuji_exposure_shift: m.fuji_exposure_shift,
        iso: m.iso,
        shutter: m.shutter,
        aperture: m.aperture,
        focal: m.focal,
        focal_35mm: m.focal_35mm,
        wb: m.wb,
        daylight_wb: m.daylight_wb,
        presets: [
            m.preset_daylight,
            m.preset_shade,
            m.preset_d65,
            m.preset_fine_weather,
        ],
        matrix: matrix(&m.matrix),
        cam_xyz: matrix(&m.cam_xyz),
        lens: text(&m.lens),
        lens_id: m.lens_id,
        dng_version: m.dng_version,
    }
}
/// LibRaw's version string.
pub fn version() -> String {
    // SAFETY: `ora_version` returns LibRaw's static, NUL-terminated version string.
    unsafe { CStr::from_ptr(ora_version()).to_string_lossy().into_owned() }
}
/// Runs the calling thread's LibRaw decodes on two OpenMP threads from now
/// on; see `ora_background_thread`.
pub(super) fn background_thread() {
    // SAFETY: takes nothing and changes only the calling thread's own settings.
    unsafe { ora_background_thread() }
}

/// `sizeof(Metadata)` on the native side.
fn native_metadata_size() -> usize {
    // SAFETY: `ora_metadata_size` takes nothing and returns a constant.
    unsafe { ora_metadata_size() as usize }
}
/// `sizeof(Sensor)` on the native side.
fn native_sensor_size() -> usize {
    // SAFETY: `ora_sensor_size` takes nothing and returns a constant.
    unsafe { ora_sensor_size() as usize }
}
/// The text in a NUL-terminated buffer the native side filled.
fn text(buf: &[c_char]) -> String {
    debug_assert!(buf.contains(&0), "native text is NUL-terminated");
    // SAFETY: every buffer passed here was zero-initialised and is only written by
    // `message` or `snprintf` on the native side, which NUL-terminate within it.
    unsafe { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() }
}
/// `native/raw.cpp`'s `HighEfficiency`.
type HighEfficiency =
    extern "C" fn(*const u8, usize, *mut u16, u32, u32, u32, *mut c_char) -> c_int;
/// Decodes a Nikon High Efficiency codestream into LibRaw's raw buffer, for LibRaw's
/// unpack: `data` is `len` bytes, `out` `height` rows of `pitch` values, `width` of
/// them the image's. Returns nonzero with `err` filled when it fails.
extern "C" fn high_efficiency(
    data: *const u8,
    len: usize,
    out: *mut u16,
    width: u32,
    height: u32,
    pitch: u32,
    err: *mut c_char,
) -> c_int {
    let result = std::panic::catch_unwind(|| {
        ensure!(
            !data.is_null() && !out.is_null() && pitch >= width,
            "Invalid raw buffer"
        );
        // SAFETY: the native side passes its codestream buffer of `len` bytes and
        // LibRaw's raw buffer, which holds at least `height` rows of `pitch` values
        // (unpack allocates `raw_height + 8` rows of `raw_pitch`), both live and
        // otherwise untouched for the length of this call.
        let (data, out) = unsafe {
            (
                std::slice::from_raw_parts(data, len),
                std::slice::from_raw_parts_mut(out, pitch as usize * height as usize),
            )
        };
        super::nikon_he::decode(data, out, width as usize, height as usize, pitch as usize)
    });
    let message = match result {
        Ok(Ok(())) => return 0,
        Ok(Err(e)) => format!("Nikon High Efficiency NEF could not be decoded: {e:#}"),
        Err(_) => "Nikon High Efficiency NEF could not be decoded".to_owned(),
    };
    let bytes = &message.as_bytes()[..message.len().min(ERR - 1)];
    // SAFETY: `err` is the native side's error buffer of `ERR` bytes; at most
    // `ERR - 1` bytes and a NUL are written.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), err, bytes.len());
        *err.add(bytes.len()) = 0;
    }
    1
}
/// LibRaw's progress callback: `ctx` is the `&AtomicBool` `develop` passed.
extern "C" fn cancelled(ctx: *mut c_void) -> c_int {
    // SAFETY: `Handle::develop` passes a reference to a live `AtomicBool` as `ctx`
    // and LibRaw calls back only while that call runs.
    unsafe { (&*(ctx as *const AtomicBool)).load(Ordering::Relaxed) as c_int }
}
#[cfg(unix)]
fn path_string(p: &Path) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(CString::new(p.as_os_str().as_bytes())?)
}
/// UTF-8, which the native side widens for LibRaw's wide-character open.
#[cfg(windows)]
fn path_string(p: &Path) -> Result<CString> {
    let utf8 = p
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path is not valid Unicode: {}", p.display()))?;
    Ok(CString::new(utf8)?)
}

/// An open LibRaw file. Closed when dropped; used from one thread at a time.
pub(super) struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `ora_open` and is closed exactly once, here.
        unsafe { ora_close(self.0) }
    }
}
/// LibRaw's own development: demosaiced camera-space pixels.
pub(super) struct Developed {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub clipped: u32,
    pub pixels: Vec<[f32; 3]>,
}
impl Handle {
    /// Opens the file and reads its metadata; fails for files LibRaw cannot read
    /// and for sensors other than three-colour Bayer and X-Trans.
    pub(super) fn open(path: &Path) -> Result<(Self, NativeMetadata)> {
        ensure!(
            native_metadata_size() == std::mem::size_of::<NativeMetadata>(),
            "The native RAW bridge was built from a different Metadata layout"
        );
        let path = path_string(path)?;
        let mut err = [0 as c_char; ERR];
        // SAFETY: NativeMetadata is plain data (integers, floats and byte arrays),
        // for which all zeros is a valid value; the native side overwrites it.
        let mut m: NativeMetadata = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated, `m` and `err` are live, writable and
        // of the sizes the native side expects (`ERR` for `err`); `high_efficiency`
        // is a function, valid for the program's life.
        let handle = unsafe { ora_open(path.as_ptr(), &mut m, high_efficiency, err.as_mut_ptr()) };
        ensure!(!handle.is_null(), "{}", text(&err));
        Ok((Self(handle), m))
    }
    /// The embedded JPEG preview.
    pub(super) fn thumbnail(&mut self) -> Result<Vec<u8>> {
        let mut data = std::ptr::null_mut();
        let mut size = 0;
        let mut err = [0 as c_char; ERR];
        // SAFETY: the handle is open; `data`, `size` and `err` are live out-pointers.
        let rc = unsafe { ora_thumbnail(self.0, &mut data, &mut size, err.as_mut_ptr()) };
        ensure!(rc == 0, "{}", text(&err));
        ensure!(
            !data.is_null() && size > 0 && size < 100_000_000,
            "Invalid preview length"
        );
        // SAFETY: on success `data` points at `size` bytes of the handle's thumbnail
        // buffer, which stays valid until the next call on this handle; the bytes
        // are copied out before any other call. `&mut self` keeps the handle to us.
        Ok(unsafe { std::slice::from_raw_parts(data, size as usize).to_vec() })
    }
    /// The unpacked sensor data, or `None` when the file is not single-channel
    /// Bayer or X-Trans data and LibRaw's own development must be used.
    pub(super) fn cfa(&self) -> Result<Option<Cfa>> {
        let (mut width, mut height) = (0u32, 0u32);
        let mut pattern = [0u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN];
        let mut err = [0 as c_char; ERR];
        // SAFETY: the handle is open; `pattern` has the PATTERN × PATTERN bytes the
        // native side fills, and the other arguments are live out-pointers.
        let rc = unsafe {
            ora_cfa_open(
                self.0,
                &mut width,
                &mut height,
                pattern.as_mut_ptr(),
                err.as_mut_ptr(),
            )
        };
        if rc > 0 {
            return Ok(None);
        }
        ensure!(rc == 0, "{}", text(&err));
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 150_000_000,
            "Invalid RAW dimensions"
        );
        ensure!(pattern.iter().all(|c| *c < 3), "Unsupported colour filter");
        let mut data = vec![0f32; width as usize * height as usize];
        // SAFETY: `ora_cfa_open` succeeded on this handle and reported `width` and
        // `height`, so `ora_cfa_copy` writes exactly `width × height` floats, which
        // `data` holds.
        unsafe { ora_cfa_copy(self.0, data.as_mut_ptr()) };
        Ok(Some(Cfa {
            width,
            height,
            pattern,
            data,
        }))
    }
    /// Unpacks the sensor data as development does and describes it.
    pub(super) fn sensor(&self) -> Result<Sensor> {
        use sha2::Digest;
        ensure!(
            native_sensor_size() == std::mem::size_of::<NativeSensor>(),
            "The native RAW bridge was built from a different Sensor layout"
        );
        let mut err = [0 as c_char; ERR];
        // SAFETY: NativeSensor is integers, arrays and pointers, for which all
        // zeros (null pointers) is valid; the native side overwrites it.
        let mut s: NativeSensor = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is open; `s` and `err` are live and writable.
        let rc = unsafe { ora_sensor(self.0, &mut s, err.as_mut_ptr()) };
        ensure!(rc == 0, "{}", text(&err));
        let channels = s.channels as usize;
        let row = s.width as usize * channels;
        ensure!(
            !s.data.is_null()
                && !s.black.is_null()
                && (6..=4104).contains(&s.black_size)
                && s.width > 0
                && s.height > 0
                && (s.pitch as usize) >= row
                && s.area_width > 0
                && s.area_height > 0
                && u64::from(s.left) + u64::from(s.area_width) <= u64::from(s.width)
                && u64::from(s.top) + u64::from(s.area_height) <= u64::from(s.height)
                && u64::from(s.pitch) * u64::from(s.height) <= 1_000_000_000
                && s.pattern_rows as usize * s.pattern_cols as usize <= s.pattern.len(),
            "Invalid RAW dimensions"
        );
        // SAFETY: on success `black` points at `black_size` values of the
        // handle's levels, valid until the handle is used again, which `&self`
        // rules out until they are copied below.
        let black = unsafe { std::slice::from_raw_parts(s.black, s.black_size as usize) };
        let mut hash = sha2::Sha256::new();
        let width = s.area_width as usize * channels;
        let mut bytes = Vec::with_capacity(width * 2);
        for y in s.top as usize..(s.top + s.area_height) as usize {
            let start = y * s.pitch as usize + s.left as usize * channels;
            // SAFETY: `data` points at the handle's raw buffer of `height` rows
            // of `pitch` values; the area was checked to lie inside it and the
            // decoder wrote it (only borders may be left unwritten). It stays
            // valid until the handle is used again, which `&self` rules out.
            let row = unsafe { std::slice::from_raw_parts(s.data.add(start), width) };
            bytes.clear();
            bytes.extend(row.iter().flat_map(|v| v.to_le_bytes()));
            hash.update(&bytes);
        }
        let cols = s.pattern_cols as usize;
        Ok(Sensor {
            channels: s.channels,
            origin: [s.left, s.top],
            area: [s.area_width, s.area_height],
            data_sha256: hash.finalize().into(),
            black: std::array::from_fn(|c| s.base_black + black[c]),
            black_pattern: (black[4], black[5], black[6..].to_vec()),
            white: s.white,
            linear_white: s.linear_white,
            pattern: (0..s.pattern_rows as usize)
                .map(|r| s.pattern[r * cols..(r + 1) * cols].to_vec())
                .collect(),
        })
    }
    /// LibRaw's development (half size when `fast`), stopping early when `cancel`
    /// is set.
    pub(super) fn develop(&self, fast: bool, cancel: &AtomicBool) -> Result<Developed> {
        let (mut width, mut height, mut gain, mut scale, mut clipped) = (0, 0, 0., 0., 0);
        let mut err = [0 as c_char; ERR];
        // SAFETY: the handle is open; `cancel` outlives this call, which is the only
        // time LibRaw invokes `cancelled` with it; the rest are live out-pointers.
        let rc = unsafe {
            ora_develop(
                self.0,
                fast as c_int,
                cancelled,
                cancel as *const AtomicBool as *mut c_void,
                &mut width,
                &mut height,
                &mut gain,
                &mut scale,
                &mut clipped,
                err.as_mut_ptr(),
            )
        };
        ensure!(rc == 0, "{}", text(&err));
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 150_000_000,
            "Invalid RAW dimensions"
        );
        let mut pixels = vec![[0f32; 3]; width as usize * height as usize];
        // SAFETY: `ora_develop` succeeded on this handle and reported `width` and
        // `height`, so `ora_copy` writes exactly `width × height × 3` floats, which
        // `pixels` holds contiguously.
        unsafe { ora_copy(self.0, pixels.as_mut_ptr().cast()) };
        ensure!(
            gain.is_finite() && pixels.iter().flatten().all(|v| v.is_finite()),
            "Non-finite RAW pixels"
        );
        Ok(Developed {
            width,
            height,
            scale,
            clipped,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    unsafe extern "C" {
        fn ora_scale_probe(wb: f32, error: *mut f32) -> i32;
        fn ora_masked_black(border: *const u16, n: usize, black: u32, maximum: u32) -> i32;
    }
    fn masked_black(border: &[u16], black: u32) -> Option<u32> {
        // SAFETY: `border` is a live slice of `n` values, only read.
        let b = unsafe { ora_masked_black(border.as_ptr(), border.len(), black, 16383) };
        u32::try_from(b).ok()
    }
    /// Optical-black border values around `level`, with a little read noise.
    fn border(level: u16) -> Vec<u16> {
        (0..20_000u32)
            .map(|i| level - 6 + (i * 7919 % 13) as u16)
            .collect()
    }
    #[test]
    fn black_level_far_below_the_masked_border_is_corrected() {
        // LibRaw reads the EOS R6 Mark III's black as 71 on average over its
        // channels; its masked border, and Adobe's DNG, say 512.
        assert_eq!(masked_black(&border(512), 71), Some(512));
        // A black LibRaw reads right, or slightly above the border, stays.
        assert_eq!(masked_black(&border(512), 512), None);
        assert_eq!(masked_black(&border(505), 512), None);
        // So does one well below a border that sits above the true black
        // (Pentax K-70: 64 against a border of 130, and Adobe says 64).
        assert_eq!(masked_black(&border(130), 64), None);
        // A margin that holds image rather than optical black is no evidence.
        let image: Vec<u16> = (0..20_000u32)
            .map(|i| 600 + (i * 7919 % 8000) as u16)
            .collect();
        assert_eq!(masked_black(&image, 0), None);
        // Nor is a margin too small to measure.
        assert_eq!(masked_black(&border(512)[..100], 0), None);
    }
    /// The hand-written mirror of `struct Metadata` and the C++ original agree on
    /// size, so a field added on one side alone fails here rather than misreading.
    #[test]
    fn native_metadata_layout_matches() {
        assert_eq!(
            std::mem::size_of::<super::NativeMetadata>(),
            super::native_metadata_size()
        );
    }
    #[test]
    fn native_sensor_layout_matches() {
        assert_eq!(
            std::mem::size_of::<super::NativeSensor>(),
            super::native_sensor_size()
        );
    }
    #[test]
    fn integer_boundary_retains_near_saturation_ramps() {
        for wb in [1., 2.5, 8., 16.] {
            let mut error = 0.;
            // SAFETY: `error` is a live out-pointer for the probe's result.
            let clipped = unsafe { ora_scale_probe(wb, &mut error) };
            assert_eq!(clipped, 0);
            assert!(error < wb / 59000., "WB {wb}: error {error}");
        }
    }
}
