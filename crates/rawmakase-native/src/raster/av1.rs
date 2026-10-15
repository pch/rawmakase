//! One AV1 still decoded through rav1d (dav1d in Rust, built without its
//! assembly), which keeps dav1d's C interface; this is the safe side of it.
//!
//! A panic inside rav1d cannot unwind out of its `extern "C"` functions, so it
//! aborts the whole app. rav1d 1.1.0 panics on a corrupt frame when it decodes
//! with a single frame context (one thread, or a frame delay of 1), so the
//! decoder is opened with at least two threads and its default frame delay,
//! which decodes with frame threading; corrupt data then comes back as an
//! error. It costs a small still about what one frame context did (some
//! 0.3 ms, mostly starting and joining the threads).
//!
//! With frame threading the first `dav1d_get_picture` after the data is sent
//! answers "again" and the next drains: it waits for the frames in flight to
//! finish, then gives the picture, an error, or "again" when the data is
//! incomplete (a cut-off file). Asking is bounded rather than waited on. The
//! context is closed before [`decode`] returns, which joins its worker
//! threads; they are idle by then.
use anyhow::{Result, bail};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dSequenceHeader,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use std::ptr::NonNull;

/// The largest frame the decoder allocates, as many pixels as `prepare` lets
/// a file have: a bitstream that claims more than its container is refused
/// before its planes are allocated.
const MAX_PIXELS: u32 = 150_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Layout {
    I400,
    I420,
    I422,
    I444,
}

/// A decoded picture; its planes are rav1d's, released on drop.
pub(super) struct Picture(Dav1dPicture);

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the picture was filled by a successful dav1d_get_picture and
        // is owned by this value alone, so this is its one unreference; it
        // releases rav1d's reference-counted planes and headers, which stay
        // valid after the context is closed.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.0))) }
    }
}

impl Picture {
    pub(super) fn width(&self) -> u32 {
        self.0.p.w as u32
    }
    pub(super) fn height(&self) -> u32 {
        self.0.p.h as u32
    }
    pub(super) fn depth(&self) -> u8 {
        self.0.p.bpc as u8
    }
    pub(super) fn layout(&self) -> Layout {
        match self.0.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => Layout::I400,
            DAV1D_PIXEL_LAYOUT_I420 => Layout::I420,
            DAV1D_PIXEL_LAYOUT_I422 => Layout::I422,
            DAV1D_PIXEL_LAYOUT_I444 => Layout::I444,
            // rav1d has no other layout.
            _ => Layout::I444,
        }
    }
    /// Plane `i` (Y, U, V) as bytes and its stride in bytes; empty for the
    /// chroma planes of a monochrome picture.
    pub(super) fn plane(&self, i: usize) -> (&[u8], usize) {
        if i > 2 || (i > 0 && self.layout() == Layout::I400) {
            return (&[], 0);
        }
        let Ok(stride) = usize::try_from(self.0.stride[i.min(1)]) else {
            // rav1d's own allocator never writes upwards.
            return (&[], 0);
        };
        let rows = match (i, self.layout()) {
            (1 | 2, Layout::I420) => (self.height() as usize).div_ceil(2),
            _ => self.height() as usize,
        };
        let Some(data) = self.0.data[i] else {
            return (&[], 0);
        };
        // SAFETY: rav1d's default allocator gives each plane `stride` bytes
        // for each of the height rounded up to 128 rows (half of them for
        // 4:2:0 chroma), at least the `rows` used here, all initialised; the
        // picture holds a reference to them until it is dropped, and `&self`
        // keeps it alive for the slice's lifetime.
        let bytes =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, stride * rows) };
        (bytes, stride)
    }
    fn sequence(&self) -> Option<&Dav1dSequenceHeader> {
        // SAFETY: a picture from dav1d_get_picture points at its sequence
        // header and holds a reference to it (`seq_hdr_ref`) until dropped.
        self.0.seq_hdr.map(|h| unsafe { h.as_ref() })
    }
    /// CICP matrix coefficients from the sequence header; unspecified (2)
    /// without one.
    pub(super) fn matrix(&self) -> u8 {
        self.sequence().map_or(2, |s| s.mtrx as u8)
    }
    pub(super) fn full_range(&self) -> bool {
        self.sequence().is_some_and(|s| s.color_range != 0)
    }
}

/// The open decoder; closed on drop, on every path out of [`decode`].
struct Context(Option<Dav1dContext>);

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context came from a successful dav1d_open and is
        // closed once, here; dav1d_close takes it out of the option.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.0))) }
    }
}

/// The first picture of the AV1 bitstream `obu`.
pub(super) fn decode(obu: &[u8]) -> Result<Picture> {
    if obu.is_empty() {
        bail!("The AV1 data is empty");
    }
    let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
    // SAFETY: dav1d_default_settings writes a whole Dav1dSettings to the
    // pointer it is given, which points at space for one; after it the value
    // is initialised.
    let mut settings = unsafe {
        dav1d_default_settings(NonNull::new(settings.as_mut_ptr()).expect("a stack pointer"));
        settings.assume_init()
    };
    // At least two threads and rav1d's own frame delay (left at 0), so it
    // decodes with frame threading: with a single frame context a corrupt
    // frame header makes rav1d 1.1.0 panic on its error path.
    settings.n_threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get().min(8))
        .max(2) as _;
    settings.frame_size_limit = MAX_PIXELS;
    let mut context = Context(None);
    // SAFETY: both pointers are to live, initialised values; dav1d_open writes
    // the new context into `context.0` and reads the settings.
    let opened = unsafe {
        dav1d_open(
            Some(NonNull::from(&mut context.0)),
            Some(NonNull::from(&mut settings)),
        )
    };
    if opened.0 != 0 || context.0.is_none() {
        bail!("Could not start the AV1 decoder");
    }

    let mut data = Dav1dData::default();
    // SAFETY: `data` is a live Dav1dData; dav1d_data_create fills it with a
    // new reference-counted buffer of `obu.len()` bytes and returns its start,
    // or null when it could not.
    let buffer = unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), obu.len()) };
    if buffer.is_null() {
        bail!("Out of memory for the AV1 data");
    }
    // SAFETY: `buffer` points at `obu.len()` writable bytes rav1d just
    // allocated, which cannot overlap `obu`.
    unsafe { std::ptr::copy_nonoverlapping(obu.as_ptr(), buffer, obu.len()) };
    // SAFETY: the context is open and `data` holds the buffer created for it.
    // On success rav1d takes the data and leaves `data` empty; on failure the
    // data is still ours, and unreferenced just below.
    let sent = unsafe { dav1d_send_data(context.0, Some(NonNull::from(&mut data))) };
    if data.sz > 0 {
        // SAFETY: `data` still holds its buffer, which this releases once.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
    }
    if sent.0 != 0 && sent.0 != -libc::EAGAIN {
        bail!("The AV1 data can't be decoded");
    }

    // All the data is sent: rav1d either has the picture or needs data it will
    // never get. Bounded, never a hang.
    for _ in 0..1000 {
        let mut picture = Dav1dPicture::default();
        // SAFETY: the context is open and `picture` is a live, empty picture
        // for dav1d_get_picture to fill.
        let got = unsafe { dav1d_get_picture(context.0, Some(NonNull::from(&mut picture))) };
        match got.0 {
            0 => return Ok(Picture(picture)),
            e if e == -libc::EAGAIN => std::thread::yield_now(),
            _ => bail!("The AV1 data can't be decoded"),
        }
    }
    bail!("The AV1 data is incomplete")
}

#[cfg(test)]
mod tests {
    /// The AV1 data of the primary item of the fixture `name`.
    fn obu(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats")
            .join(name);
        let mut file = std::fs::File::open(path).unwrap();
        let heif = rawmakase_interop::isobmff::Heif::read(&mut file).unwrap();
        heif.data(&mut file, heif.primary).unwrap()
    }

    #[test]
    fn an_av1_still_decodes_at_its_size_and_depth() {
        let picture = super::decode(&obu("ten-bit.avif")).unwrap();
        assert_eq!((picture.width(), picture.height()), (16, 12));
        assert_eq!(picture.depth(), 10);
        let (y, stride) = picture.plane(0);
        assert!(stride >= 2 * 16 && y.len() >= stride * 12);
    }

    #[test]
    fn cut_off_av1_data_is_an_error_not_a_hang() {
        let obu = obu("ten-bit.avif");
        for keep in [obu.len() - 1, obu.len() / 2, 3, 0] {
            assert!(super::decode(&obu[..keep]).is_err(), "{keep}");
        }
        assert!(super::decode(&[0xff; 64]).is_err());
    }
}
