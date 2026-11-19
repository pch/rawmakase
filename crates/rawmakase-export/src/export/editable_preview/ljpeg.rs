//! A lossless JPEG encoder (ITU T.81 process 14, "LJ92") for an editable
//! preview's tiles, which DNG stores as Compression 7.
//!
//! It writes what the Adobe DNG SDK's encoder (`dng_lossless_jpeg.cpp`)
//! writes for a tile that isn't a mosaic: the SOI marker, a SOF3 frame of
//! 16-bit precision with one component for each sample of a pixel (numbered
//! from 0, sampled 1 × 1) and the tile's own width and height, one DHT
//! segment per component, a scan of every component with predictor 1 and no
//! point transform, and EOI. Each component has a Huffman table of its own,
//! optimal for that tile's differences.
//!
//! Predictor 1 predicts each sample from the one to its left, of the same
//! component. As T.81 H.1.2.1 says, the first row has nothing above, the first
//! sample of every other row predicts from the sample above it, and the first
//! sample of the scan from 2^15. Differences are taken modulo 2^16, so a
//! difference of 32768 has length 16 and, as T.81 says, no bits after its
//! code; DNG files of version 1.1 and later are read that way.

/// The bits of a sample.
const PRECISION: u8 = 16;
/// The longest Huffman code JPEG allows.
const LONGEST: usize = 16;
/// The difference lengths (SSSS) a 16-bit sample's difference can have.
const LENGTHS: usize = 17;

/// The lossless JPEG of `samples`, `width × height` pixels of `components`
/// interleaved samples each.
pub(super) fn encode(samples: &[u16], width: u16, height: u16, components: usize) -> Vec<u8> {
    let (w, h) = (usize::from(width), usize::from(height));
    assert!(
        (1..=4).contains(&components) && w > 0 && h > 0 && samples.len() == w * h * components,
        "a lossless JPEG of {w} × {h} pixels of {components} samples, from {} samples",
        samples.len()
    );
    let row = w * components;
    // Every sample's difference from its prediction, in the order coded.
    let differences: Vec<i16> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let prediction = if i % row >= components {
                samples[i - components]
            } else if i >= row {
                samples[i - row]
            } else {
                1 << (PRECISION - 1)
            };
            s.wrapping_sub(prediction) as i16
        })
        .collect();
    let mut counts = vec![[0u32; LENGTHS]; components];
    for (i, &d) in differences.iter().enumerate() {
        counts[i % components][usize::from(length(d))] += 1;
    }
    let tables: Vec<Table> = counts.iter().map(Table::optimal).collect();

    let mut out = vec![0xff, 0xd8];
    let mut frame = vec![PRECISION];
    frame.extend(height.to_be_bytes());
    frame.extend(width.to_be_bytes());
    frame.push(components as u8);
    for c in 0..components as u8 {
        frame.extend([c, 0x11, 0]);
    }
    segment(&mut out, 0xc3, &frame);
    for (c, table) in tables.iter().enumerate() {
        let mut dht = vec![c as u8];
        dht.extend(table.bits);
        dht.extend(&table.values);
        segment(&mut out, 0xc4, &dht);
    }
    let mut scan = vec![components as u8];
    for c in 0..components as u8 {
        scan.extend([c, c << 4]);
    }
    // Predictor 1, the end of spectral selection (0) and no point transform.
    scan.extend([1, 0, 0]);
    segment(&mut out, 0xda, &scan);

    let mut bits = Bits {
        out,
        pending: 0,
        count: 0,
    };
    for (i, &d) in differences.iter().enumerate() {
        let table = &tables[i % components];
        let n = length(d);
        let (code, size) = table.codes[usize::from(n)];
        bits.put(u32::from(code), size);
        // A length of 16 has only one difference, 32768, and no bits for it.
        if (1..16).contains(&n) {
            // A negative difference is written as its value minus 1, in its
            // low `n` bits (T.81 F.1.2.1).
            let v = i32::from(d) - i32::from(d < 0);
            bits.put(v as u32 & ((1 << n) - 1), u32::from(n));
        }
    }
    let mut out = bits.finish();
    out.extend([0xff, 0xd9]);
    out
}

/// The bits a difference's value needs: SSSS, its Huffman-coded length.
fn length(d: i16) -> u8 {
    (16 - d.unsigned_abs().leading_zeros()) as u8
}

/// Appends a marker segment: the marker, then the length of `body` with its
/// own two bytes, then `body`.
fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) {
    out.extend([0xff, marker]);
    out.extend(((body.len() + 2) as u16).to_be_bytes());
    out.extend(body);
}

/// A Huffman table of difference lengths: what a DHT segment holds, and each
/// length's code.
struct Table {
    /// How many codes there are of each length, 1 to 16 bits (BITS).
    bits: [u8; LONGEST],
    /// The symbols in the order of their codes (HUFFVAL).
    values: Vec<u8>,
    /// Each length's code and its size in bits; 0 bits for a length that
    /// doesn't occur.
    codes: [(u16, u32); LENGTHS],
}

impl Table {
    /// The optimal table for `counts`, each length's number of differences,
    /// with no code longer than 16 bits, as T.81 K.2 and K.3 make it.
    fn optimal(counts: &[u32; LENGTHS]) -> Self {
        let sizes = Self::sizes(counts);
        // How many codes there are of each size.
        let mut bits = [0u32; 2 * LONGEST + 2];
        for &s in &sizes {
            if s > 0 {
                bits[s] += 1;
            }
        }
        // Codes longer than 16 bits are taken out two at a time: one takes
        // their prefix, a bit shorter, and a shorter code becomes the prefix
        // of the other and of a code that had that shorter code's length
        // (K.3's Adjust_BITS).
        for i in (LONGEST + 1..bits.len()).rev() {
            while bits[i] > 0 {
                let mut j = i - 2;
                while bits[j] == 0 {
                    j -= 1;
                }
                bits[i] -= 2;
                bits[i - 1] += 1;
                bits[j + 1] += 2;
                bits[j] -= 1;
            }
        }
        // The reserved symbol has one of the longest codes, which no symbol
        // takes now: it keeps any code from being all 1 bits.
        let longest = (1..=LONGEST).rev().find(|&i| bits[i] > 0).unwrap_or(1);
        bits[longest] -= 1;
        // The symbols in order of their unadjusted sizes take the codes in
        // order of length.
        let mut values: Vec<u8> = (0..LENGTHS as u8)
            .filter(|&s| sizes[usize::from(s)] > 0)
            .collect();
        values.sort_by_key(|&s| sizes[usize::from(s)]);
        let mut codes = [(0u16, 0u32); LENGTHS];
        let mut code = 0u32;
        let mut next = values.iter();
        for (len, &n) in bits[1..=LONGEST].iter().enumerate() {
            for _ in 0..n {
                let &symbol = next.next().expect("a symbol for every code");
                codes[usize::from(symbol)] = (code as u16, len as u32 + 1);
                code += 1;
            }
            code <<= 1;
        }
        Self {
            bits: std::array::from_fn(|i| bits[i + 1] as u8),
            values,
            codes,
        }
    }

    /// The size of each symbol's code in an optimal Huffman code without a
    /// length limit, 0 for a symbol that doesn't occur, with a reserved
    /// symbol counted once after the 17 lengths (T.81 K.2, Figure K.1).
    fn sizes(counts: &[u32; LENGTHS]) -> [usize; LENGTHS + 1] {
        let mut freq: Vec<u64> = counts.iter().map(|&c| u64::from(c)).collect();
        freq.push(1);
        let mut size = [0usize; LENGTHS + 1];
        // The next symbol in the same branch of the tree.
        let mut others = [None::<usize>; LENGTHS + 1];
        // The nonzero frequency least, then next least, the larger symbol on
        // a tie, as K.2 takes them.
        let least = |freq: &[u64], not: Option<usize>| {
            (0..freq.len())
                .filter(|&i| freq[i] > 0 && Some(i) != not)
                .min_by_key(|&i| (freq[i], std::cmp::Reverse(i)))
        };
        while let Some(v1) = least(&freq, None)
            && let Some(v2) = least(&freq, Some(v1))
        {
            freq[v1] += freq[v2];
            freq[v2] = 0;
            let mut v = Some(v1);
            let mut last = v1;
            while let Some(i) = v {
                size[i] += 1;
                last = i;
                v = others[i];
            }
            others[last] = Some(v2);
            let mut v = Some(v2);
            while let Some(i) = v {
                size[i] += 1;
                v = others[i];
            }
        }
        size
    }
}

/// Entropy-coded data, written most significant bit first, a 0 byte stuffed
/// after every 0xFF.
struct Bits {
    out: Vec<u8>,
    /// The bits not yet written, in the low `count` bits.
    pending: u64,
    count: u32,
}

impl Bits {
    /// Writes the low `n` bits of `v`, at most 16.
    fn put(&mut self, v: u32, n: u32) {
        self.pending = self.pending << n | u64::from(v);
        self.count += n;
        while self.count >= 8 {
            self.count -= 8;
            let b = (self.pending >> self.count) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
        }
    }

    /// The data, its last byte filled with 1 bits.
    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            let n = 8 - self.count;
            self.put((1 << n) - 1, n);
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes of a table's codes, 0 for a length with none.
    fn sizes(t: &Table) -> Vec<u32> {
        t.codes.iter().map(|c| c.1).collect()
    }

    /// A table whose codes would run longer than 16 bits unlimited, as counts
    /// growing like the Fibonacci numbers make them, is cut to 16 bits, and
    /// its codes stay a prefix code with none all 1 bits.
    #[test]
    fn long_codes_are_limited_to_16_bits() {
        let mut counts = [0u32; LENGTHS];
        let (mut a, mut b) = (1, 1);
        for c in counts.iter_mut().rev() {
            *c = a;
            (a, b) = (b, a + b);
        }
        let unlimited = Table::sizes(&counts);
        assert!(unlimited.iter().any(|&s| s > LONGEST), "{unlimited:?}");
        let t = Table::optimal(&counts);
        assert!(sizes(&t).iter().all(|&s| (1..=16).contains(&s)));
        assert_eq!(t.values.len(), LENGTHS);
        assert_eq!(
            t.bits.iter().map(|&n| usize::from(n)).sum::<usize>(),
            LENGTHS
        );
        // A prefix code with room left: the Kraft sum is below 1.
        let kraft: f64 = sizes(&t).iter().map(|&s| 0.5f64.powi(s as i32)).sum();
        assert!(kraft < 1., "{kraft}");
        for (i, &(code, size)) in t.codes.iter().enumerate() {
            assert_ne!(u32::from(code), (1 << size) - 1, "length {i}");
            for &(other, other_size) in &t.codes[i + 1..] {
                let n = size.min(other_size);
                assert_ne!(
                    u32::from(code) >> (size - n),
                    u32::from(other) >> (other_size - n)
                );
            }
        }
    }

    /// A tile whose differences all have one length codes that length with
    /// one bit, and the frequent lengths of a tile have the shorter codes.
    #[test]
    fn frequent_lengths_have_the_shorter_codes() {
        let mut counts = [0u32; LENGTHS];
        counts[0] = 1000;
        let t = Table::optimal(&counts);
        assert_eq!((t.bits[0], t.values.as_slice()), (1, &[0][..]));
        assert_eq!(t.codes[0], (0, 1));
        counts[3] = 400;
        counts[9] = 30;
        counts[16] = 1;
        let t = Table::optimal(&counts);
        assert!(t.codes[0].1 <= t.codes[3].1 && t.codes[3].1 <= t.codes[9].1);
        assert!(t.codes[9].1 <= t.codes[16].1);
    }

    /// The headers are Adobe's for a tile of three samples a pixel: SOF3 at
    /// 16 bits with the tile's size and three components 0, 1 and 2; a DHT
    /// for each; and a scan of the three, each with its own table, with
    /// predictor 1 and no point transform.
    #[test]
    fn the_headers_are_those_adobe_writes() {
        let samples: Vec<u16> = (0..5 * 2 * 3).map(|i| i * 1000).collect();
        let jpeg = encode(&samples, 5, 2, 3);
        assert_eq!(jpeg[..2], [0xff, 0xd8]);
        assert_eq!(
            jpeg[2..21],
            [
                0xff, 0xc3, 0, 17, 16, 0, 2, 0, 5, 3, 0, 0x11, 0, 1, 0x11, 0, 2, 0x11, 0
            ]
        );
        let sos = jpeg.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        assert_eq!(
            jpeg[sos..sos + 14],
            [0xff, 0xda, 0, 12, 3, 0, 0, 1, 0x10, 2, 0x20, 1, 0, 0]
        );
        let dhts: Vec<u8> = jpeg[..sos]
            .windows(2)
            .enumerate()
            .filter(|(_, w)| *w == [0xff, 0xc4])
            .map(|(i, _)| jpeg[i + 4])
            .collect();
        assert_eq!(dhts, [0, 1, 2]);
        assert_eq!(jpeg[jpeg.len() - 2..], [0xff, 0xd9]);
    }

    /// Each difference is predicted as T.81 says (the first row from the
    /// left, the first column from above, the first sample from 2^15), and a
    /// difference of 32768 is its length's code alone.
    #[test]
    fn differences_are_predicted_and_coded_as_t81_says() {
        // One component, 2 × 2: 32768 + 0, then +32768 (length 16) to the
        // right; below, 32768 + 1 from above and +3 from the left.
        let samples = [32768, 0, 32769, 32772];
        let jpeg = encode(&samples, 2, 2, 1);
        let sos = jpeg.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        let data = &jpeg[sos + 10..jpeg.len() - 2];
        let dht = jpeg.windows(2).position(|w| w == [0xff, 0xc4]).unwrap();
        let bits: [u8; 16] = jpeg[dht + 5..dht + 21].try_into().unwrap();
        let n: usize = bits.iter().map(|&b| usize::from(b)).sum();
        let values = &jpeg[dht + 21..dht + 21 + n];
        // Lengths 0, 16, 1 and 2, one each.
        let mut symbols = values.to_vec();
        symbols.sort_unstable();
        assert_eq!(symbols, [0, 1, 2, 16]);
        // The canonical codes of the table read from the file.
        let mut code = 0u32;
        let mut table = Vec::new();
        let mut v = values.iter();
        for (len, &count) in bits.iter().enumerate() {
            for _ in 0..count {
                table.push((*v.next().unwrap(), code, len as u32 + 1));
                code += 1;
            }
            code <<= 1;
        }
        let code_of = |s: u8| table.iter().find(|t| t.0 == s).map(|t| (t.1, t.2)).unwrap();
        let mut expected = Vec::new();
        for (s, extra) in [(0, None), (16, None), (1, Some((1, 1))), (2, Some((3, 2)))] {
            expected.push(code_of(s));
            if let Some(e) = extra {
                expected.push(e);
            }
        }
        let mut written = Bits {
            out: Vec::new(),
            pending: 0,
            count: 0,
        };
        for (v, n) in expected {
            written.put(v, n);
        }
        assert_eq!(data, written.finish());
    }

    /// 0xFF bytes of the entropy-coded data are followed by a stuffed 0, and
    /// the last byte is padded with 1 bits.
    #[test]
    fn ff_bytes_are_stuffed_and_the_end_padded_with_ones() {
        let mut bits = Bits {
            out: Vec::new(),
            pending: 0,
            count: 0,
        };
        bits.put(0xff, 8);
        bits.put(0b101, 3);
        assert_eq!(bits.finish(), [0xff, 0, 0b1011_1111]);
    }
}
