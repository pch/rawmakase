//! A HEIC's pixels decoded by the system's libheif, as on Linux, opened when
//! first needed: RAWmakase links and bundles no HEVC decoder, so each
//! distribution makes its own patent decision, and a system without libheif,
//! or with one that has no HEVC decoder, is told what to install.
//!
//! libheif reads the bytes RAWmakase read the header from, through the same
//! open file, so the picture decoded is the one the header checks passed. It
//! is asked to leave the picture uncropped and unturned (RAWmakase applies the
//! `clap`, `irot` and `imir`), and to convert it to RGB of the header's depth
//! itself, by the image's own matrix and range; the pixels are in the colour
//! space the header's profile describes. Alpha is dropped. The library is
//! opened once and kept for the life of the process.
use super::super::avif;
use super::{HeicFile, Pixels};
use anyhow::{Result, anyhow, bail, ensure};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

/// The library's file names, the versioned one first: the unversioned one is
/// often only in a `-dev` package.
const LIBRARY: [&CStr; 2] = [c"libheif.so.1", c"libheif.so"];

/// What to install, said when libheif can't be opened.
const INSTALL: &str = "Showing HEIF images needs the system's libheif with its HEVC decoder \
     (libde265). Install it from your distribution: on Arch, `libheif`; on Debian or Ubuntu, \
     `libheif1` and `libheif-plugin-libde265`; on Fedora, `libheif-freeworld` from RPM Fusion";

/// What to install, said when libheif opens but has no HEVC decoder.
const NO_HEVC: &str = "The system's libheif has no HEVC decoder, which HEIC photos need. \
     Install its libde265 plugin: on Debian or Ubuntu, `libheif-plugin-libde265`; on Fedora, \
     `libheif-freeworld` from RPM Fusion";

/// `heif_compression_HEVC`.
const HEVC: c_int = 1;
/// `heif_colorspace_RGB`.
const RGB: c_int = 1;
/// `heif_chroma_interleaved_RGB` and `heif_chroma_interleaved_RRGGBB_LE`.
const INTERLEAVED_RGB: c_int = 10;
const INTERLEAVED_RRGGBB_LE: c_int = 14;
/// `heif_channel_interleaved`.
const INTERLEAVED: c_int = 10;
/// The offset of `heif_decoding_options.ignore_transformations`, after its
/// one-byte `version`: the first of its version 1 fields, which later
/// versions only add to.
const IGNORE_TRANSFORMATIONS: usize = 1;

/// `heif_error`, returned by value.
#[repr(C)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    /// Owned by libheif, valid until the next call on the object it is about.
    message: *const c_char,
}

type Context = c_void;
type Handle = c_void;
type Image = c_void;
type Options = c_void;

/// The functions RAWmakase calls, from the library opened.
struct Library {
    context_alloc: unsafe extern "C" fn() -> *mut Context,
    context_free: unsafe extern "C" fn(*mut Context),
    read_from_memory:
        unsafe extern "C" fn(*mut Context, *const c_void, usize, *const c_void) -> HeifError,
    primary_handle: unsafe extern "C" fn(*mut Context, *mut *mut Handle) -> HeifError,
    /// Declared as taking a mutable pointer, as `Owned` releases one; libheif
    /// takes a constant one, the same in the C ABI.
    handle_release: unsafe extern "C" fn(*mut Handle),
    options_alloc: unsafe extern "C" fn() -> *mut Options,
    options_free: unsafe extern "C" fn(*mut Options),
    decode: unsafe extern "C" fn(
        *const Handle,
        *mut *mut Image,
        c_int,
        c_int,
        *const Options,
    ) -> HeifError,
    image_release: unsafe extern "C" fn(*mut Image),
    width: unsafe extern "C" fn(*const Image, c_int) -> c_int,
    height: unsafe extern "C" fn(*const Image, c_int) -> c_int,
    bits: unsafe extern "C" fn(*const Image, c_int) -> c_int,
    plane: unsafe extern "C" fn(*const Image, c_int, *mut c_int) -> *const u8,
}

// SAFETY: plain function pointers into a library never closed; libheif's
// functions may be called from any thread, each context used by one.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

/// The library, opened and initialized once, or why it can't be used.
fn library() -> Result<&'static Library, &'static str> {
    static LIBRARY_OPENED: OnceLock<Result<Library, &'static str>> = OnceLock::new();
    LIBRARY_OPENED.get_or_init(open).as_ref().map_err(|e| *e)
}

/// Whether libheif and its HEVC decoder are there, or what to install.
pub(super) fn available() -> std::result::Result<(), String> {
    library().map(|_| ()).map_err(String::from)
}

/// A symbol's address as the function pointer `F`.
///
/// # Safety
/// `F` must be the function pointer type of the symbol's C signature.
unsafe fn cast<F: Copy>(address: *mut c_void) -> F {
    assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    // SAFETY: a function pointer the size of a data pointer, as on every
    // platform libheif runs on; its type is the caller's to get right.
    unsafe { std::mem::transmute_copy(&address) }
}

fn open() -> Result<Library, &'static str> {
    // SAFETY: each name is a NUL-terminated string; the library is a system
    // one, loaded locally and never closed, so the pointers taken from it
    // stay valid.
    let lib = LIBRARY
        .iter()
        .map(|name| unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) })
        .find(|lib| !lib.is_null())
        .ok_or(INSTALL)?;
    let symbol = |name: &CStr| {
        // SAFETY: `lib` is an open library and `name` NUL-terminated.
        let address = unsafe { libc::dlsym(lib, name.as_ptr()) };
        (!address.is_null()).then_some(address)
    };
    macro_rules! function {
        ($name:literal) => {
            // SAFETY: the symbol is libheif's function of that name, whose C
            // signature the field's type declares.
            unsafe { cast(symbol($name).ok_or(INSTALL)?) }
        };
    }
    // libheif 1.13 and later load their decoder plugins here; earlier ones
    // have no such function and need none.
    if let Some(init) = symbol(c"heif_init") {
        // SAFETY: heif_init takes optional parameters, none here; it is
        // never undone, as the library stays open.
        let init: unsafe extern "C" fn(*const c_void) -> HeifError = unsafe { cast(init) };
        if unsafe { init(std::ptr::null()) }.code != 0 {
            return Err(INSTALL);
        }
    }
    let have_decoder: unsafe extern "C" fn(c_int) -> c_int =
        function!(c"heif_have_decoder_for_format");
    // SAFETY: a query of the library's own decoders.
    if unsafe { have_decoder(HEVC) } == 0 {
        return Err(NO_HEVC);
    }
    Ok(Library {
        context_alloc: function!(c"heif_context_alloc"),
        context_free: function!(c"heif_context_free"),
        read_from_memory: function!(c"heif_context_read_from_memory_without_copy"),
        primary_handle: function!(c"heif_context_get_primary_image_handle"),
        handle_release: function!(c"heif_image_handle_release"),
        options_alloc: function!(c"heif_decoding_options_alloc"),
        options_free: function!(c"heif_decoding_options_free"),
        decode: function!(c"heif_decode_image"),
        image_release: function!(c"heif_image_release"),
        width: function!(c"heif_image_get_width"),
        height: function!(c"heif_image_get_height"),
        bits: function!(c"heif_image_get_bits_per_pixel_range"),
        plane: function!(c"heif_image_get_plane_readonly"),
    })
}

/// A libheif object, released by `release` when dropped.
struct Owned<T> {
    pointer: *mut T,
    release: unsafe extern "C" fn(*mut T),
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            // SAFETY: the pointer is the object libheif allocated, released
            // once, by the function that releases that kind.
            unsafe { (self.release)(self.pointer) }
        }
    }
}

/// `error` as a message: none for success, else libheif's words.
fn check(error: HeifError) -> Result<()> {
    if error.code == 0 {
        return Ok(());
    }
    let why = if error.message.is_null() {
        "unknown error".into()
    } else {
        // SAFETY: libheif's message is a NUL-terminated string, read before
        // any other call on the object it is about.
        unsafe { CStr::from_ptr(error.message) }.to_string_lossy()
    };
    bail!("libheif could not decode the HEIF image: {why}")
}

pub(super) fn pixels(mut file: HeicFile) -> Result<Pixels> {
    let lib = library().map_err(|e| anyhow!(e))?;
    // The file the header was read from, whole: the item limit already held
    // each picture to 256 MiB, and the header to what it says.
    let mut bytes = Vec::new();
    file.file.seek(SeekFrom::Start(0))?;
    file.file.read_to_end(&mut bytes)?;
    // Declared after `bytes`, so released before them: libheif reads them
    // without a copy.
    // SAFETY: allocations libheif makes, each released once by `Owned`.
    let context = Owned {
        pointer: unsafe { (lib.context_alloc)() },
        release: lib.context_free,
    };
    ensure!(!context.pointer.is_null(), "libheif could not start");
    // SAFETY: `bytes` outlive the context, which only reads them.
    check(unsafe {
        (lib.read_from_memory)(
            context.pointer,
            bytes.as_ptr().cast(),
            bytes.len(),
            std::ptr::null(),
        )
    })?;
    let mut handle = std::ptr::null_mut();
    // SAFETY: a live context; `handle` is written on success.
    check(unsafe { (lib.primary_handle)(context.pointer, &mut handle) })?;
    let handle = Owned {
        pointer: handle,
        release: lib.handle_release,
    };
    let options = Owned {
        // SAFETY: an allocation libheif makes, of its current version.
        pointer: unsafe { (lib.options_alloc)() },
        release: lib.options_free,
    };
    ensure!(!options.pointer.is_null(), "libheif could not start");
    // SAFETY: the options are at least version 1, whose second byte is
    // `ignore_transformations`.
    unsafe { *options.pointer.cast::<u8>().add(IGNORE_TRANSFORMATIONS) = 1 };
    let chroma = if file.deep {
        INTERLEAVED_RRGGBB_LE
    } else {
        INTERLEAVED_RGB
    };
    let mut image = std::ptr::null_mut();
    // SAFETY: a live handle and options; `image` is written on success.
    check(unsafe { (lib.decode)(handle.pointer, &mut image, RGB, chroma, options.pointer) })?;
    let image = Owned {
        pointer: image,
        release: lib.image_release,
    };
    // SAFETY: a decoded image of an interleaved channel.
    let (width, height) = unsafe {
        (
            (lib.width)(image.pointer, INTERLEAVED),
            (lib.height)(image.pointer, INTERLEAVED),
        )
    };
    let (width, height) = (u32::try_from(width)?, u32::try_from(height)?);
    ensure!(
        file.expects(width, height),
        "The HEIF image is not the size it says"
    );
    let mut stride = 0;
    // SAFETY: as above; `stride` is written with the plane.
    let plane = unsafe { (lib.plane)(image.pointer, INTERLEAVED, &mut stride) };
    let pixel = if file.deep { 6 } else { 3 };
    let row = width as usize * pixel;
    let stride = usize::try_from(stride)?;
    ensure!(
        !plane.is_null() && stride >= row,
        "libheif could not decode the HEIF image"
    );
    // SAFETY: libheif's plane holds `height` rows of `stride` bytes, alive
    // as long as `image`.
    let plane = unsafe { std::slice::from_raw_parts(plane, stride * (height as usize - 1) + row) };
    let rows = (0..height as usize).map(|y| &plane[y * stride..y * stride + row]);
    let rgb = if file.deep {
        // SAFETY: as above.
        let bits = unsafe { (lib.bits)(image.pointer, INTERLEAVED) };
        let bits = u8::try_from(bits)
            .ok()
            .filter(|b| (8..=16).contains(b))
            .ok_or_else(|| anyhow!("libheif decoded the HEIF image at {bits} bits"))?;
        let samples: Vec<u16> = rows
            .flat_map(|r| r.as_chunks::<2>().0.iter().map(|s| u16::from_le_bytes(*s)))
            .collect();
        avif::widen(&samples, bits)
    } else {
        rows.flatten().copied().collect()
    };
    Ok(Pixels { width, height, rgb })
}
