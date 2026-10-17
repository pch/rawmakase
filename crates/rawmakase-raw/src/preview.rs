//! Embedded JPEG previews, found without decoding the sensor data: each
//! container lists its candidates, and the largest is the one thumbnails use.
use crate::info::RawInfo;

/// An embedded JPEG: where it is and, when known, its size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub offset: u64,
    pub len: u64,
    pub pixels: u64,
}

/// The candidate thumbnails use: the most pixels, then the most bytes.
pub(crate) fn largest(candidates: &[Candidate], bytes: &[u8]) -> Option<(u64, u64)> {
    candidates
        .iter()
        .filter(|c| c.len > 2 && is_jpeg(bytes, c.offset))
        .max_by_key(|c| (c.pixels, c.len))
        .map(|c| (c.offset, c.len))
}

/// Whether a JPEG starts at `at`: its start of image marker, or Epson's
/// (whose first byte is 0xEE, read as 0xFF; our measurements).
pub(crate) fn is_jpeg(bytes: &[u8], at: u64) -> bool {
    usize::try_from(at)
        .ok()
        .and_then(|at| bytes.get(at..at.checked_add(2)?))
        .is_some_and(|b| b == [0xff, 0xd8] || b == [0xee, 0xd8])
}

/// Why a file has no preview thumbnails can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPreview {
    /// The file has none.
    Missing,
    /// The file's previews are not JPEG (uncompressed thumbnails).
    NotJpeg,
}

impl std::fmt::Display for NoPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NoPreview::Missing => "No thumbnail in file",
            NoPreview::NotJpeg => "Embedded preview is not JPEG",
        })
    }
}

impl std::error::Error for NoPreview {}

/// Where the preview thumbnails use is in the file: offset and length.
pub(crate) fn find(info: &RawInfo, bytes: &[u8]) -> Result<(u64, u64), NoPreview> {
    largest(&info.previews, bytes).ok_or(if info.uncompressed_preview {
        NoPreview::NotJpeg
    } else {
        NoPreview::Missing
    })
}
