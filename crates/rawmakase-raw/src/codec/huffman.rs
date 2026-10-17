//! Canonical Huffman codes as JPEG defines them (ITU T.81 annex C): code
//! lengths 1 to 16 with the number of codes of each length, then the symbols
//! in code order. Decoded through a table indexed by the next 16 bits.
use super::bits::Msb;
use crate::error::{Result, corrupt};

pub(crate) struct Huffman {
    /// By the next 16 bits: the code's length (0 for none) and its symbol.
    table: Vec<(u8, u8)>,
}

impl Huffman {
    /// From the number of codes of each length 1 to 16 and the symbols.
    pub fn new(counts: &[u8; 16], symbols: &[u8]) -> Result<Self> {
        let total: usize = counts.iter().map(|&n| usize::from(n)).sum();
        if total > symbols.len() || total == 0 {
            return Err(corrupt("Huffman table"));
        }
        let mut table = vec![(0u8, 0u8); 1 << 16];
        let (mut code, mut k) = (0u32, 0usize);
        for (i, &n) in counts.iter().enumerate() {
            let len = i as u32 + 1;
            for _ in 0..n {
                if code >= 1 << len {
                    return Err(corrupt("Huffman table overfull"));
                }
                let shift = 16 - len;
                let start = (code << shift) as usize;
                for e in &mut table[start..start + (1 << shift)] {
                    *e = (len as u8, symbols[k]);
                }
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Ok(Self { table })
    }
    /// The code the next 16 bits start with: its length and symbol, for
    /// pumps other than [`Msb`].
    #[inline]
    pub fn lookup(&self, next16: u32) -> Result<(u32, u8)> {
        match self.table.get(next16 as usize) {
            Some(&(len, symbol)) if len > 0 => Ok((u32::from(len), symbol)),
            _ => Err(corrupt("invalid Huffman code")),
        }
    }
    /// The next symbol.
    #[inline]
    pub fn symbol(&self, bits: &mut Msb) -> Result<u8> {
        let (len, symbol) = self.table[bits.peek(16) as usize];
        if len == 0 {
            return Err(corrupt("invalid Huffman code"));
        }
        bits.skip(u32::from(len));
        Ok(symbol)
    }
    /// The next difference: a length symbol then that many bits, as JPEG's
    /// lossless and DC differences are coded (F.2.2.1). Length 16 is 32768
    /// with no bits following; with `sixteen`, the 16 bits some encoders
    /// write after it hold the difference.
    #[inline]
    pub fn difference(&self, bits: &mut Msb, sixteen: bool) -> Result<i32> {
        let len = u32::from(self.symbol(bits)?);
        Ok(match len {
            0 => 0,
            16 => {
                if sixteen {
                    return Ok(extend(bits.get(16), 16));
                }
                32768
            }
            1..=15 => extend(bits.get(len), len),
            _ => return Err(corrupt("difference length")),
        })
    }
}

/// A `len`-bit difference read as JPEG codes it: values below half the range
/// are negative.
#[inline]
pub(crate) fn extend(v: u32, len: u32) -> i32 {
    if len == 0 {
        0
    } else if v < 1 << (len - 1) {
        v as i32 - (1 << len) + 1
    } else {
        v as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_canonical_codes() {
        // Lengths: one code of 2 bits (00 → 5), two of 3 bits (010 → 6, 011 → 7).
        let mut counts = [0u8; 16];
        counts[1] = 1;
        counts[2] = 2;
        let h = Huffman::new(&counts, &[5, 6, 7]).unwrap();
        let mut bits = Msb::new(&[0b00_010_011, 0b1100_0000]);
        assert_eq!(h.symbol(&mut bits).unwrap(), 5);
        assert_eq!(h.symbol(&mut bits).unwrap(), 6);
        assert_eq!(h.symbol(&mut bits).unwrap(), 7);
        assert!(h.symbol(&mut bits).is_err());
    }
    #[test]
    fn extends_differences() {
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        assert_eq!(extend(0b010, 3), -5);
        assert_eq!(extend(0b110, 3), 6);
    }
}
