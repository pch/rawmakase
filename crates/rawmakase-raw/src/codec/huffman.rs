//! Canonical Huffman codes as JPEG defines them (ITU T.81 annex C): code
//! lengths 1 to 16 with the number of codes of each length, then the symbols
//! in code order. Decoded with a 10-bit lookup table and canonical bounds for
//! longer codes.
use super::bits::Msb;
use crate::error::{Result, corrupt};

const FAST_BITS: u32 = 10;
const FAST_SIZE: usize = 1 << FAST_BITS;
const FAST_SHIFT: u32 = 16 - FAST_BITS;
const SLOW_LENS: usize = 16 - FAST_BITS as usize;

#[derive(PartialEq)]
pub(crate) struct Huffman {
    /// By the top [`FAST_BITS`] bits: the code's length (`1..=FAST_BITS`, or 0
    /// for longer or invalid codes) and its symbol.
    fast: [(u8, u8); FAST_SIZE],
    /// For lengths 11 to 16, one past the largest 16-bit left-aligned code of
    /// that length (0 when no codes of that length exist).
    end16: [u32; SLOW_LENS],
    /// `first_symbol_index - first_code` for lengths 11 to 16.
    val_offset: [i32; SLOW_LENS],
    /// Symbols of lengths 11 to 16 in canonical code order.
    symbols: Vec<u8>,
}

impl Huffman {
    /// From the number of codes of each length 1 to 16 and the symbols.
    pub(crate) fn new(counts: &[u8; 16], symbols: &[u8]) -> Result<Self> {
        let total: usize = counts.iter().map(|&n| usize::from(n)).sum();
        if total > symbols.len() || total == 0 {
            return Err(corrupt("Huffman table"));
        }
        let mut fast = [(0u8, 0u8); FAST_SIZE];
        let mut end16 = [0u32; SLOW_LENS];
        let mut val_offset = [0i32; SLOW_LENS];
        let slow_count: usize = counts[FAST_BITS as usize..]
            .iter()
            .map(|&n| usize::from(n))
            .sum();
        let mut slow_symbols = Vec::with_capacity(slow_count);
        let (mut code, mut k) = (0u32, 0usize);
        for (i, &n) in counts.iter().enumerate() {
            let len = i as u32 + 1;
            if len <= FAST_BITS {
                let shift = FAST_BITS - len;
                let span = 1usize << shift;
                for _ in 0..n {
                    if code >= 1 << len {
                        return Err(corrupt("Huffman table overfull"));
                    }
                    let start = (code as usize) << shift;
                    fast[start..start + span].fill((len as u8, symbols[k]));
                    code += 1;
                    k += 1;
                }
            } else {
                let first_code = code;
                let first_sym = slow_symbols.len();
                for _ in 0..n {
                    if code >= 1 << len {
                        return Err(corrupt("Huffman table overfull"));
                    }
                    slow_symbols.push(symbols[k]);
                    code += 1;
                    k += 1;
                }
                if n > 0 {
                    let s = i - FAST_BITS as usize;
                    end16[s] = code << (16 - len);
                    val_offset[s] = first_sym as i32 - first_code as i32;
                }
            }
            code <<= 1;
        }
        Ok(Self {
            fast,
            end16,
            val_offset,
            symbols: slow_symbols,
        })
    }
    #[inline]
    fn lookup_slow(&self, next16: u32) -> Result<(u32, u8)> {
        for i in 0..SLOW_LENS {
            if next16 < self.end16[i] {
                let len = FAST_BITS + 1 + i as u32;
                let code = (next16 >> (16 - len)) as i32;
                let idx = (code + self.val_offset[i]) as usize;
                return Ok((len, self.symbols[idx]));
            }
        }
        Err(corrupt("invalid Huffman code"))
    }
    /// The code the next 16 bits start with: its length and symbol, for
    /// pumps other than [`Msb`].
    #[inline]
    pub(crate) fn lookup(&self, next16: u32) -> Result<(u32, u8)> {
        if next16 >= 1 << 16 {
            return Err(corrupt("invalid Huffman code"));
        }
        let (len, symbol) = self.fast[(next16 >> FAST_SHIFT) as usize];
        if len > 0 {
            return Ok((u32::from(len), symbol));
        }
        self.lookup_slow(next16)
    }
    /// The next symbol.
    #[inline]
    pub(crate) fn symbol(&self, bits: &mut Msb) -> Result<u8> {
        let next16 = bits.peek(16);
        let (len, symbol) = self.fast[(next16 >> FAST_SHIFT) as usize];
        if len > 0 {
            bits.skip(u32::from(len));
            return Ok(symbol);
        }
        let (len, symbol) = self.lookup_slow(next16)?;
        bits.skip(len);
        Ok(symbol)
    }
    /// The next difference: a length symbol then that many bits, as JPEG's
    /// lossless and DC differences are coded (F.2.2.1). Length 16 is 32768
    /// with no bits following; with `sixteen`, the 16 bits some encoders
    /// write after it hold the difference.
    #[inline]
    pub(crate) fn difference(&self, bits: &mut Msb, sixteen: bool) -> Result<i32> {
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
    fn decodes_codes_longer_than_fast_table() {
        // Code lengths 11..=16 exercise the canonical slow path beyond FAST_BITS=10.
        let mut counts = [0u8; 16];
        counts[0] = 1; // 1-bit: 0 -> sym 10
        counts[9] = 1; // 10-bit: 1000000000 -> sym 11
        counts[10] = 2; // 11-bit -> syms 12, 13
        counts[11] = 2; // 12-bit -> syms 14, 15
        counts[13] = 2; // 14-bit -> syms 16, 17
        counts[15] = 2; // 16-bit -> syms 18, 19
        let syms = [10u8, 11, 12, 13, 14, 15, 16, 17, 18, 19];
        let h = Huffman::new(&counts, &syms).unwrap();

        let mut expected = Vec::new();
        let mut code = 0u32;
        let mut k = 0usize;
        for (i, &n) in counts.iter().enumerate() {
            let len = i as u32 + 1;
            for _ in 0..n {
                expected.push((len, syms[k], code << (16 - len)));
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        for (len, sym, left16) in &expected {
            assert_eq!(h.lookup(*left16).unwrap(), (*len, *sym));
            if *len < 16 {
                let tail = (1u32 << (16 - *len)) - 1;
                assert_eq!(h.lookup(*left16 | tail).unwrap(), (*len, *sym));
            }
        }
        assert!(h.lookup(0xffff).is_err());
        assert!(h.lookup(1 << 16).is_err());

        // Stream symbols of lengths 11, 16, 1, 14 in one bit buffer.
        let mut stream = 0u64;
        let mut used = 0u32;
        let seq = [expected[2], expected[9], expected[0], expected[6]];
        for (len, _, left16) in seq {
            let raw = u64::from(left16 >> (16 - len));
            used += len;
            stream |= raw << (64 - used);
        }
        let bytes = stream.to_be_bytes();
        let mut bits = Msb::new(&bytes);
        for (_, sym, _) in seq {
            assert_eq!(h.symbol(&mut bits).unwrap(), sym);
        }
    }
    #[test]
    fn matches_full_16bit_table_exhaustively() {
        fn reference(counts: &[u8; 16], symbols: &[u8]) -> Result<Vec<(u8, u8)>> {
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
                    table[start..start + (1 << shift)].fill((len as u8, symbols[k]));
                    code += 1;
                    k += 1;
                }
                code <<= 1;
            }
            Ok(table)
        }

        let cases: [([u8; 16], Vec<u8>); 5] = [
            // Fast-only table (lengths <= 10).
            (
                [0, 1, 2, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0],
                (10..17).collect(),
            ),
            // Slow-only table (lengths 11..=16, fast table completely empty).
            (
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 2, 0, 4, 1, 5],
                (20..35).collect(),
            ),
            // Mixed fast and slow codes spanning the 10/11 boundary.
            (
                [1, 0, 1, 0, 0, 0, 0, 0, 0, 2, 2, 1, 3, 0, 2, 4],
                (40..56).collect(),
            ),
            // Complete tree reaching 0xffff with a 16-bit code.
            (
                [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2],
                (60..77).collect(),
            ),
            // ITU T.81 Annex K luminance AC-like distribution.
            (
                [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125],
                (0u8..162).collect(),
            ),
        ];

        for (counts, syms) in cases {
            let h = Huffman::new(&counts, &syms).unwrap();
            let full = reference(&counts, &syms).unwrap();
            for next16 in 0..(1u32 << 16) {
                let (exp_len, exp_sym) = full[next16 as usize];
                match h.lookup(next16) {
                    Ok((len, sym)) => {
                        assert_eq!((len as u8, sym), (exp_len, exp_sym), "next16={next16:#06x}");
                        assert!(exp_len > 0);
                    }
                    Err(_) => assert_eq!(exp_len, 0, "next16={next16:#06x}"),
                }
            }
            assert!(h.lookup(1 << 16).is_err());
            assert!(h.lookup(u32::MAX).is_err());
            assert!(h == Huffman::new(&counts, &syms).unwrap());
        }
    }
    #[test]
    fn rejects_empty_truncated_and_overfull_tables() {
        // Empty table.
        assert!(Huffman::new(&[0u8; 16], &[]).is_err());
        // Fewer symbols than counts demand.
        let mut counts = [0u8; 16];
        counts[1] = 2;
        assert!(Huffman::new(&counts, &[1]).is_err());
        // Overfull inside the fast table (3 codes of length 1).
        let mut over_fast = [0u8; 16];
        over_fast[0] = 3;
        assert!(Huffman::new(&over_fast, &[0, 1, 2]).is_err());
        // Complete in the fast table, then an extra code at length 11.
        let mut over_boundary = [0u8; 16];
        over_boundary[0] = 2;
        over_boundary[10] = 1;
        assert!(Huffman::new(&over_boundary, &[0, 1, 2]).is_err());
        // Complete up to length 16, then one code too many at length 16.
        let over_slow = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3];
        let syms: Vec<u8> = (0..18).collect();
        assert!(Huffman::new(&over_slow, &syms).is_err());
    }
    #[test]
    fn extends_differences() {
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        assert_eq!(extend(0b010, 3), -5);
        assert_eq!(extend(0b110, 3), 6);
    }
}
