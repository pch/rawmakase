//! The embedded preview, and the header facts, read from a file in pieces,
//! without its sensor data:
//! for files on network volumes, where reading the whole file to show its
//! preview costs most of the time.
//!
//! The readers stay as they are. They are given a buffer the file's size in
//! which only the parts read from the file are filled: its start and end,
//! where the containers keep their directories and boxes, then the start of
//! each preview candidate (whose header says its size) and the chosen
//! preview whole. A reader that needed a part not read finds zeros there and
//! gives up, finds no preview, or finds fewer candidates. Giving up or finding
//! no preview is `None`, and the caller reads the whole file as before. Fewer
//! candidates can't be told from the file's own, so only formats compared with
//! whole-file reads are read this way ([`checked`]), and a preview smaller
//! than the one asked for is `None` too.
use crate::Variant;
use crate::info::RawInfo;
use std::io::{Read, Seek, SeekFrom};

/// What is read first from the file's start: directories, maker notes and
/// metadata boxes are within it in the files measured.
const HEAD: u64 = 512 << 10;
/// What is read from the file's end, for containers that keep their
/// directories there.
const TAIL: u64 = 64 << 10;
/// What is read of each candidate before one is chosen, and then again until
/// a JPEG's frame header, which says its size, is read.
const CANDIDATE_HEAD: u64 = 64 << 10;
/// The most read of a candidate to find its frame header: a JPEG's EXIF, ICC
/// profile and other segments before it are up to 64 KiB each. A file with a
/// longer candidate whose frame header isn't found within it is `None`.
const MAX_CANDIDATE_HEAD: u64 = 512 << 10;
/// The most candidates whose start is read.
const MAX_CANDIDATES: usize = 16;
/// The largest file read this way; a larger one is read as before.
const MAX_FILE: u64 = 4 << 30;

/// An embedded preview read in pieces ([`preview_from_file`]).
pub struct PartialPreview {
    /// The header facts, as [`crate::open`] reads them from the parts read.
    /// Facts the readers take from the sensor data (black levels measured from
    /// masked pixels, a frame found in the data) are not to be relied on.
    pub info: RawInfo,
    /// The preview, as [`crate::preview_reaching`] gives it: a JPEG.
    pub preview: Vec<u8>,
    /// The bytes read from the file.
    pub read: u64,
}

/// The embedded preview of the RAW `file`, read without its sensor data: the
/// smallest that reaches `edge` pixels on its long side, as
/// [`crate::preview_reaching`] chooses it. `None` when the file can't be read
/// this way; the caller then reads the whole file. That is the case when the
/// file isn't a RAW this crate reads, its preview isn't found in the parts
/// read or it can't be read; when its format isn't one of [`checked`]'s; and
/// when the preview found is smaller than `edge`.
pub fn preview_from_file(file: &mut (impl Read + Seek), edge: u32) -> Option<PartialPreview> {
    let partial = in_pieces(file, edge, checked)?;
    let reaches = crate::preview::jpeg_size(&partial.preview, 0, partial.preview.len() as u64)
        .is_some_and(|[w, h]| w.max(h) >= edge);
    reaches.then_some(partial)
}

/// Whether a preview read in pieces is the one the whole file gives, for
/// files of this `variant`. A reader that needs a part not read finds zeros
/// there, and can't tell them from zeros in the file: it may then drop a
/// candidate without an error and choose another. For these formats the
/// comparison with whole-file reads over the raw.pixls.us archive found no
/// difference; a format is added once `real_files_give_the_same_preview_read_in_pieces`
/// finds none for it. [`header_from_file`] reads photo info by the same list:
/// for it, `rawmakase-decode`'s `photo_info_from_the_header_is_the_whole_file_s`
/// is the comparison, as photo info can come from parts of the file a preview
/// doesn't need (an EXIF directory between the parts read reads as empty).
fn checked(variant: Variant) -> bool {
    use Variant::*;
    matches!(
        variant,
        DngUncompressed
            | DngLosslessJpeg
            | DngDeflate
            | DngUncompressedFloat
            | DngJpegXl
            | DngVc5
            | Rw2Delta
            | Rw2PackedRotated
            | Rw2Seeded
            | Rw2Packed
            | Rw2Huffman
            | Rw2Unpacked
            | ArwCurve
            | ArwUncompressed
            | ArwPacked
            | ArwLosslessJpeg
            | ArwYcbcr
            | ArwV1
            | ArwV6
            | NefHuffman
            | NefHighEfficiency
            | NefUnpacked
            | NefPacked
            | NefPacked14
            | NefSmallRaw
            | Cr3Lossless
            | Cr3Craw
            | OrfCompressed
            | PefHuffman
    )
}

/// A RAW's header facts read in pieces ([`header_from_file`]). The parts of
/// the file read aren't kept: nothing is to be decoded from them.
pub struct PartialHeader {
    /// The header facts, as [`crate::open`] reads them from the parts read;
    /// as [`PartialPreview::info`], facts taken from the sensor data are not
    /// to be relied on.
    pub info: RawInfo,
    /// The bytes read from the file.
    pub read: u64,
}

/// The header facts of the RAW `file`, read without its sensor data, as
/// [`preview_from_file`] reads them before choosing a preview: its start and
/// end, each preview candidate's start up to its frame header (a file whose
/// EXIF gives no image size takes its frame from the largest preview's), and
/// only for the formats [`checked`] accepts. `None` when it can't be read this
/// way; the caller then reads the whole file.
pub fn header_from_file(file: &mut (impl Read + Seek)) -> Option<PartialHeader> {
    let mut f = Sparse::new(file)?;
    let info = header_in_pieces(&mut f, checked)?;
    Some(PartialHeader { info, read: f.read })
}

/// The preview [`preview_from_file`] reads, for files whose variant `allow`
/// accepts, without its check of the preview's size.
fn in_pieces(
    file: &mut (impl Read + Seek),
    edge: u32,
    allow: impl Fn(Variant) -> bool,
) -> Option<PartialPreview> {
    let mut f = Sparse::new(file)?;
    let info = header_in_pieces(&mut f, allow)?;
    let (at, len) = crate::preview::find_reaching(&info, &f.bytes, edge).ok()?;
    f.fill(at, len)?;
    let preview = crate::embedded(&f.bytes, (at, len)).ok()?.into_owned();
    Some(PartialPreview {
        info,
        preview,
        read: f.read,
    })
}

/// The header facts of the file `f` reads, for files whose variant `allow`
/// accepts: its start and end, then each preview candidate's start up to its
/// JPEG frame header, at which it is opened again. `None` when they can't be
/// read this way.
fn header_in_pieces<R: Read + Seek>(
    f: &mut Sparse<'_, R>,
    allow: impl Fn(Variant) -> bool,
) -> Option<RawInfo> {
    f.fill(0, HEAD)?;
    f.fill(f.len().saturating_sub(TAIL), TAIL)?;
    let info = crate::variant::open(&f.bytes).ok()?;
    if !allow(info.variant) {
        return None;
    }
    // A candidate not read can't be told from one that isn't a preview.
    if info.previews.len() > MAX_CANDIDATES {
        return None;
    }
    for c in &info.previews {
        f.fill(c.offset, c.len.min(CANDIDATE_HEAD))?;
        // A JPEG's size is in its frame header, after its EXIF and other
        // segments, which may be longer than the part read first. It is
        // looked for in the part read alone, so that a frame header cut short
        // isn't completed with zeros.
        let mut read = c.len.min(CANDIDATE_HEAD);
        while crate::preview::is_jpeg(&f.bytes, c.offset)
            && crate::preview::jpeg_size(&f.bytes, c.offset, read).is_none()
        {
            if read >= c.len {
                // Read whole without a frame: the readers see the same
                // bytes of it as in the whole file.
                break;
            }
            if read >= MAX_CANDIDATE_HEAD {
                return None;
            }
            let more = CANDIDATE_HEAD.min(c.len - read);
            f.fill(c.offset + read, more)?;
            read += more;
        }
    }
    // Opened again now that the candidates' frames are read: a reader may
    // size its candidates from them (NEF's do), and a candidate of unknown
    // size could be taken for the largest. The candidates themselves come
    // from the directories already read; should they differ, a reader used
    // what wasn't read.
    let first: Vec<_> = info.previews.iter().map(|c| (c.offset, c.len)).collect();
    let info = crate::variant::open(&f.bytes).ok()?;
    if !allow(info.variant) || !info.previews.iter().map(|c| (c.offset, c.len)).eq(first) {
        return None;
    }
    Some(info)
}

/// A buffer the size of the file, filled where the file has been read.
struct Sparse<'r, R: Read + Seek> {
    file: &'r mut R,
    bytes: Vec<u8>,
    /// The ranges read, as start and end, sorted and apart.
    filled: Vec<(u64, u64)>,
    read: u64,
}

impl<'r, R: Read + Seek> Sparse<'r, R> {
    fn new(file: &'r mut R) -> Option<Self> {
        let len = file.seek(SeekFrom::End(0)).ok()?;
        if len == 0 || len > MAX_FILE {
            return None;
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(len).ok()?).ok()?;
        bytes.resize(usize::try_from(len).ok()?, 0);
        Some(Self {
            file,
            bytes,
            filled: Vec::new(),
            read: 0,
        })
    }
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    /// Reads `len` bytes at `at`, fewer where the file ends sooner, skipping
    /// what has been read already.
    fn fill(&mut self, at: u64, len: u64) -> Option<()> {
        let end = at.saturating_add(len).min(self.len());
        let start = at.min(end);
        for (from, to) in self.gaps(start, end) {
            self.file.seek(SeekFrom::Start(from)).ok()?;
            let range = usize::try_from(from).ok()?..usize::try_from(to).ok()?;
            self.file.read_exact(&mut self.bytes[range]).ok()?;
            self.read += to - from;
        }
        self.mark(start, end);
        Some(())
    }
    /// The parts of `start..end` not read yet.
    fn gaps(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let mut gaps = Vec::new();
        let mut at = start;
        for &(s, e) in &self.filled {
            if e <= at {
                continue;
            }
            if s >= end {
                break;
            }
            if s > at {
                gaps.push((at, s));
            }
            at = at.max(e);
        }
        if at < end {
            gaps.push((at, end));
        }
        gaps
    }
    /// Records `start..end` as read.
    fn mark(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        self.filled.push((start, end));
        self.filled.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.filled.len());
        for &(s, e) in &self.filled {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.filled = merged;
    }
}

#[cfg(test)]
mod tests;
