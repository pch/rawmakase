//! Bounded reading of the file's bytes, in memory or in pieces from the
//! file, and the allocation limits every decoder checks header sizes
//! against before allocating.
use crate::error::{Error, Result, corrupt};
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

/// Byte order.
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
