//! The C ABI of `native/raw.cpp`: its declarations, the metadata struct both sides
//! share by layout, and one safe wrapper per entry point that states what the call
//! relies on. Nothing else in the crate calls LibRaw or Little CMS directly.
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
//! - `ora_thumbnail` points into the handle's own buffer, valid until the next
//!   call on that handle.
use anyhow::{Result, bail, ensure};
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
    pub(super) sony_daylight_wb: [f32; 3],
    pub(super) left_margin: u32,
    pub(super) top_margin: u32,
    pub(super) aspect_left: u32,
    pub(super) aspect_top: u32,
    pub(super) aspect_width: u32,
    pub(super) aspect_height: u32,
}
const _: () = assert!(std::mem::size_of::<NativeMetadata>() == 460);

/// Size of the error buffers `native/raw.cpp`'s `message` writes into.
const ERR: usize = 512;

unsafe extern "C" {
    fn ora_version() -> *const c_char;
    fn ora_background_thread();
    fn ora_metadata_size() -> u32;
    fn ora_open(path: *const c_char, m: *mut NativeMetadata, err: *mut c_char) -> *mut c_void;
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
    fn ora_thumbnail(h: *mut c_void, data: *mut *mut u8, size: *mut u32, err: *mut c_char)
    -> c_int;
    fn ora_srgb_profile(data: *mut u8, size: u32) -> u32;
    fn ora_display(path: *const c_char, data: *mut u8, count: u32) -> c_int;
}

/// LibRaw's version string.
pub fn version() -> String {
    // SAFETY: `ora_version` returns LibRaw's static, NUL-terminated version string.
    unsafe { CStr::from_ptr(ora_version()).to_string_lossy().into_owned() }
}
/// Runs the calling thread at low priority from now on, with LibRaw decodes on
/// two OpenMP threads; see `ora_background_thread`.
pub(super) fn background_thread() {
    // SAFETY: takes nothing and changes only the calling thread's own settings.
    unsafe { ora_background_thread() }
}

/// `sizeof(Metadata)` on the native side.
fn native_metadata_size() -> usize {
    // SAFETY: `ora_metadata_size` takes nothing and returns a constant.
    unsafe { ora_metadata_size() as usize }
}
/// The text in a NUL-terminated buffer the native side filled.
fn text(buf: &[c_char]) -> String {
    debug_assert!(buf.contains(&0), "native text is NUL-terminated");
    // SAFETY: every buffer passed here was zero-initialised and is only written by
    // `message` or `snprintf` on the native side, which NUL-terminate within it.
    unsafe { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() }
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
/// Unpacked sensor data for RAWmakase's own demosaic.
pub(super) struct Cfa {
    pub width: u32,
    pub height: u32,
    /// The colour pattern as a `PATTERN × PATTERN` tile: 0 red, 1 green, 2 blue.
    pub pattern: [u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN],
    /// `width × height` values, (raw − black) / (white − black), not white balanced.
    pub data: Vec<f32>,
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
        // of the sizes the native side expects (`ERR` for `err`).
        let handle = unsafe { ora_open(path.as_ptr(), &mut m, err.as_mut_ptr()) };
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

/// Little CMS's sRGB profile, serialized.
pub fn srgb_profile() -> Result<Vec<u8>> {
    // SAFETY: with a null buffer and zero capacity the native side only reports
    // the size.
    let size = unsafe { ora_srgb_profile(std::ptr::null_mut(), 0) };
    ensure!(size > 0, "Cannot create sRGB profile");
    let mut data = vec![0; size as usize];
    // SAFETY: `data` has the `size` bytes the native side was told it may write.
    let written = unsafe { ora_srgb_profile(data.as_mut_ptr(), size) };
    ensure!(written == size, "Cannot serialize sRGB profile");
    Ok(data)
}
/// Converts 8-bit sRGB pixels in place to the monitor profile at `path`.
pub fn display_transform(path: &Path, data: &mut [u8]) -> Result<()> {
    ensure!(data.len().is_multiple_of(3), "Invalid RGB buffer");
    let p = path_string(path)?;
    let count: u32 = (data.len() / 3).try_into()?;
    // SAFETY: `p` is NUL-terminated and `data` holds `count` RGB triples, which the
    // native side transforms in place.
    let rc = unsafe { ora_display(p.as_ptr(), data.as_mut_ptr(), count) };
    if rc != 0 {
        bail!("Cannot use monitor ICC profile {}", path.display());
    }
    Ok(())
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
