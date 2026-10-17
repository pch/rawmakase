//! The coefficient coding and wavelet of SMPTE RDD 34 (LLVC), as Sony's
//! "compressed RAW 2" uses them: lines of coefficients in sets of four
//! (a bit depth, magnitudes and signs, runs of zero sets), dequantisation,
//! and the 5/3 inverse wavelet by lifting.
//!
//! Sources: SMPTE RDD 34:2015 §6.2–6.5 (the variable-length decoding state
//! machine of figures 6.5–6.8, inverse quantisation, the lifting steps and
//! symmetric extension); our own measurements against LibRaw's output on our
//! files for where Sony's streams differ: a zero code at depth 0 is a zero set
//! that starts a run, a run code's count ends at one, and dequantisation
//! reconstructs to the middle of the quantisation step.
use super::bits::Msb;
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// The deepest coefficient bit depth decoded.
const MAX_DEPTH: u32 = 24;

/// Bits of a half: the pump and how many bits it holds.
pub(crate) struct Half<'a> {
    bits: Msb<'a>,
    len: u64,
}

impl<'a> Half<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            bits: Msb::new(data),
            len: data.len() as u64 * 8,
        }
    }
    /// The bits read so far.
    pub fn consumed(&self) -> u64 {
        self.bits.consumed()
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    fn check(&self) -> Result<()> {
        if self.bits.consumed() > self.len {
            return Err(corrupt("LLVC line past the end of its data"));
        }
        Ok(())
    }
    fn bit(&mut self) -> Result<u32> {
        let b = self.bits.get(1);
        self.check()?;
        Ok(b)
    }
    fn get(&mut self, n: u32) -> Result<u32> {
        let v = self.bits.get(n);
        self.check()?;
        Ok(v)
    }
    /// The zeros before a one, the one consumed.
    fn zeros_then_one(&mut self) -> Result<u32> {
        let mut n = 0;
        while self.bit()? == 0 {
            n += 1;
            if n > MAX_DEPTH {
                return Err(corrupt("LLVC depth code"));
            }
        }
        Ok(n)
    }
}

/// Decodes one line of `width` coefficients: sets of four, each a bit depth
/// change (or a run of zero sets), then the previous set's signs, then this
/// set's magnitudes of that depth (RDD 34 figures 6.5–6.8).
pub(crate) fn line(half: &mut Half, width: usize) -> Result<Vec<i32>> {
    let mut out = Vec::with_capacity(width.div_ceil(4) * 4);
    if width == 0 {
        return Ok(out);
    }
    let mut cnt = width.div_ceil(4) as u64;
    let (mut abs, mut depth, mut state, mut run) = ([0u32; 4], 0u32, 0u8, 0u64);
    let (mut abs_valid, mut abs_end, mut depth_valid, mut depth_end) = (false, false, true, false);
    loop {
        if cnt == 0 {
            depth_valid = false;
        }
        if cnt == 1 {
            depth_end = true;
        }
        cnt = cnt.saturating_sub(1);
        run = run.saturating_sub(1);
        if depth_valid {
            match state {
                0 => {
                    if half.bit()? == 0 {
                        // A zero code at depth 0 is a zero set that starts a
                        // run (our measurements); otherwise no change.
                        if depth == 0 {
                            state = 1;
                        }
                    } else if half.bit()? == 0 {
                        depth += half.zeros_then_one()? + 1;
                    } else {
                        // "1 1 0^n 1": down by n + 1; "1 1 0^(depth − 1)": to 0.
                        let mut n = 0;
                        while n + 1 < depth && half.bit()? == 0 {
                            n += 1;
                        }
                        if n + 1 >= depth {
                            depth = 0;
                            state = 1;
                        } else {
                            depth -= n + 1;
                        }
                    }
                }
                1 => {
                    if half.bit()? == 1 {
                        depth = half.zeros_then_one()? + 1;
                        state = 0;
                    } else {
                        // A run: "0^lim" is the rest of the line, else
                        // "0^m 1" and m bits under a leading one.
                        let lim = (cnt + 2).next_power_of_two().trailing_zeros();
                        let mut m = 1;
                        while m < lim && half.bit()? == 0 {
                            m += 1;
                        }
                        if m >= lim {
                            run = cnt + 2;
                        } else {
                            run = 1;
                            for _ in 0..m {
                                run = run << 1 | u64::from(half.bit()?);
                            }
                        }
                        state = 2;
                    }
                }
                _ => {
                    // The run ends when its count reaches one (our
                    // measurements).
                    if run == 1 {
                        depth = half.zeros_then_one()? + 1;
                        state = 0;
                    }
                }
            }
            if depth > MAX_DEPTH {
                return Err(corrupt("LLVC depth"));
            }
        }
        if abs_valid {
            for &a in &abs {
                let v = a as i32;
                out.push(if a != 0 && half.bit()? == 1 { -v } else { v });
            }
        }
        if abs_end {
            out.truncate(width);
            return Ok(out);
        }
        abs_valid = depth_valid;
        abs_end = depth_end;
        if depth_valid {
            for a in &mut abs {
                *a = half.get(depth)?;
            }
        }
    }
}

/// A quantised coefficient's value: unchanged at index 0, else the middle of
/// its step of `1 << qi`, odd magnitudes one lower (our measurements).
pub(crate) fn dequantise(q: i32, qi: u32) -> i32 {
    if q == 0 || qi == 0 {
        return q;
    }
    let a = i64::from(q).abs();
    let m = (((2 * a + 1) << (qi - 1).min(30)) - (a & 1)).min(i64::from(i32::MAX)) as i32;
    if q < 0 { -m } else { m }
}

/// The low and high sample counts of `n` samples whose low-pass samples are
/// those at `phase` modulo 2.
pub(crate) fn split(n: usize, phase: usize) -> (usize, usize) {
    let low = (n + 1).saturating_sub(phase) / 2;
    (low, n - low)
}

/// A row-major plane of samples.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<i32>,
}

impl Plane {
    pub fn zeros(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height],
        }
    }
    pub fn row(&self, y: usize) -> &[i32] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
    pub fn row_mut(&mut self, y: usize) -> &mut [i32] {
        &mut self.data[y * self.width..(y + 1) * self.width]
    }
}

/// The 5/3 inverse by lifting of one signal of low samples `l` and high
/// samples `h`, the low ones at `phase` modulo 2, into `out`; out-of-range
/// neighbours by whole-sample symmetric extension (RDD 34 §6.5).
fn lift(l: &[i32], h: &[i32], phase: usize, even: &mut Vec<i32>, out: &mut [i32]) {
    let (nl, nh) = (l.len() as isize, h.len() as isize);
    let p = phase as isize;
    let hs = |i: isize| {
        if nh == 0 {
            0
        } else {
            i64::from(h[i.clamp(0, nh - 1) as usize])
        }
    };
    even.clear();
    even.extend(l.iter().enumerate().map(|(j, &v)| {
        let j = j as isize;
        (i64::from(v) - ((hs(j - 1 + p) + hs(j + p) + 2) >> 2)) as i32
    }));
    let es = |i: isize| {
        if nl == 0 {
            0
        } else {
            i64::from(even[i.clamp(0, nl - 1) as usize])
        }
    };
    for (o, &e) in out.iter_mut().skip(phase).step_by(2).zip(even.iter()) {
        *o = e;
    }
    for (k, (o, &v)) in out.iter_mut().skip(1 - phase).step_by(2).zip(h).enumerate() {
        let k = k as isize;
        *o = (i64::from(v) + ((es(k - p) + es(k + 1 - p)) >> 1)) as i32;
    }
}

/// The vertical inverse of a low and a high band of equal width.
fn vertical(low: &Plane, high: &Plane, phase: usize, cancel: &AtomicBool) -> Result<Plane> {
    let (w, n) = (low.width, low.height + high.height);
    let mut out = Plane::zeros(w, n);
    let (mut l, mut h, mut col, mut even) = (vec![], vec![], vec![0; n], vec![]);
    for x in 0..w {
        if x % 4 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        l.clear();
        h.clear();
        l.extend((0..low.height).map(|y| low.data[y * w + x]));
        h.extend((0..high.height).map(|y| high.data[y * w + x]));
        lift(&l, &h, phase, &mut even, &mut col);
        for (y, &v) in col.iter().enumerate() {
            out.data[y * w + x] = v;
        }
    }
    Ok(out)
}

/// The horizontal inverse of a low and a high band of equal height, low
/// samples at even columns.
fn horizontal(low: &Plane, high: &Plane, cancel: &AtomicBool) -> Result<Plane> {
    let (n, rows) = (low.width + high.width, low.height);
    let mut out = Plane::zeros(n, rows);
    let mut even = vec![];
    for y in 0..rows {
        if y % 16 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        lift(low.row(y), high.row(y), 0, &mut even, out.row_mut(y));
    }
    Ok(out)
}

/// The bands of one level: LL (or the level below's result), LH, HL, HH.
pub(crate) struct Level<'a> {
    pub ll: &'a Plane,
    pub lh: &'a Plane,
    pub hl: &'a Plane,
    pub hh: &'a Plane,
}

/// One level's 2-D inverse (RDD 34 figure 6.10): vertical on LL with LH and
/// on HL with HH (low rows at `phase`), then horizontal.
pub(crate) fn inverse(b: &Level, phase: usize, cancel: &AtomicBool) -> Result<Plane> {
    let fits = b.ll.width == b.lh.width
        && b.hl.width == b.hh.width
        && b.ll.height == b.hl.height
        && b.lh.height == b.hh.height
        && split(b.ll.height + b.lh.height, phase) == (b.ll.height, b.lh.height)
        && b.ll.width == b.hl.width;
    if !fits {
        return Err(corrupt("LLVC band sizes"));
    }
    let low = vertical(b.ll, b.lh, phase, cancel)?;
    let high = vertical(b.hl, b.hh, phase, cancel)?;
    horizontal(&low, &high, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<u8> {
        let b: Vec<bool> = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c == '1')
            .collect();
        b.chunks(8)
            .map(|c| c.iter().fold(0u8, |a, &x| a << 1 | u8::from(x)) << (8 - c.len()))
            .collect()
    }

    #[test]
    fn decodes_rdd34s_worked_example() {
        // RDD 34 figure 6.9: eight sets, the codes in the figure's order.
        let s = "100001 0110000010000100 11000 010 100001 01001101000000000000 \
                 110000 10 010 001 001010011100 1100 0000 0";
        let data = bits(s);
        let mut half = Half::new(&data);
        let out = line(&mut half, 32).unwrap();
        let want = [
            6, 0, -8, 4, 0, 0, 0, 0, -9, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        assert_eq!(out, want);
        assert_eq!(
            half.consumed(),
            s.chars().filter(|c| *c == '0' || *c == '1').count() as u64
        );
    }

    #[test]
    fn dequantises_to_the_middle_of_the_step() {
        assert_eq!(dequantise(0, 3), 0);
        assert_eq!(dequantise(5, 0), 5);
        // 2|q| + 1 = 5, << 1 = 10, odd magnitude one lower: 9.
        assert_eq!(dequantise(2, 2), 10);
        assert_eq!(dequantise(1, 2), 5);
        assert_eq!(dequantise(-3, 1), -6);
    }

    #[test]
    fn lifting_inverts_the_forward_53_transform() {
        // Forward 5/3 with symmetric extension, written from its definition.
        let x: Vec<i32> = (0..11).map(|i| (i * 37 % 23) - 5).collect();
        for phase in 0..2 {
            let n = x.len() as isize;
            let at = |i: isize| {
                x[(if i < 0 {
                    -i
                } else if i >= n {
                    2 * (n - 1) - i
                } else {
                    i
                }) as usize]
            };
            let hi: Vec<i32> = (0..n)
                .filter(|t| (t % 2) as usize != phase)
                .map(|t| at(t) - ((at(t - 1) + at(t + 1)) >> 1))
                .collect();
            let hv = |t: isize| -> i32 {
                let t = if t < 0 {
                    -t
                } else if t >= n {
                    2 * (n - 1) - t
                } else {
                    t
                };
                at(t) - ((at(t - 1) + at(t + 1)) >> 1)
            };
            let lo: Vec<i32> = (0..n)
                .filter(|t| (t % 2) as usize == phase)
                .map(|t| at(t) + ((hv(t - 1) + hv(t + 1) + 2) >> 2))
                .collect();
            let mut out = vec![0; x.len()];
            lift(&lo, &hi, phase, &mut vec![], &mut out);
            assert_eq!(out, x, "phase {phase}");
        }
    }

    #[test]
    fn truncated_lines_are_errors() {
        let mut half = Half::new(&[0xff, 0xff]);
        assert!(line(&mut half, 64).is_err());
    }
}
