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
            .ok_or_else(|| past_end(at, len as u64))
    }
    /// The 16-bit value at `at`.
    pub(crate) fn u16(&self, at: u64) -> Result<u16> {
        let b = self.slice(at, 2)?;
        let b = [b[0], b[1]];
        Ok(match self.order {
            Order::Little => u16::from_le_bytes(b),
            Order::Big => u16::from_be_bytes(b),
        })
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

/// The error of a read of `len` bytes at `at` that the file doesn't hold.
fn past_end(at: u64, len: u64) -> Error {
    corrupt(format!("{len} bytes at {at} past the end of the file"))
}

/// The file's bytes as the header, container and preview readers see them,
/// with bounds-checked reads at file offsets. Every read past the end is
/// `None` (or [`Self::slice`]'s error), never a panic.
///
/// The file is held whole in memory: the bytes a reader asks for are
/// borrowed from it. Readers take a `Source` rather than a slice so that
/// what holds the bytes can change without changing them.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source<'a> {
    /// The file's bytes, from its first on.
    Slice(&'a [u8]),
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
    /// The file's length in bytes.
    pub(crate) fn len(self) -> u64 {
        match self {
            Self::Slice(s) => s.len() as u64,
        }
    }

    /// The bytes from `at` to the end, their offsets counted from `at`;
    /// `None` past the end.
    pub(crate) fn slice_from(self, at: u64) -> Option<Self> {
        let len = self.len().checked_sub(at)?;
        self.slice_exact(at, len)
    }

    /// The `len` bytes at `at`, their offsets counted from `at`; `None`
    /// unless all of them are in the file.
    pub(crate) fn slice_exact(self, at: u64, len: u64) -> Option<Self> {
        match self {
            Self::Slice(s) => range(s, at, len).map(Self::Slice),
        }
    }

    /// The byte at `at`.
    pub(crate) fn byte(self, at: u64) -> Option<u8> {
        match self {
            Self::Slice(s) => usize::try_from(at).ok().and_then(|i| s.get(i).copied()),
        }
    }

    /// The `N` bytes at `at`.
    pub(crate) fn array<const N: usize>(self, at: u64) -> Option<[u8; N]> {
        match self {
            Self::Slice(s) => range(s, at, N as u64)?.try_into().ok(),
        }
    }

    /// The `len` bytes at `at`; `None` unless all of them are in the file.
    pub(crate) fn get(self, at: u64, len: u64) -> Option<Cow<'a, [u8]>> {
        match self {
            Self::Slice(s) => range(s, at, len).map(Cow::Borrowed),
        }
    }

    /// As [`Self::get`], with [`Bytes::slice`]'s error.
    pub(crate) fn slice(self, at: u64, len: u64) -> Result<Cow<'a, [u8]>> {
        match self {
            Self::Slice(s) => {
                let start = usize::try_from(at).map_err(|_| corrupt("offset"))?;
                let count = usize::try_from(len).map_err(|_| corrupt("length"))?;
                start
                    .checked_add(count)
                    .and_then(|end| s.get(start..end))
                    .map(Cow::Borrowed)
                    .ok_or_else(|| past_end(at, len))
            }
        }
    }

    /// The 16-bit value at `at`, in `order`.
    pub(crate) fn u16(self, order: Order, at: u64) -> Option<u16> {
        let b = self.array::<2>(at)?;
        Some(match order {
            Order::Little => u16::from_le_bytes(b),
            Order::Big => u16::from_be_bytes(b),
        })
    }

    /// The 32-bit value at `at`, in `order`.
    pub(crate) fn u32(self, order: Order, at: u64) -> Option<u32> {
        let b = self.array::<4>(at)?;
        Some(match order {
            Order::Little => u32::from_le_bytes(b),
            Order::Big => u32::from_be_bytes(b),
        })
    }

    /// As [`Self::array`], with [`Bytes::slice`]'s error.
    pub(crate) fn read_array<const N: usize>(self, at: u64) -> Result<[u8; N]> {
        let bytes = self.slice(at, N as u64)?;
        Ok(bytes
            .as_ref()
            .try_into()
            .expect("slice returned the requested length"))
    }

    /// As [`Self::u32`], with [`Bytes::u32`]'s error.
    pub(crate) fn read_u32(self, order: Order, at: u64) -> Result<u32> {
        let b = self.read_array::<4>(at)?;
        Ok(match order {
            Order::Little => u32::from_le_bytes(b),
            Order::Big => u32::from_be_bytes(b),
        })
    }

    /// The 64-bit value at `at`, in `order`.
    pub(crate) fn u64(self, order: Order, at: u64) -> Option<u64> {
        let b = self.array::<8>(at)?;
        Some(match order {
            Order::Little => u64::from_le_bytes(b),
            Order::Big => u64::from_be_bytes(b),
        })
    }

    /// Whether the file starts with `prefix`.
    pub(crate) fn starts_with(self, prefix: &[u8]) -> bool {
        self.starts_with_at(0, prefix)
    }

    /// Whether the bytes at `at` start with `prefix`: `false` when the file
    /// ends sooner.
    pub(crate) fn starts_with_at(self, at: u64, prefix: &[u8]) -> bool {
        match self {
            Self::Slice(s) => usize::try_from(at)
                .ok()
                .and_then(|i| s.get(i..))
                .is_some_and(|rest| rest.starts_with(prefix)),
        }
    }

    /// Where the first `needle` is from `from` on, before `to` or the
    /// file's end; `None` when there is none or `from` is past them.
    pub(crate) fn find_byte(self, from: u64, to: u64, needle: u8) -> Option<u64> {
        match self {
            Self::Slice(s) => {
                let start = usize::try_from(from).ok()?;
                let end = usize::try_from(to.min(s.len() as u64)).ok()?;
                s.get(start..end)?
                    .iter()
                    .position(|&b| b == needle)
                    .map(|p| from + p as u64)
            }
        }
    }

    /// A reader of the file from its start, for readers that take
    /// [`Read`] and [`Seek`].
    pub(crate) fn reader(self) -> SourceReader<'a> {
        SourceReader {
            source: self,
            pos: 0,
        }
    }
}

/// The `len` bytes at `at` in `s`, when they are all there.
fn range(s: &[u8], at: u64, len: u64) -> Option<&[u8]> {
    let start = usize::try_from(at).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    s.get(start..end)
}

/// A [`Read`] and [`Seek`] cursor over a [`Source`], as
/// [`std::io::Cursor`] is over a slice: a seek past the end is allowed, and
/// reads there give no bytes.
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
        // At most the buffer's length, so it fits a `usize`.
        let avail = (len - self.pos).min(buf.len() as u64) as usize;
        match self.source {
            Source::Slice(s) => {
                let start = usize::try_from(self.pos)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
                buf[..avail].copy_from_slice(&s[start..start + avail]);
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
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )
        })?;
        self.pos = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every read is bounds-checked at the file's offsets: whole within the
    /// file it gives the bytes, crossing or past the end it gives none, and
    /// an offset that overflows gives none rather than panicking.
    #[test]
    fn source_reads_are_bounded() {
        let file: Vec<u8> = (0..16).collect();
        let s = Source::from(&file);
        assert_eq!(s.len(), 16);
        assert_eq!(s.byte(15), Some(15));
        assert_eq!(s.byte(16), None);
        assert_eq!(s.array::<4>(12), Some([12, 13, 14, 15]));
        assert_eq!(s.array::<4>(13), None);
        assert_eq!(s.array::<4>(u64::MAX), None);
        assert_eq!(s.get(14, 2).as_deref(), Some(&[14, 15][..]));
        assert_eq!(s.get(14, 3), None);
        assert_eq!(s.get(16, 0).as_deref(), Some(&[][..]));
        assert_eq!(s.get(17, 0), None);
        assert_eq!(s.get(1, u64::MAX), None);
        assert!(matches!(s.get(3, 2), Some(Cow::Borrowed(&[3, 4]))));
        assert_eq!(s.u16(Order::Little, 2), Some(0x0302));
        assert_eq!(s.u16(Order::Big, 2), Some(0x0203));
        assert_eq!(s.u32(Order::Little, 4), Some(0x0706_0504));
        assert_eq!(s.u32(Order::Big, 4), Some(0x0405_0607));
        assert_eq!(s.u64(Order::Big, 8), Some(0x0809_0a0b_0c0d_0e0f));
        assert_eq!(s.u16(Order::Big, 15), None);
        assert_eq!(s.u32(Order::Big, 13), None);
        assert_eq!(s.u64(Order::Big, 9), None);
        assert!(s.starts_with(&[0, 1]));
        assert!(s.starts_with_at(14, &[14, 15]));
        assert!(s.starts_with_at(16, &[]));
        assert!(!s.starts_with_at(15, &[15, 16]));
        assert!(!s.starts_with_at(17, &[]));
        assert_eq!(s.find_byte(3, 16, 9), Some(9));
        assert_eq!(s.find_byte(3, 9, 9), None);
        assert_eq!(s.find_byte(3, u64::MAX, 15), Some(15));
        assert_eq!(s.find_byte(17, 20, 0), None);
    }

    /// The `Result` reads fail as [`Bytes`]'s do, with the same message.
    #[test]
    fn source_errors_are_the_bytes_errors() {
        let file = [1u8, 2, 3, 4, 5];
        let s = Source::from(&file);
        let b = Bytes::new(&file, Order::Big);
        assert_eq!(s.slice(1, 4).ok().as_deref(), b.slice(1, 4).ok());
        assert_eq!(
            s.slice(2, 4).unwrap_err().to_string(),
            b.slice(2, 4).unwrap_err().to_string()
        );
        assert_eq!(s.read_u32(Order::Big, 1).ok(), b.u32(1).ok());
        assert_eq!(
            s.read_u32(Order::Big, 2).unwrap_err().to_string(),
            b.u32(2).unwrap_err().to_string()
        );
        assert_eq!(s.read_array::<2>(3).ok(), Some([4, 5]));
        assert_eq!(
            s.read_array::<2>(4).unwrap_err().to_string(),
            b.slice(4, 2).unwrap_err().to_string()
        );
        // Offsets and lengths beyond `usize` (on 32-bit targets) or beyond
        // the end fail with the same error as [`Bytes`] gives.
        for (at, len) in [(u64::MAX, 1), (0, u64::MAX), (u64::MAX, u64::MAX)] {
            assert_eq!(
                s.slice(at, len).unwrap_err().to_string(),
                b.slice(at, len).unwrap_err().to_string()
            );
        }
        assert_eq!(
            s.read_u32(Order::Big, u64::MAX).unwrap_err().to_string(),
            b.u32(u64::MAX).unwrap_err().to_string()
        );
        assert_eq!(
            s.read_array::<2>(u64::MAX).unwrap_err().to_string(),
            b.slice(u64::MAX, 2).unwrap_err().to_string()
        );
    }

    /// A subrange counts its offsets from its start and reads no further
    /// than its end, though the file goes on.
    #[test]
    fn subranges_count_from_their_start() {
        let file: Vec<u8> = (0..16).collect();
        let s = Source::from(&file);
        let sub = s.slice_exact(4, 8).unwrap();
        assert_eq!(sub.len(), 8);
        assert_eq!(sub.byte(0), Some(4));
        assert_eq!(sub.byte(8), None);
        assert_eq!(sub.u16(Order::Big, 6), Some(0x0a0b));
        assert_eq!(sub.u16(Order::Big, 7), None);
        assert_eq!(sub.find_byte(0, u64::MAX, 12), None);
        let tail = s.slice_from(10).unwrap();
        assert_eq!(tail.len(), 6);
        assert_eq!(tail.get(0, 6).as_deref(), Some(&file[10..]));
        assert_eq!(s.slice_from(16).map(Source::len), Some(0));
        assert!(s.slice_from(17).is_none());
        assert!(s.slice_exact(12, 5).is_none());
        assert!(s.slice_exact(u64::MAX, 2).is_none());
    }

    /// The reader reads and seeks as a [`std::io::Cursor`] over the same
    /// bytes does, past the end too, so the TIFF reader sees no change.
    #[test]
    fn the_reader_matches_a_cursor() {
        let file: Vec<u8> = (0..10).collect();
        let mut ours = Source::from(&file).reader();
        let mut cursor = std::io::Cursor::new(&file[..]);
        let steps = [
            SeekFrom::Start(3),
            SeekFrom::Current(2),
            SeekFrom::End(-1),
            SeekFrom::Start(9),
            SeekFrom::Start(12),
            SeekFrom::End(4),
            SeekFrom::Current(-20),
            SeekFrom::Start(0),
        ];
        for step in steps {
            assert_eq!(ours.seek(step).ok(), cursor.seek(step).ok(), "{step:?}");
            let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
            assert_eq!(ours.read(&mut a).ok(), cursor.read(&mut b).ok(), "{step:?}");
            assert_eq!(a, b, "{step:?}");
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        ours.seek(SeekFrom::Start(1)).unwrap();
        cursor.seek(SeekFrom::Start(1)).unwrap();
        ours.read_to_end(&mut a).unwrap();
        cursor.read_to_end(&mut b).unwrap();
        assert_eq!(a, b);
    }
}
