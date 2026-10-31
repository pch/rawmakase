//! RAWmakase's own RAW decoders, in pure Rust: containers, maker notes,
//! sensor codecs, camera facts and embedded previews.
//!
//! Three operations, cheapest first, each on the file's bytes in memory:
//! [`open`] reads the header facts ([`RawInfo`]) without decoding the
//! sensor data, [`preview`] finds the embedded JPEG, and [`decode`] unpacks
//! the sensor data ([`Sensor`]), checking `cancel` as it goes. One selector
//! ([`Variant`]) decides which reader and codec handle a file; a file of a
//! known container in a variant not decoded here is [`Error::Unsupported`].
//! [`capture_dates`] and [`exif_tiffs`] read, from the file itself, the few
//! parts that date a photo and hold its camera's EXIF in the containers
//! whose EXIF is not a TIFF at the file's start; [`embedded_xmp`] the XMP a
//! CR3 keeps in a box of its own.
#![forbid(unsafe_code)]
mod captured;
mod codec;
mod color;
mod container;
mod error;
mod exif_tiffs;
mod facts;
mod geometry;
mod info;
mod io;
mod levels;
mod names;
mod preview;
mod sensor;
mod variant;
mod vendor;

pub use captured::{CaptureDates, Stamp};
pub use error::{Error, Result};
pub use exif_tiffs::ExifTiffs;
pub use info::{Cfa, MatrixSource, Presets, PreviewSpace, RawInfo, SuperCcd, UNSET};
pub use preview::{NoPreview, is_jpeg_xl};
pub use sensor::Sensor;
use std::sync::atomic::AtomicBool;
pub use variant::Variant;

/// Nikon High Efficiency (JPEG XS) codestream decoding, for the LibRaw
/// bridge's unpack until RAWmakase's own decoders replace it.
pub use codec::nikon_he::decode as decode_nikon_high_efficiency;
/// Synthetic High Efficiency codestreams, for other crates' tests.
#[cfg(feature = "test-support")]
pub use codec::nikon_he::tests as nikon_high_efficiency_test_support;

/// The decoders' version, for the About window and fingerprints.
pub const VERSION: &str = concat!("RAWmakase ", env!("CARGO_PKG_VERSION"));

/// The header facts of the RAW file in `bytes`.
pub fn open(bytes: &[u8]) -> Result<RawInfo> {
    variant::open(bytes)
}

/// A camera's make and model as its file writes them, before RAWmakase's
/// naming: "Canon" and "Canon EOS R6m2" where [`RawInfo`] says "Canon" and
/// "EOS R6 Mark II".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileNames {
    pub make: String,
    pub model: String,
}

/// The make and model the RAW file in `bytes` writes, from its header.
pub fn file_camera_names(bytes: &[u8]) -> Result<FileNames> {
    let info = open(bytes)?;
    Ok(FileNames {
        make: info.file_make,
        model: info.file_model,
    })
}

/// The maker a file's make names, as RAWmakase spells it ("SONY" is
/// "Sony", "NIKON CORPORATION" "Nikon"); another maker's name, trimmed.
pub fn maker_name(file_make: &str) -> String {
    names::make(file_make)
}

/// When the CR3, CRW, MRW or X3F `file` was taken, from the parts of it
/// that say so, nothing else read; none for another file or one that can't
/// be read. Holds for files whose sensor data is not decoded here (X3F).
pub fn capture_dates(file: &mut (impl std::io::Read + std::io::Seek)) -> CaptureDates {
    captured::read(file)
}

/// Where the CR3, MRW or X3F `file` keeps its camera's EXIF, as TIFFs of
/// their own, nothing else read; none for another file (a CRW has no
/// TIFF) or one that can't be read. Holds for files whose sensor data is
/// not decoded here (X3F).
pub fn exif_tiffs(file: &mut (impl std::io::Read + std::io::Seek)) -> ExifTiffs {
    let Some(mut f) = io::Pieces::new(file) else {
        return ExifTiffs::default();
    };
    let head = f.read_upto(0, 16).unwrap_or_default();
    exif_tiffs::read(&mut f, &head)
}

/// The XMP packet a camera embeds in the CR3 `file` (an in-camera rating,
/// title or description), the box holding it read alone; none for another
/// file, one without, or one that can't be read.
pub fn embedded_xmp(file: &mut (impl std::io::Read + std::io::Seek)) -> Option<Vec<u8>> {
    let mut f = io::Pieces::new(file)?;
    let head = f.read_upto(0, 16)?;
    container::cr3::is_cr3(&head)
        .then(|| container::cr3::xmp(&mut f))
        .flatten()
}

/// What RAWmakase reads of an X3F, whose sensor data it doesn't decode: all
/// the photo info it has of one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct X3fMetadata {
    /// The preview's EXIF, a TIFF of its own, as [`exif_tiffs`] finds it;
    /// none on the SD9 and SD10.
    pub exif: Option<Vec<u8>>,
    pub properties: X3fProperties,
}

/// What an X3F's property list says of its camera and exposure, which the
/// SD9 and SD10 write instead of a preview's EXIF. Values not given are
/// empty or `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct X3fProperties {
    /// CAMMANUF and CAMMODEL, as the file writes them.
    pub make: String,
    pub model: String,
    /// In seconds.
    pub exposure: Option<f64>,
    pub aperture: Option<f64>,
    pub iso: Option<f64>,
    /// In millimetres.
    pub focal: Option<f64>,
}

/// The preview's EXIF and the property list of the X3F `file`, its
/// directory and these sections read alone; `None` for a file that is no
/// X3F, or when a section holding them can't be read (a file still being
/// copied), so that what can't be read is not taken for absent.
pub fn x3f_metadata(file: &mut (impl std::io::Read + std::io::Seek)) -> Option<X3fMetadata> {
    let mut f = io::Pieces::new(file)?;
    let head = f.read_upto(0, 16)?;
    if !container::x3f::is_x3f(&head) {
        return None;
    }
    container::x3f::metadata(&mut f)
}

/// The embedded preview of the file `info` was read from, JPEG or JPEG XL
/// ([`is_jpeg_xl`] tells which; a JPEG is copied when its start of image
/// marker needs mending).
pub fn preview<'a>(
    info: &RawInfo,
    bytes: &'a [u8],
) -> std::result::Result<std::borrow::Cow<'a, [u8]>, NoPreview> {
    embedded(bytes, preview::find(info, bytes)?)
}

/// The embedded preview a thumbnail `edge` pixels on its long side is best
/// made from, as [`preview`] gives it: the smallest JPEG that reaches `edge`
/// and has the largest preview's shape, else the largest.
pub fn preview_reaching<'a>(
    info: &RawInfo,
    bytes: &'a [u8],
    edge: u32,
) -> std::result::Result<std::borrow::Cow<'a, [u8]>, NoPreview> {
    embedded(bytes, preview::find_reaching(info, bytes, edge)?)
}

fn embedded(
    bytes: &[u8],
    (at, len): (u64, u64),
) -> std::result::Result<std::borrow::Cow<'_, [u8]>, NoPreview> {
    let range = usize::try_from(at)
        .ok()
        .zip(usize::try_from(at.saturating_add(len)).ok());
    let jpeg = range
        .and_then(|(start, end)| bytes.get(start..end))
        .ok_or(NoPreview::Missing)?;
    Ok(match jpeg {
        [first, rest @ ..] if *first != 0xff && !preview::is_jpeg_xl(jpeg, 0) => {
            let mut mended = vec![0xff];
            mended.extend_from_slice(rest);
            std::borrow::Cow::Owned(mended)
        }
        _ => std::borrow::Cow::Borrowed(jpeg),
    })
}

/// The sensor data of the file `info` was read from; stops with
/// [`Error::Cancelled`] soon after `cancel` is set.
pub fn decode(info: &RawInfo, bytes: &[u8], cancel: &AtomicBool) -> Result<Sensor> {
    variant::decode(info, bytes, cancel)
}
