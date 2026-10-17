//! Bit pumps: reading a byte stream as bits, most significant first (JPEG,
//! with or without its 0xFF00 byte stuffing) or least significant first. Past
//! the end of the data a pump reads zeros, as truncated files are common and
//! decoders fill what they cannot read; callers that must know check
//! [`Msb::overrun`].

/// Most significant bit first.
pub(crate) struct Msb<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bits not yet consumed, left aligned.
    cache: u64,
    bits: u32,
    /// JPEG entropy-coded data: 0xFF is followed by a stuffed 0x00, and any
    /// other byte after 0xFF is a marker, where the data ends.
    jpeg: bool,
    /// A JPEG marker was reached; only zeros follow.
    marker: bool,
    /// Bytes of zeros fetched past the end.
    past: usize,
    /// Whether bits past the end of an earlier restart interval were consumed.
    overran: bool,
}

impl<'a> Msb<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            cache: 0,
            bits: 0,
            jpeg: false,
            marker: false,
            past: 0,
            overran: false,
        }
    }
    /// A JPEG entropy-coded segment.
    pub fn jpeg(data: &'a [u8]) -> Self {
        Self {
            jpeg: true,
            ..Self::new(data)
        }
    }
    /// The bits of the data (up to a JPEG marker) not yet consumed.
    pub fn unread(&self) -> u64 {
        let cached = u64::from(self.bits).saturating_sub(self.past as u64 * 8);
        let mut rest = 0u64;
        if !self.marker {
            let mut i = self.pos;
            while i < self.data.len() {
                if self.jpeg && self.data[i] == 0xff {
                    match self.data.get(i + 1) {
                        Some(0) => i += 1,
                        Some(_) => break,
                        None => {}
                    }
                }
                rest += 8;
                i += 1;
            }
        }
        cached + rest
    }
    /// Whether bits past the end of the data (or a JPEG marker) were
    /// consumed, in this restart interval or an earlier one: the stream was
    /// truncated.
    pub fn overrun(&self) -> bool {
        self.overran || self.past as u64 * 8 > u64::from(self.bits)
    }
    /// The bits consumed so far, those past the end included (a pump not
    /// reading JPEG).
    pub fn consumed(&self) -> u64 {
        (self.pos + self.past) as u64 * 8 - u64::from(self.bits)
    }
    #[inline]
    fn next_byte(&mut self) -> u8 {
        if self.marker || self.pos >= self.data.len() {
            self.past += 1;
            return 0;
        }
        let b = self.data[self.pos];
        self.pos += 1;
        if self.jpeg && b == 0xff {
            match self.data.get(self.pos) {
                Some(0) => self.pos += 1,
                Some(_) => {
                    // A marker: keep it unread.
                    self.pos -= 1;
                    self.marker = true;
                    self.past += 1;
                    return 0;
                }
                None => {}
            }
        }
        b
    }
    #[inline]
    fn refill(&mut self) {
        if !self.jpeg {
            // Fast path: whole bytes straight from the data.
            while self.bits <= 56 {
                let b = if self.pos < self.data.len() {
                    let b = self.data[self.pos];
                    self.pos += 1;
                    b
                } else {
                    self.past += 1;
                    0
                };
                self.cache |= u64::from(b) << (56 - self.bits);
                self.bits += 8;
            }
            return;
        }
        while self.bits <= 56 {
            let b = self.next_byte();
            self.cache |= u64::from(b) << (56 - self.bits);
            self.bits += 8;
        }
    }
    /// The next `n` bits (at most 32) without consuming them.
    #[inline]
    pub fn peek(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        if self.bits < n {
            self.refill();
        }
        (self.cache >> (64 - n)) as u32
    }
    #[inline]
    pub fn skip(&mut self, n: u32) {
        if self.bits < n {
            self.refill();
        }
        self.cache <<= n;
        self.bits -= n;
    }
    /// The next `n` bits (at most 32).
    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n);
        v
    }
    /// Restarts after a JPEG restart marker: drops the cached bits and skips
    /// the marker (0xFFDn) at the current position, if there is one.
    pub fn restart(&mut self) {
        // A pump fetches ahead, so it has normally stopped at the marker, and
        // what is cached is the segment's padding.
        self.overran = self.overrun();
        self.cache = 0;
        self.bits = 0;
        self.past = 0;
        self.marker = false;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xff && (0xd0..=0xd7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                break;
            }
            self.pos += 1;
        }
    }
}

/// Most significant bit first from 32-bit words of a byte order (Phase One's
/// and Hasselblad's codings); zeros past the end.
pub(crate) struct Words<'a> {
    data: &'a [u8],
    order: crate::io::Order,
    pos: usize,
    /// Bits not yet consumed, left aligned.
    cache: u64,
    bits: u32,
}

impl<'a> Words<'a> {
    pub fn new(data: &'a [u8], order: crate::io::Order) -> Self {
        Self {
            data,
            order,
            pos: 0,
            cache: 0,
            bits: 0,
        }
    }
    /// The next `n` bits (at most 32) without consuming them.
    #[inline]
    pub fn peek(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        if self.bits < n {
            let word = self
                .data
                .get(self.pos..)
                .and_then(|d| d.get(..4))
                .map_or(0, |w| {
                    let w = [w[0], w[1], w[2], w[3]];
                    match self.order {
                        crate::io::Order::Little => u32::from_le_bytes(w),
                        crate::io::Order::Big => u32::from_be_bytes(w),
                    }
                });
            self.pos = self.pos.saturating_add(4);
            self.cache |= u64::from(word) << (32 - self.bits);
            self.bits += 32;
        }
        (self.cache >> (64 - n)) as u32
    }
    #[inline]
    pub fn skip(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        self.peek(n);
        self.cache <<= n;
        self.bits -= n;
    }
    /// The next `n` bits (at most 32).
    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn words_read_each_word_most_significant_bit_first() {
        let data = [0x78, 0x56, 0x34, 0x12, 0xff, 0, 0, 0x80];
        let mut w = Words::new(&data, crate::io::Order::Little);
        assert_eq!(w.get(0), 0);
        assert_eq!(w.get(4), 0x1);
        assert_eq!(w.peek(8), 0x23);
        assert_eq!(w.get(28), 0x234_5678);
        assert_eq!(w.get(32), 0x8000_00ff);
        assert_eq!(w.get(16), 0);
        let mut b = Words::new(&data, crate::io::Order::Big);
        assert_eq!(b.get(12), 0x785);
    }
    #[test]
    fn msb_reads_across_bytes() {
        let mut p = Msb::new(&[0b1010_1100, 0b0101_0011, 0xff]);
        assert_eq!(p.get(3), 0b101);
        assert_eq!(p.get(7), 0b011_0001);
        assert_eq!(p.peek(6), 0b01_0011);
        assert_eq!(p.get(14), 0b01_0011_1111_1111);
        assert!(!p.overrun());
        assert_eq!(p.get(32), 0);
        assert!(p.overrun());
    }
    #[test]
    fn jpeg_pump_unstuffs_and_stops_at_markers() {
        let mut p = Msb::jpeg(&[0xff, 0x00, 0x12, 0xff, 0xd9, 0x34]);
        assert_eq!(p.get(16), 0xff12);
        assert_eq!(p.get(8), 0);
        assert!(p.overrun() || p.get(32) == 0);
    }
    #[test]
    fn counts_what_is_left_before_a_marker() {
        let mut p = Msb::jpeg(&[0x12, 0xff, 0x00, 0x34, 0x56, 0xff, 0xd9]);
        assert_eq!(p.unread(), 32);
        p.get(12);
        assert_eq!(p.unread(), 20);
        p.get(20);
        assert_eq!(p.unread(), 0);
    }
    #[test]
    fn jpeg_pump_restarts_after_a_restart_marker() {
        let mut p = Msb::jpeg(&[0xab, 0xff, 0xd0, 0xcd]);
        assert_eq!(p.get(4), 0xa);
        p.restart();
        assert_eq!(p.get(8), 0xcd);
    }
}
