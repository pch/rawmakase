//! Bounded reading of the file's bytes, in memory or in pieces from the
//! file, and the allocation limits every decoder checks header sizes
//! against before allocating.
use crate::error::{Error, Result, corrupt};
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};

/// The largest sensor decoded, in samples: 400 megapixels of four channels.
pub(crate) const MAX_SAMPLES: u64 = 1_600_000_000;
/// The largest side of a sensor image.
pub(crate) const MAX_SIDE: u32 = 65_535;

/// The number of samples of a `width × height × channels` image, checked
/// against the limits.
pub(crate) fn samples(width: u32, height: u32, channels: u32) -> Result<usize> {
    if width == 0 || height == 0 || channels == 0 {
        return Err(corrupt("empty image"));
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::TooLarge("image side"));
    }
    let n = u64::from(width) * u64::from(height) * u64::from(channels);
    if n > MAX_SAMPLES {
        return Err(Error::TooLarge("image samples"));
    }
    usize::try_from(n).map_err(|_| Error::TooLarge("image samples"))
}

/// `n` zero samples, allocated fallibly.
pub(crate) fn zeroed(n: usize) -> Result<Vec<u16>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n)
        .map_err(|_| Error::TooLarge("image allocation"))?;
    v.resize(n, 0);
    Ok(v)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    Little,
    Big,
}

/// A view of the file's bytes with bounds-checked, endian-aware reads.
#[derive(Clone, Copy)]
pub(crate) struct Bytes<'a> {
    pub data: &'a [u8],
    pub order: Order,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8], order: Order) -> Self {
        Self { data, order }
    }
    /// `len` bytes at `at`.
    pub(crate) fn slice(&self, at: u64, len: u64) -> Result<&'a [u8]> {
        let start = usize::try_from(at).map_err(|_| corrupt("offset"))?;
        let len = usize::try_from(len).map_err(|_| corrupt("length"))?;
        start
            .checked_add(len)
            .and_then(|end| self.data.get(start..end))
            .ok_or_else(|| corrupt(format!("{len} bytes at {at} past the end of the file")))
    }
    /// The 32-bit value at `at`.
    pub(crate) fn u32(&self, at: u64) -> Result<u32> {
        let b = self.slice(at, 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Ok(match self.order {
            Order::Little => u32::from_le_bytes(b),
            Order::Big => u32::from_be_bytes(b),
        })
    }
}

/// A file that can be read anywhere.
pub(crate) trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// The most bytes read in one piece: a header, a directory, a metadata box.
pub(crate) const MAX_PIECE: u64 = 16 << 20;

/// A file read in pieces, for reads that need a few of its parts and not
/// the rest (`captured`).
pub(crate) struct Pieces<'r> {
    file: &'r mut dyn ReadSeek,
    /// The file's length.
    pub len: u64,
}

impl<'r> Pieces<'r> {
    pub(crate) fn new(file: &'r mut dyn ReadSeek) -> Option<Self> {
        let len = file.seek(SeekFrom::End(0)).ok()?;
        Some(Self { file, len })
    }
    /// The `len` bytes at `at`; `None` past the end of the file, longer
    /// than [`MAX_PIECE`] or when the file can't be read.
    pub(crate) fn read(&mut self, at: u64, len: u64) -> Option<Vec<u8>> {
        if len > MAX_PIECE || at.checked_add(len)? > self.len {
            return None;
        }
        self.file.seek(SeekFrom::Start(at)).ok()?;
        let mut piece = vec![0; usize::try_from(len).ok()?];
        self.file.read_exact(&mut piece).ok()?;
        Some(piece)
    }
    /// As [`Self::read`], fewer bytes where the file ends sooner.
    pub(crate) fn read_upto(&mut self, at: u64, len: u64) -> Option<Vec<u8>> {
        self.read(at, len.min(self.len.checked_sub(at)?))
    }
}

/// Sparse byte storage backed only by ranges explicitly filled from a seekable
/// reader. Unread positions yield zero bytes for short fixed reads without
/// reserving whole-file memory, and contiguous multi-byte reads spanning unread
/// gaps are capped at [`MAX_PIECE`].
#[derive(Debug)]
pub(crate) struct SparseData {
    len: u64,
    read: u64,
    /// Disjoint, sorted, non-adjacent `(start, bytes)` segments.
    segments: Vec<(u64, Vec<u8>)>,
}

impl SparseData {
    pub(crate) fn new(len: u64) -> Self {
        Self {
            len,
            read: 0,
            segments: Vec::new(),
        }
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn read(&self) -> u64 {
        self.read
    }

    #[cfg(test)]
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.segments.iter().map(|(_, s)| s.len()).sum()
    }

    #[cfg(test)]
    pub(crate) fn filled_ranges(&self) -> Vec<(u64, u64)> {
        self.segments
            .iter()
            .map(|(start, s)| (*start, *start + s.len() as u64))
            .collect()
    }

    /// Returns whether `[start, end)` (clamped to [`Self::len`]) contains any
    /// byte not read yet.
    pub(crate) fn has_gap(&self, start: u64, end: u64) -> bool {
        self.next_gap(start, end).is_some()
    }

    /// The first unfilled `[from, to)` sub-range at or after `start` within
    /// `[start, end)` (clamped to [`Self::len`]).
    fn next_gap(&self, start: u64, end: u64) -> Option<(u64, u64)> {
        let mut cursor = start.min(self.len);
        let end = end.min(self.len);
        if cursor >= end {
            return None;
        }
        let mut idx = self.first_segment_after(cursor);
        while let Some((seg_start, seg_data)) = self.segments.get(idx) {
            if *seg_start >= end {
                break;
            }
            if cursor < *seg_start {
                return Some((cursor, (*seg_start).min(end)));
            }
            cursor = cursor.max(*seg_start + seg_data.len() as u64);
            if cursor >= end {
                return None;
            }
            idx += 1;
        }
        (cursor < end).then_some((cursor, end))
    }

    /// Reads any missing bytes in `[at, at + len)` (clamped to [`Self::len`])
    /// from `file`, merging adjacent or overlapping segments so subsequent
    /// slice reads borrow without copying.
    pub(crate) fn fill(&mut self, file: &mut (impl Read + Seek), at: u64, len: u64) -> Option<()> {
        let end = at.saturating_add(len).min(self.len);
        let mut cursor = at.min(end);
        while let Some((gs, ge)) = self.next_gap(cursor, end) {
            let gap_len = usize::try_from(ge - gs).ok()?;
            let mut buf = Vec::new();
            buf.try_reserve_exact(gap_len).ok()?;
            buf.resize(gap_len, 0);
            self.segments.try_reserve(1).ok()?;
            read_exact_at(file, gs, &mut buf, &mut self.read)?;
            self.segments.push((gs, buf));
            self.coalesce()?;
            cursor = ge;
        }
        Some(())
    }

    /// Reads `[at, at + len)` directly into one newly allocated buffer, reusing
    /// already-loaded bytes and reading only missing gaps from `file` without
    /// inserting them into `self.segments`.
    pub(crate) fn read_range(
        &mut self,
        file: &mut (impl Read + Seek),
        at: u64,
        len: u64,
    ) -> Option<Vec<u8>> {
        let end = at.checked_add(len)?;
        if end > self.len {
            return None;
        }
        let len_usize = usize::try_from(len).ok()?;
        let mut out = Vec::new();
        out.try_reserve_exact(len_usize).ok()?;
        out.resize(len_usize, 0);
        let mut cursor = at;
        let mut idx = self.first_segment_after(at);
        while cursor < end {
            if let Some((seg_start, seg_data)) = self.segments.get(idx)
                && *seg_start < end
            {
                if cursor < *seg_start {
                    let gap_end = (*seg_start).min(end);
                    let dst_off = (cursor - at) as usize;
                    let gap_len = (gap_end - cursor) as usize;
                    read_exact_at(
                        file,
                        cursor,
                        &mut out[dst_off..dst_off + gap_len],
                        &mut self.read,
                    )?;
                    cursor = gap_end;
                    continue;
                }
                let seg_end = (*seg_start + seg_data.len() as u64).min(end);
                let count = (seg_end - cursor) as usize;
                let dst_off = (cursor - at) as usize;
                let src_off = (cursor - *seg_start) as usize;
                out[dst_off..dst_off + count].copy_from_slice(&seg_data[src_off..src_off + count]);
                cursor = seg_end;
                idx += 1;
            } else {
                let dst_off = (cursor - at) as usize;
                let gap_len = (end - cursor) as usize;
                read_exact_at(
                    file,
                    cursor,
                    &mut out[dst_off..dst_off + gap_len],
                    &mut self.read,
                )?;
                break;
            }
        }
        Some(out)
    }

    fn coalesce(&mut self) -> Option<()> {
        self.segments.sort_unstable_by_key(|(start, _)| *start);
        let mut write = 0usize;
        for read in 0..self.segments.len() {
            if write == 0 {
                write = 1;
                continue;
            }
            let prev_end = {
                let (s, d) = &self.segments[write - 1];
                *s + d.len() as u64
            };
            let curr_start = self.segments[read].0;
            if curr_start <= prev_end {
                let overlap = (prev_end - curr_start) as usize;
                let extra = self.segments[read].1.len().saturating_sub(overlap);
                if extra > 0 {
                    if self.segments[write - 1].1.try_reserve_exact(extra).is_err() {
                        self.segments.retain(|(_, d)| !d.is_empty());
                        return None;
                    }
                    let curr = std::mem::take(&mut self.segments[read].1);
                    self.segments[write - 1]
                        .1
                        .extend_from_slice(&curr[overlap..]);
                } else {
                    self.segments[read].1 = Vec::new();
                }
            } else {
                if write != read {
                    self.segments.swap(write, read);
                }
                write += 1;
            }
        }
        self.segments.truncate(write);
        Some(())
    }

    fn first_segment_after(&self, at: u64) -> usize {
        self.segments
            .partition_point(|(start, data)| *start + data.len() as u64 <= at)
    }

    pub(crate) fn byte_at(&self, at: u64) -> Option<u8> {
        if at >= self.len {
            return None;
        }
        let idx = self.first_segment_after(at);
        if let Some((start, data)) = self.segments.get(idx)
            && *start <= at
        {
            return Some(data[(at - *start) as usize]);
        }
        Some(0)
    }

    pub(crate) fn array_at<const N: usize>(&self, at: u64) -> Option<[u8; N]> {
        let end = at.checked_add(N as u64)?;
        if end > self.len {
            return None;
        }
        let mut out = [0u8; N];
        if N == 0 {
            return Some(out);
        }
        self.copy_into(at, &mut out);
        Some(out)
    }

    fn copy_into(&self, at: u64, dst: &mut [u8]) {
        let end = at + dst.len() as u64;
        let mut idx = self.first_segment_after(at);
        while let Some((seg_start, seg_data)) = self.segments.get(idx) {
            if *seg_start >= end {
                break;
            }
            let copy_start = at.max(*seg_start);
            let copy_end = end.min(*seg_start + seg_data.len() as u64);
            if copy_start < copy_end {
                let count = (copy_end - copy_start) as usize;
                let dst_off = (copy_start - at) as usize;
                let src_off = (copy_start - *seg_start) as usize;
                dst[dst_off..dst_off + count].copy_from_slice(&seg_data[src_off..src_off + count]);
            }
            idx += 1;
        }
    }

    pub(crate) fn get(&self, at: u64, len: u64) -> Option<Cow<'_, [u8]>> {
        let end = at.checked_add(len)?;
        if end > self.len {
            return None;
        }
        if len == 0 {
            return Some(Cow::Borrowed(&[]));
        }
        let len_usize = usize::try_from(len).ok()?;
        let idx = self.first_segment_after(at);
        if let Some((seg_start, seg_data)) = self.segments.get(idx)
            && *seg_start <= at
            && *seg_start + seg_data.len() as u64 >= end
        {
            let off = (at - *seg_start) as usize;
            return Some(Cow::Borrowed(&seg_data[off..off + len_usize]));
        }
        if len > MAX_PIECE {
            return None;
        }
        let mut out = Vec::new();
        out.try_reserve_exact(len_usize).ok()?;
        out.resize(len_usize, 0);
        self.copy_into(at, &mut out);
        Some(Cow::Owned(out))
    }

    pub(crate) fn starts_with_at(&self, at: u64, prefix: &[u8]) -> bool {
        let Some(end) = at.checked_add(prefix.len() as u64) else {
            return false;
        };
        if end > self.len {
            return false;
        }
        let mut idx = self.first_segment_after(at);
        let mut cursor = at;
        while cursor < end {
            let p_off = (cursor - at) as usize;
            if let Some((seg_start, seg_data)) = self.segments.get(idx)
                && *seg_start < end
            {
                if cursor < *seg_start {
                    let gap_end = (*seg_start).min(end);
                    let gap_len = (gap_end - cursor) as usize;
                    if !prefix[p_off..p_off + gap_len].iter().all(|&b| b == 0) {
                        return false;
                    }
                    cursor = gap_end;
                    continue;
                }
                let seg_end = (*seg_start + seg_data.len() as u64).min(end);
                let count = (seg_end - cursor) as usize;
                let src_off = (cursor - *seg_start) as usize;
                if seg_data[src_off..src_off + count] != prefix[p_off..p_off + count] {
                    return false;
                }
                cursor = seg_end;
                idx += 1;
            } else {
                let tail_len = (end - cursor) as usize;
                return prefix[p_off..p_off + tail_len].iter().all(|&b| b == 0);
            }
        }
        true
    }

    pub(crate) fn find_byte(&self, from: u64, to: u64, needle: u8) -> Option<u64> {
        let end = to.min(self.len);
        if from >= end {
            return None;
        }
        let mut cursor = from;
        let mut idx = self.first_segment_after(from);
        while cursor < end {
            if let Some((seg_start, seg_data)) = self.segments.get(idx)
                && *seg_start < end
            {
                if cursor < *seg_start {
                    let gap_end = (*seg_start).min(end);
                    if needle == 0 && cursor < gap_end {
                        return Some(cursor);
                    }
                    cursor = gap_end;
                    continue;
                }
                let seg_end = (*seg_start + seg_data.len() as u64).min(end);
                let count = (seg_end - cursor) as usize;
                let src_off = (cursor - *seg_start) as usize;
                if let Some(pos) = seg_data[src_off..src_off + count]
                    .iter()
                    .position(|&b| b == needle)
                {
                    return Some(cursor + pos as u64);
                }
                cursor = seg_end;
                idx += 1;
            } else {
                return (needle == 0).then_some(cursor);
            }
        }
        None
    }
}

fn read_exact_at(
    file: &mut (impl Read + Seek),
    at: u64,
    dst: &mut [u8],
    read: &mut u64,
) -> Option<()> {
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(dst).ok()?;
    *read += dst.len() as u64;
    Some(())
}

/// Random-access view over a whole in-memory file slice or a sparse file.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source<'a> {
    Slice(&'a [u8]),
    Sparse {
        data: &'a SparseData,
        base: u64,
        len: u64,
    },
}

impl<'a> From<&'a [u8]> for Source<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Self::Slice(bytes)
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for Source<'a> {
    fn from(bytes: &'a [u8; N]) -> Self {
        Self::Slice(bytes)
    }
}

impl<'a> From<&'a Vec<u8>> for Source<'a> {
    fn from(bytes: &'a Vec<u8>) -> Self {
        Self::Slice(bytes)
    }
}

impl<'a> Source<'a> {
    pub(crate) fn len(self) -> u64 {
        match self {
            Self::Slice(s) => s.len() as u64,
            Self::Sparse { len, .. } => len,
        }
    }

    /// Returns whether `[at, at + len)` overflows, falls out of bounds, or
    /// covers any byte not loaded yet in a sparse view.
    pub(crate) fn has_gap(self, at: u64, len: u64) -> bool {
        let Some(end) = at.checked_add(len) else {
            return true;
        };
        match self {
            Self::Slice(s) => end > s.len() as u64,
            Self::Sparse {
                data,
                base,
                len: view_len,
            } => end > view_len || data.has_gap(base + at, base + end),
        }
    }

    pub(crate) fn slice_from(self, at: u64) -> Option<Self> {
        let len = self.len().checked_sub(at)?;
        self.slice_exact(at, len)
    }

    pub(crate) fn slice_exact(self, at: u64, len: u64) -> Option<Self> {
        let end = at.checked_add(len)?;
        if end > self.len() {
            return None;
        }
        Some(match self {
            Self::Slice(s) => {
                Self::Slice(s.get(usize::try_from(at).ok()?..usize::try_from(end).ok()?)?)
            }
            Self::Sparse { data, base, .. } => Self::Sparse {
                data,
                base: base.checked_add(at)?,
                len,
            },
        })
    }

    pub(crate) fn byte(self, at: u64) -> Option<u8> {
        match self {
            Self::Slice(s) => usize::try_from(at).ok().and_then(|i| s.get(i).copied()),
            Self::Sparse { data, base, len } => {
                (at < len).then(|| data.byte_at(base + at)).flatten()
            }
        }
    }

    pub(crate) fn array<const N: usize>(self, at: u64) -> Option<[u8; N]> {
        match self {
            Self::Slice(s) => {
                let start = usize::try_from(at).ok()?;
                let end = start.checked_add(N)?;
                s.get(start..end)?.try_into().ok()
            }
            Self::Sparse { data, base, len } => {
                let end = at.checked_add(N as u64)?;
                if end > len {
                    return None;
                }
                data.array_at(base + at)
            }
        }
    }

    pub(crate) fn get(self, at: u64, len: u64) -> Option<Cow<'a, [u8]>> {
        match self {
            Self::Slice(s) => {
                let start = usize::try_from(at).ok()?;
                let end = start.checked_add(usize::try_from(len).ok()?)?;
                s.get(start..end).map(Cow::Borrowed)
            }
            Self::Sparse {
                data,
                base,
                len: view_len,
            } => {
                let end = at.checked_add(len)?;
                if end > view_len {
                    return None;
                }
                data.get(base + at, len)
            }
        }
    }

    pub(crate) fn u16(self, order: Order, at: u64) -> Option<u16> {
        let b = self.array::<2>(at)?;
        Some(match order {
            Order::Little => u16::from_le_bytes(b),
            Order::Big => u16::from_be_bytes(b),
        })
    }

    pub(crate) fn u32(self, order: Order, at: u64) -> Option<u32> {
        let b = self.array::<4>(at)?;
        Some(match order {
            Order::Little => u32::from_le_bytes(b),
            Order::Big => u32::from_be_bytes(b),
        })
    }

    pub(crate) fn u64(self, order: Order, at: u64) -> Option<u64> {
        let b = self.array::<8>(at)?;
        Some(match order {
            Order::Little => u64::from_le_bytes(b),
            Order::Big => u64::from_be_bytes(b),
        })
    }

    pub(crate) fn starts_with(self, prefix: &[u8]) -> bool {
        self.starts_with_at(0, prefix)
    }

    pub(crate) fn starts_with_at(self, at: u64, prefix: &[u8]) -> bool {
        match self {
            Self::Slice(s) => usize::try_from(at)
                .ok()
                .and_then(|i| s.get(i..))
                .is_some_and(|rest| rest.starts_with(prefix)),
            Self::Sparse { data, base, len } => at
                .checked_add(prefix.len() as u64)
                .is_some_and(|end| end <= len && data.starts_with_at(base + at, prefix)),
        }
    }

    pub(crate) fn find_byte(self, from: u64, to: u64, needle: u8) -> Option<u64> {
        match self {
            Self::Slice(s) => {
                let start = usize::try_from(from).ok()?;
                let end = usize::try_from(to.min(s.len() as u64)).ok()?;
                let slice = s.get(start..end)?;
                slice
                    .iter()
                    .position(|&b| b == needle)
                    .map(|p| from + p as u64)
            }
            Self::Sparse { data, base, len } => {
                let clamped_to = to.min(len);
                data.find_byte(
                    base.checked_add(from)?,
                    base.checked_add(clamped_to)?,
                    needle,
                )
                .map(|abs| abs - base)
            }
        }
    }

    pub(crate) fn reader(self) -> SourceReader<'a> {
        SourceReader {
            source: self,
            pos: 0,
        }
    }
}

/// [`Read`] + [`Seek`] cursor over a [`Source`].
pub(crate) struct SourceReader<'a> {
    source: Source<'a>,
    pos: u64,
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = self.source.len();
        if self.pos >= len || buf.is_empty() {
            return Ok(0);
        }
        let avail = (len - self.pos).min(buf.len() as u64) as usize;
        match self.source {
            Source::Slice(s) => {
                let start = self.pos as usize;
                buf[..avail].copy_from_slice(&s[start..start + avail]);
            }
            Source::Sparse { data, base, .. } => {
                buf[..avail].fill(0);
                data.copy_into(base + self.pos, &mut buf[..avail]);
            }
        }
        self.pos += avail as u64;
        Ok(avail)
    }
}

impl Seek for SourceReader<'_> {
    fn seek(&mut self, style: SeekFrom) -> std::io::Result<u64> {
        let next = match style {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(offset) => self.source.len().checked_add_signed(offset),
            SeekFrom::Current(offset) => self.pos.checked_add_signed(offset),
        }
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "negative seek"))?;
        self.pos = next;
        Ok(next)
    }
}
