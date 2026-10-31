//! Embedded previews, JPEG or JPEG XL (Lightroom's DNGs of HDR and panorama
//! merges, whose JPEG is a 256-pixel thumbnail), found without decoding the
//! sensor data: each container lists its candidates, and the largest is the
//! one thumbnails use.
use crate::info::RawInfo;

/// An embedded JPEG or JPEG XL: where it is and, when known, its size in
/// pixels.
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
        .filter(|c| c.len > 2 && (is_jpeg(bytes, c.offset) || is_jpeg_xl(bytes, c.offset)))
        .max_by_key(|c| (c.pixels, c.len))
        .map(|c| (c.offset, c.len))
}

/// The candidate a thumbnail `edge` pixels on its long side is made from: of
/// the JPEGs at least `edge` pixels on their long side and of the largest's
/// shape (not a letterboxed or turned one), the one with the fewest pixels;
/// else the largest. The largest is often the full-size JPEG, many times the
/// pixels to decode of a mid-sized one beside it.
pub(crate) fn smallest_reaching(
    candidates: &[Candidate],
    bytes: &[u8],
    edge: u32,
) -> Option<(u64, u64)> {
    let largest = largest(candidates, bytes)?;
    let size = |(at, len): (u64, u64)| {
        is_jpeg(bytes, at)
            .then(|| jpeg_size(bytes, at, len))
            .flatten()
            .map(|[w, h]| [u64::from(w), u64::from(h)])
    };
    let Some([width, height]) = size(largest) else {
        return Some(largest);
    };
    // Within 2%: mid-sized previews round their size to whole blocks.
    let shaped = |[w, h]: [u64; 2]| (w * height).abs_diff(h * width) * 50 <= w * height;
    // Whole in the file: a preview cut short is no better than none.
    let whole = |c: &&Candidate| {
        c.len > 2
            && c.offset
                .checked_add(c.len)
                .is_some_and(|end| end <= bytes.len() as u64)
    };
    candidates
        .iter()
        .filter(whole)
        .filter_map(|c| Some(((c.offset, c.len), size((c.offset, c.len))?)))
        .filter(|&(_, [w, h])| w.max(h) >= u64::from(edge) && shaped([w, h]))
        .min_by_key(|&(_, [w, h])| w * h)
        .map(|(found, _)| found)
        .or(Some(largest))
}

/// Whether a JPEG starts at `at`: its start of image marker, or Epson's
/// (whose first byte is 0xEE) or Minolta's (whose first byte is another,
/// before the second marker's 0xFF), the first byte read as 0xFF (our
/// measurements).
pub(crate) fn is_jpeg(bytes: &[u8], at: u64) -> bool {
    usize::try_from(at)
        .ok()
        .and_then(|at| bytes.get(at..at.checked_add(3)?))
        .is_some_and(|b| b[..2] == [0xff, 0xd8] || b[..2] == [0xee, 0xd8] || b[1..] == [0xd8, 0xff])
}

/// Whether a JPEG XL starts at `at`: a bare codestream (ISO/IEC 18181-1) or
/// its container (18181-2).
pub fn is_jpeg_xl(bytes: &[u8], at: u64) -> bool {
    const CONTAINER: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10];
    usize::try_from(at)
        .ok()
        .and_then(|at| bytes.get(at..))
        .is_some_and(|b| b.starts_with(&[0xff, 0x0a]) || b.starts_with(&CONTAINER))
}

/// The width and height of the JPEG `len` bytes at `at`, from its first
/// start-of-frame segment (ITU T.81 B.2.2); `None` when the markers before
/// it do not lead there within the JPEG.
pub(crate) fn jpeg_size(bytes: &[u8], at: u64, len: u64) -> Option<[u32; 2]> {
    let start = usize::try_from(at).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    let jpeg = bytes.get(start..end.min(bytes.len()))?;
    let mut i = 2;
    while let [0xff, marker, ..] = *jpeg.get(i..)? {
        match marker {
            // Fill bytes before a marker.
            0xff => i += 1,
            // Start of frame, other than DHT, JPG and DAC: precision, then
            // height and width.
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                let f = jpeg.get(i + 4..i + 9)?;
                let (h, w) = (
                    u16::from_be_bytes([f[1], f[2]]),
                    u16::from_be_bytes([f[3], f[4]]),
                );
                return (w > 0 && h > 0).then_some([u32::from(w), u32::from(h)]);
            }
            // Start of scan or end of image before a frame.
            0xda | 0xd9 => return None,
            // Markers without a length.
            0x01 | 0xd0..=0xd7 => i += 2,
            _ => {
                let l = jpeg.get(i + 2..i + 4)?;
                i += 2 + usize::from(u16::from_be_bytes([l[0], l[1]]));
            }
        }
    }
    None
}

/// Why a file has no preview thumbnails can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPreview {
    /// The file has none.
    Missing,
    /// The file's previews are neither JPEG nor JPEG XL (uncompressed
    /// thumbnails).
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
    largest(&info.previews, bytes).ok_or(none(info))
}

/// Where the preview for a thumbnail `edge` pixels on its long side is in
/// the file (see [`smallest_reaching`]).
pub(crate) fn find_reaching(
    info: &RawInfo,
    bytes: &[u8],
    edge: u32,
) -> Result<(u64, u64), NoPreview> {
    smallest_reaching(&info.previews, bytes, edge).ok_or(none(info))
}

fn none(info: &RawInfo) -> NoPreview {
    if info.uncompressed_preview {
        NoPreview::NotJpeg
    } else {
        NoPreview::Missing
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_jpeg_may_start_with_another_first_byte() {
        let bytes = [0xff, 0xd8, 0xff, 0x02, 0xd8, 0xff, 0x02, 0xd8, 0x00];
        assert!(super::is_jpeg(&bytes, 0));
        assert!(super::is_jpeg(&bytes, 3));
        // Without the second marker's 0xFF, not a JPEG; nor past the end.
        assert!(!super::is_jpeg(&bytes, 6));
        assert!(!super::is_jpeg(&bytes, 8));
        assert!(!super::is_jpeg(&bytes, u64::MAX));
    }

    /// JPEG headers of these sizes, one after another, as candidates.
    fn jpegs(sizes: &[[u16; 2]]) -> (Vec<u8>, Vec<super::Candidate>) {
        let mut bytes = Vec::new();
        let mut candidates = Vec::new();
        for &[w, h] in sizes {
            let offset = bytes.len() as u64;
            bytes.extend([0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08]);
            bytes.extend(h.to_be_bytes());
            bytes.extend(w.to_be_bytes());
            bytes.extend([0; 12]);
            bytes.extend([0xff, 0xd9]);
            candidates.push(super::Candidate {
                offset,
                len: bytes.len() as u64 - offset,
                pixels: u64::from(w) * u64::from(h),
            });
        }
        (bytes, candidates)
    }

    fn reaching(sizes: &[[u16; 2]], edge: u32) -> usize {
        let (bytes, candidates) = jpegs(sizes);
        let (at, _) = super::smallest_reaching(&candidates, &bytes, edge).unwrap();
        candidates.iter().position(|c| c.offset == at).unwrap()
    }

    #[test]
    fn thumbnails_use_the_smallest_preview_that_reaches_their_size() {
        // A Canon CR3's: thumbnail, mid-sized preview, full-size JPEG.
        assert_eq!(reaching(&[[160, 120], [1620, 1080], [6000, 4000]], 640), 1);
        // Rounded to whole blocks, as Sony's are.
        assert_eq!(reaching(&[[6000, 4000], [1616, 1080]], 640), 1);
        assert_eq!(reaching(&[[6000, 4000], [640, 424], [1616, 1080]], 640), 1);
        assert_eq!(reaching(&[[6000, 4000], [640, 424]], 640), 1);
        // None reaches it but the largest.
        assert_eq!(reaching(&[[400, 267], [6000, 4000]], 640), 1);
    }

    #[test]
    fn thumbnails_keep_the_largest_preview_when_a_smaller_one_differs_in_shape() {
        // Letterboxed, as a Pentax's 640×480 of a 3:2 photo is.
        assert_eq!(reaching(&[[640, 480], [6000, 4000]], 640), 1);
        // Turned upright already.
        assert_eq!(reaching(&[[1080, 1620], [6000, 4000]], 640), 1);
        // Cut short by the end of the file.
        let (bytes, candidates) = jpegs(&[[6000, 4000], [1620, 1080]]);
        let cut = &bytes[..bytes.len() - 4];
        assert_eq!(
            super::smallest_reaching(&candidates, cut, 640),
            Some((candidates[0].offset, candidates[0].len))
        );
        // A largest whose size can't be read.
        let (mut bytes, candidates) = jpegs(&[[1620, 1080], [6000, 4000]]);
        bytes[candidates[1].offset as usize + 3] = 0xda;
        assert_eq!(
            super::smallest_reaching(&candidates, &bytes, 640),
            Some((candidates[1].offset, candidates[1].len))
        );
    }
}
