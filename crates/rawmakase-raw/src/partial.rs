//! The embedded preview read from a file in pieces, without its sensor data:
//! for files on network volumes, where reading the whole file to show its
//! preview costs most of the time.
//!
//! The readers stay as they are. They are given a buffer the file's size in
//! which only the parts read from the file are filled: its start and end,
//! where the containers keep their directories and boxes, then the start of
//! each preview candidate (whose header says its size) and the chosen
//! preview whole. A reader that needed a part not read finds zeros there and
//! gives up, finds no preview, or finds fewer candidates; the first two are
//! `None`, and the caller reads the whole file as before.
use crate::info::RawInfo;
use std::io::{Read, Seek, SeekFrom};

/// What is read first from the file's start: directories, maker notes and
/// metadata boxes are within it in the files measured.
const HEAD: u64 = 512 << 10;
/// What is read from the file's end, for containers that keep their
/// directories there.
const TAIL: u64 = 64 << 10;
/// What is read of each candidate before one is chosen: a JPEG's markers up to
/// its frame header, EXIF included, which say its size.
const CANDIDATE_HEAD: u64 = 64 << 10;
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
    /// The preview, as [`crate::preview()`] gives it: JPEG or JPEG XL.
    pub preview: Vec<u8>,
    /// The bytes read from the file.
    pub read: u64,
}

/// The embedded preview of the RAW `file`, read without its sensor data: the
/// smallest that reaches `edge` pixels on its long side as
/// [`crate::preview_reaching`] chooses it, or with `edge` `None` the largest as
/// [`crate::preview()`] does. `None` when the file can't be read this way (it
/// isn't a RAW this crate reads, its preview isn't found in the parts read, or
/// it can't be read); the caller then reads the whole file.
pub fn preview_from_file(
    file: &mut (impl Read + Seek),
    edge: Option<u32>,
) -> Option<PartialPreview> {
    let mut f = Sparse::new(file)?;
    f.fill(0, HEAD)?;
    f.fill(f.len().saturating_sub(TAIL), TAIL)?;
    let info = crate::variant::open(&f.bytes).ok()?;
    for c in info.previews.iter().take(MAX_CANDIDATES) {
        f.fill(c.offset, c.len.min(CANDIDATE_HEAD))?;
    }
    let found = match edge {
        Some(edge) => crate::preview::find_reaching(&info, &f.bytes, edge),
        None => crate::preview::find(&info, &f.bytes),
    };
    let (at, len) = found.ok()?;
    f.fill(at, len)?;
    let preview = crate::embedded(&f.bytes, (at, len)).ok()?.into_owned();
    Some(PartialPreview {
        info,
        preview,
        read: f.read,
    })
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
        bytes.resize(bytes.capacity(), 0);
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
