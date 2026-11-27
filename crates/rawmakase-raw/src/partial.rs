//! The embedded preview, and the header facts, read from a file in pieces,
//! without its sensor data: for Library thumbnails and photo info, where
//! reading the whole file costs its size in memory and most of the time, and
//! for Develop's placeholder on network volumes.
//!
//! The readers stay as they are. They are given a buffer the file's size in
//! which only the parts read from the file are filled: its start and end,
//! where most containers keep their directories and boxes; the parts a
//! container says its header is in beyond those (`variant::header_parts`: a
//! RAF's records and raw directory, which follow its embedded JPEG, a TIFF's
//! directories and their short values wherever they are, the start of a
//! CR2's or a NEF's sensor data, a CR3's timed metadata and the start of the
//! JPEG after an Olympus compact's sensor data); then the start of each
//! preview candidate (whose header says its size) and the chosen preview
//! whole. A reader that needed a part not read finds zeros there and
//! gives up, finds no preview, finds fewer candidates or reads a fact as
//! absent. Giving up or finding no preview is `None`, and the caller reads
//! the whole file as before. The others can't be told from the file's own,
//! so only formats compared with whole-file reads are read this way
//! ([`checked`]), and a preview smaller than the one asked for is `None` too.
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
/// How many times a container is asked for the parts its header is in
/// ([`Sparse::fill_header_parts`]): an older CR2 shows one directory more each
/// time, and has four; a directory's values may take a round more.
const MAX_ROUNDS: usize = 8;
/// The longest such part read. A RAF's records are up to 59 KiB in the files
/// measured, a CR2's maker note 70 KiB.
const MAX_PART: u64 = 1 << 20;
/// Such parts closer than this are read as one, with what is between them:
/// a directory's values are many and next to each other, and each read is a
/// round trip on a network volume.
const JOIN: u64 = 64 << 10;

/// An embedded preview read in pieces ([`preview_from_file`]).
pub struct PartialPreview {
    /// The header facts, as [`crate::open`] reads them from the parts read.
    /// Facts the readers take from the sensor data (black levels measured from
    /// masked pixels, a frame found in the data) are not to be relied on.
    pub info: RawInfo,
    /// The preview, as [`crate::preview_reaching`] gives it ([`preview_from_file`])
    /// or [`crate::preview()`] does ([`largest_preview_from_file`]).
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

/// The largest embedded preview of the RAW `file`, as [`crate::preview()`]
/// gives it, read without its sensor data: for Develop's placeholder while
/// the whole file is read. Read as [`preview_from_file`] reads one, for the
/// formats [`checked`] accepts, with the file opened again once its
/// candidates' frames are read, as a reader may size its candidates from
/// them. `None` when it can't be read this way.
pub fn largest_preview_from_file(file: &mut (impl Read + Seek)) -> Option<PartialPreview> {
    largest_in_pieces(file, checked)
}

/// The preview [`largest_preview_from_file`] reads, for files whose variant
/// `allow` accepts.
fn largest_in_pieces(
    file: &mut (impl Read + Seek),
    allow: impl Fn(Variant) -> bool,
) -> Option<PartialPreview> {
    let mut f = Sparse::new(file)?;
    let info = header_in_pieces(&mut f, allow)?;
    let (at, len) = crate::preview::find(&info, f.source()).ok()?;
    f.fill(at, len)?;
    let preview = crate::embedded_source(f.source(), (at, len))
        .ok()?
        .into_owned();
    Some(PartialPreview {
        info,
        preview,
        read: f.read,
    })
}

/// Whether a preview read in pieces is the one the whole file gives, for
/// files of this `variant`. A reader that needs a part not read finds zeros
/// there, and can't tell them from zeros in the file: it may then drop a
/// candidate without an error and choose another, or read a fact as absent.
/// For these formats the comparison with whole-file reads over the
/// raw.pixls.us archive found no difference; a format is added once
/// `real_files_give_the_same_preview_read_in_pieces` finds none for it, in
/// the preview or in any header fact. [`header_from_file`] reads photo info
/// by the same list: for it, `rawmakase-decode`'s
/// `photo_info_from_the_header_is_the_whole_file_s` is the comparison, as it
/// also covers the files without a preview to compare.
fn checked(variant: Variant) -> bool {
    use Variant::*;
    matches!(
        variant,
        DngUncompressed
            | DngLosslessJpeg
            | DngDeflate
            | DngLossyJpeg
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
            | OrfPacked
            | OrfUnpacked
            | PefPacked
            | PefUnpacked
            | RafUnpacked
            | RafSuperCcdSr
            | RafTiled
            | RafPacked
            | RafPackedWords
            | RafCompressed
            | Cr2LosslessJpeg
            | Cr2Sraw
            | SrwUnpacked
            | SrwPacked
            | SrwBlocks
            | SrwHuffman
            | SrwTiles
            | CrwCompressed
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
/// end, the parts its container names, each preview candidate's start up to
/// its frame header (a file whose EXIF gives no image size takes its frame
/// from the largest preview's, and a RAF's EXIF is in its embedded JPEG,
/// before that frame header), and only for the formats [`checked`] accepts.
/// `None` when it can't be read this way; the caller then reads the whole
/// file.
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
    let (at, len) = crate::preview::find_reaching(&info, f.source(), edge).ok()?;
    f.fill(at, len)?;
    let preview = crate::embedded_source(f.source(), (at, len))
        .ok()?
        .into_owned();
    Some(PartialPreview {
        info,
        preview,
        read: f.read,
    })
}

/// The header facts of the file `f` reads, for files whose variant `allow`
/// accepts: its start and end and the parts its container names, then each
/// preview candidate's start up to its JPEG frame header, at which it is
/// opened again. `None` when they can't be read this way.
fn header_in_pieces<R: Read + Seek>(
    f: &mut Sparse<'_, R>,
    allow: impl Fn(Variant) -> bool,
) -> Option<RawInfo> {
    f.fill(0, HEAD)?;
    f.fill(f.len().saturating_sub(TAIL), TAIL)?;
    f.fill_header_parts()?;
    let info = crate::variant::open_source(f.source()).ok()?;
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
        while crate::preview::is_jpeg(f.source(), c.offset)
            && crate::preview::jpeg_size(f.source(), c.offset, read).is_none()
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
    // size could be taken for the largest. A RAF's reader takes the EXIF
    // from its embedded JPEG, whose segments before the frame header are now
    // read whole. The candidates themselves come from the directories
    // already read; should they differ, a reader used what wasn't read.
    let first: Vec<_> = info.previews.iter().map(|c| (c.offset, c.len)).collect();
    let info = crate::variant::open_source(f.source()).ok()?;
    if !allow(info.variant) || !info.previews.iter().map(|c| (c.offset, c.len)).eq(first) {
        return None;
    }
    Some(info)
}

/// A buffer the size of the file, filled where the file has been read. It
/// is filled with zeros first, so it takes the file's size in memory while
/// it is read, as the file read whole does: what reading in pieces saves is
/// the reading (measured 2026-11-18 on macOS: a 100 MB RAF's preview peaks at
/// 111 MB either way). A buffer the allocator zeroes instead is no smaller
/// there, as a freed block of the size is taken again and written with zeros.
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
    /// The buffer as the readers read it: zeros where the file hasn't been
    /// read.
    fn source(&self) -> crate::io::Source<'_> {
        crate::io::Source::Slice(&self.bytes)
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
    /// Reads the parts the file's container keeps its header in beyond the
    /// start and the end ([`crate::variant::header_parts`]): a RAF's records
    /// and raw directory, after its embedded JPEG; a TIFF's directories and
    /// their values; a CR2's or a NEF's sensor data's start; a CR3's timed
    /// metadata; the start of the JPEG after an Olympus compact's sensor
    /// data. A part read may show where the next is, so the container is
    /// asked until every part it names is read. Parts less than [`JOIN`]
    /// apart are read as one. `None` when the container says it can't be
    /// read in pieces, a part is longer than [`MAX_PART`], or parts are still
    /// missing after [`MAX_ROUNDS`].
    fn fill_header_parts(&mut self) -> Option<()> {
        for _ in 0..MAX_ROUNDS {
            let mut parts = crate::variant::header_parts(self.source())?;
            parts.retain(|&(at, len)| !self.gaps(at, at.saturating_add(len)).is_empty());
            if parts.is_empty() {
                return Some(());
            }
            if parts.iter().any(|&(_, len)| len > MAX_PART) {
                return None;
            }
            parts.sort_unstable();
            let mut joined: Vec<(u64, u64)> = Vec::with_capacity(parts.len());
            for (at, len) in parts {
                let end = at.saturating_add(len);
                match joined.last_mut() {
                    Some(last) if at <= last.1.saturating_add(JOIN) => last.1 = last.1.max(end),
                    _ => joined.push((at, end)),
                }
            }
            for (start, end) in joined {
                self.fill(start, end - start)?;
            }
        }
        None
    }
    /// The parts of `start..end` not read yet, as far as the file goes.
    fn gaps(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let end = end.min(self.len());
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
