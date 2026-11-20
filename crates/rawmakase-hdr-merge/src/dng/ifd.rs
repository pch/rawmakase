//! A little-endian TIFF built in memory: data blocks first, then each
//! directory with the values that do not fit in its entries. Offsets are 32
//! bits, so a file is at most 4 GiB ([`crate::Error::TooLarge`]).
use crate::Error;
use std::collections::BTreeMap;

/// TIFF field types (TIFF 6, section 2).
pub(super) mod kind {
    pub(crate) const BYTE: u16 = 1;
    pub(crate) const ASCII: u16 = 2;
    pub(crate) const SHORT: u16 = 3;
    pub(crate) const LONG: u16 = 4;
    pub(crate) const RATIONAL: u16 = 5;
    pub(crate) const SRATIONAL: u16 = 10;
}

/// A field's type, count and value bytes, little-endian.
#[derive(Clone, Debug)]
pub(super) struct Value {
    pub kind: u16,
    pub count: u32,
    pub bytes: Vec<u8>,
}

/// The denominator rationals written from reals take: six decimal places.
const DENOMINATOR: u32 = 1_000_000;

impl Value {
    pub(super) fn bytes(kind: u16, bytes: Vec<u8>) -> Self {
        Self {
            kind,
            count: bytes.len() as u32,
            bytes,
        }
    }
    /// ASCII text, NUL-terminated.
    pub(super) fn ascii(text: &str) -> Self {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        Self::bytes(kind::ASCII, bytes)
    }
    pub(super) fn shorts(values: &[u16]) -> Self {
        Self {
            kind: kind::SHORT,
            count: values.len() as u32,
            bytes: values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
    }
    pub(super) fn longs(values: &[u32]) -> Self {
        Self {
            kind: kind::LONG,
            count: values.len() as u32,
            bytes: values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
    }
    /// Unsigned rationals of `values`, to six decimal places; negative and
    /// non-finite values are 0.
    pub(super) fn rationals(values: &[f64]) -> Self {
        let num = |v: f64| {
            let n = (v * f64::from(DENOMINATOR)).round();
            if n.is_finite() && n > 0. {
                n.min(f64::from(u32::MAX)) as u32
            } else {
                0
            }
        };
        Self {
            kind: kind::RATIONAL,
            count: values.len() as u32,
            bytes: values
                .iter()
                .flat_map(|&v| [num(v).to_le_bytes(), DENOMINATOR.to_le_bytes()].concat())
                .collect(),
        }
    }
    /// Signed rationals of `values`, to six decimal places, clamped to what
    /// they can hold (±2147); non-finite values are 0.
    pub(super) fn srationals(values: &[f64]) -> Self {
        let num = |v: f64| {
            let n = (v * f64::from(DENOMINATOR)).round();
            if n.is_finite() {
                n.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
            } else {
                0
            }
        };
        Self {
            kind: kind::SRATIONAL,
            count: values.len() as u32,
            bytes: values
                .iter()
                .flat_map(|&v| [num(v).to_le_bytes(), (DENOMINATOR as i32).to_le_bytes()].concat())
                .collect(),
        }
    }
}

/// A directory's entries by tag: written in ascending order, as TIFF requires.
#[derive(Clone, Debug, Default)]
pub(super) struct Ifd(pub BTreeMap<u16, Value>);

impl Ifd {
    pub(super) fn set(&mut self, tag: u16, value: Value) {
        self.0.insert(tag, value);
    }
}

/// The file being built: the header, then blocks and directories as they are
/// added; or, from [`Tiff::after`], what follows a file's first part.
pub(super) struct Tiff {
    /// Where `buf` starts in the file.
    base: usize,
    buf: Vec<u8>,
}

impl Tiff {
    pub(super) fn new() -> Self {
        // IFD0's offset is set by `finish`.
        Self {
            base: 0,
            buf: b"II*\0\0\0\0\0".to_vec(),
        }
    }
    /// The blocks and directories that follow `len` bytes of a file.
    pub(super) fn after(len: usize) -> Self {
        Self {
            base: len,
            buf: Vec::new(),
        }
    }
    /// The file's length so far.
    fn len(&self) -> Result<u32, Error> {
        u32::try_from(self.base + self.buf.len()).map_err(|_| Error::TooLarge)
    }
    /// The offset the next block starts at, on a word boundary.
    fn align(&mut self) -> Result<u32, Error> {
        if (self.base + self.buf.len()) % 2 == 1 {
            self.buf.push(0);
        }
        self.len()
    }
    /// Adds `bytes` and returns their offset.
    pub(super) fn block(&mut self, bytes: &[u8]) -> Result<u32, Error> {
        let at = self.align()?;
        self.buf.extend_from_slice(bytes);
        self.len()?;
        Ok(at)
    }
    /// Adds a directory (its long values first) and returns its offset; its
    /// next-directory offset is 0.
    pub(super) fn directory(&mut self, ifd: &Ifd) -> Result<u32, Error> {
        let mut inline = Vec::with_capacity(ifd.0.len());
        for (&tag, value) in &ifd.0 {
            let field = if value.bytes.len() <= 4 {
                let mut v = value.bytes.clone();
                v.resize(4, 0);
                [v[0], v[1], v[2], v[3]]
            } else {
                self.block(&value.bytes)?.to_le_bytes()
            };
            inline.push((tag, value.kind, value.count, field));
        }
        let at = self.align()?;
        let count = u16::try_from(inline.len()).map_err(|_| Error::TooLarge)?;
        self.buf.extend(count.to_le_bytes());
        for (tag, kind, count, field) in inline {
            self.buf.extend(tag.to_le_bytes());
            self.buf.extend(kind.to_le_bytes());
            self.buf.extend(count.to_le_bytes());
            self.buf.extend(field);
        }
        self.buf.extend(0u32.to_le_bytes());
        self.len()?;
        Ok(at)
    }
    /// The bytes, the header's offset of IFD0 left 0 for the file's writer
    /// to set.
    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}
