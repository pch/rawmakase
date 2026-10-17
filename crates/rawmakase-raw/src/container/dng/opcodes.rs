//! DNG opcode lists (DNG Specification 1.7.1.0, chapter 7, "Opcode List
//! Processing"): the data opcodes of OpcodeList1, applied to the raw image as
//! stored, and of OpcodeList2, applied once it is mapped to linear reference
//! values. Lists are big-endian whatever the file's byte order: a count, then
//! per opcode its ID, the DNG version that defined it, flags (bit 0
//! optional) and the size of its parameters.
//!
//! Both lists work here on 16-bit samples: OpcodeList1's logical range is
//! 0–65535, OpcodeList2's 0.0–1.0 stored as 0–65535. Opcodes computing in
//! floating point read a sample as its fraction of 65535 and write it back
//! rounded, clipped to the range after each opcode as the specification
//! says. Pixel positions are the modified image's: the stored image for
//! OpcodeList1, the active area for OpcodeList2.
//!
//! Applied: FixVignetteRadial, FixBadPixelsConstant, FixBadPixelsList,
//! MapTable, MapPolynomial, GainMap, DeltaPerRow/Column and
//! ScalePerRow/Column. Warps and TrimBounds change the image's geometry and
//! are skipped, as are opcodes of a later DNG version than 1.7.1 and unknown
//! ones, and gain maps whose every gain is 1. A list that does not parse is
//! not applied at all.
//!
//! Sources: the DNG Specification 1.7.1.0; our own measurements on the test
//! dataset's DNGs (where each list's coordinates start).
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// The latest DNG version whose opcodes are understood (1.7.1.0).
const VERSION: u32 = 0x0107_0100;
/// Rows between checks of the cancel flag.
const CANCEL_ROWS: usize = 64;

/// The part of the image an opcode modifies: its bounds (bottom and right
/// exclusive), planes and the pitch of the rows and columns it touches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Area {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
    pub plane: u32,
    pub planes: u32,
    pub row_pitch: u32,
    pub col_pitch: u32,
}

/// A gain map: `points` (vertical, horizontal) samples starting at `origin`,
/// `spacing` apart, in fractions of the image's height and width;
/// `planes` gains per point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GainMap {
    pub points: [usize; 2],
    pub spacing: [f64; 2],
    pub origin: [f64; 2],
    pub planes: usize,
    pub gains: Vec<f32>,
}

/// One opcode of a list, with its parameters.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Opcode {
    /// Gain `1 + k0 r² + … + k4 r¹⁰` around `centre` (fractions of the
    /// image), `r` relative to the farthest pixel.
    FixVignetteRadial {
        k: [f64; 5],
        centre: [f64; 2],
    },
    /// Pixels of value `constant` are bad: interpolated from their colour's
    /// neighbours in the Bayer mosaic of `phase`.
    FixBadPixelsConstant {
        constant: u32,
        phase: u32,
    },
    /// Bad points (row, column) and rectangles (top, left, bottom, right).
    FixBadPixelsList {
        phase: u32,
        points: Vec<[u32; 2]>,
        rects: Vec<[u32; 4]>,
    },
    MapTable {
        area: Area,
        table: Vec<u16>,
    },
    /// Coefficients from the constant term up.
    MapPolynomial {
        area: Area,
        coefficients: Vec<f64>,
    },
    GainMap {
        area: Area,
        map: GainMap,
    },
    /// Per-row (`rows`) or per-column values added (`scale` false) or
    /// multiplied.
    PerLine {
        area: Area,
        rows: bool,
        scale: bool,
        values: Vec<f32>,
    },
}

/// Big-endian reads of an opcode's parameters.
struct Params<'a> {
    data: &'a [u8],
    at: usize,
}

impl Params<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let b = self.data.get(self.at..end)?;
        self.at = end;
        Some(b)
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn f64(&mut self) -> Option<f64> {
        let b = self.take(8)?;
        let mut v = [0u8; 8];
        v.copy_from_slice(b);
        Some(f64::from_be_bytes(v))
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    /// `n` items, each read by `f`, at most as many as the bytes left
    /// could hold at `size` bytes each (so a count cannot allocate more
    /// than the list's own size).
    fn many<T>(
        &mut self,
        n: u32,
        size: usize,
        f: impl Fn(&mut Self) -> Option<T>,
    ) -> Option<Vec<T>> {
        let n = usize::try_from(n).ok()?;
        if n.checked_mul(size)? > self.data.len().saturating_sub(self.at) {
            return None;
        }
        (0..n).map(|_| f(self)).collect()
    }
    fn area(&mut self) -> Option<Area> {
        let area = Area {
            top: self.u32()?,
            left: self.u32()?,
            bottom: self.u32()?,
            right: self.u32()?,
            plane: self.u32()?,
            planes: self.u32()?,
            row_pitch: self.u32()?,
            col_pitch: self.u32()?,
        };
        (area.planes > 0 && area.row_pitch > 0 && area.col_pitch > 0).then_some(area)
    }
}

/// The opcodes of a list, those that are skipped left out; `None` when the
/// list does not parse (it is then not applied).
pub(crate) fn parse(list: &[u8]) -> Option<Vec<Opcode>> {
    if list.is_empty() {
        return Some(Vec::new());
    }
    let mut p = Params { data: list, at: 0 };
    let count = p.u32()?;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = p.u32()?;
        let version = p.u32()?;
        let _flags = p.u32()?;
        let size = usize::try_from(p.u32()?).ok()?;
        let data = p.take(size)?;
        if version > VERSION {
            continue;
        }
        if let Some(op) = opcode(id, data)? {
            out.push(op);
        }
    }
    Some(out)
}

/// An opcode from its ID and parameters: `None` when they do not parse,
/// `Some(None)` for an opcode that is skipped.
fn opcode(id: u32, data: &[u8]) -> Option<Option<Opcode>> {
    let mut p = Params { data, at: 0 };
    let op = match id {
        3 => {
            let k = [p.f64()?, p.f64()?, p.f64()?, p.f64()?, p.f64()?];
            let centre = [p.f64()?, p.f64()?];
            if k.iter().chain(&centre).any(|v| !v.is_finite()) {
                return None;
            }
            Opcode::FixVignetteRadial { k, centre }
        }
        4 => Opcode::FixBadPixelsConstant {
            constant: p.u32()?,
            phase: p.u32()?,
        },
        5 => {
            let phase = p.u32()?;
            let (np, nr) = (p.u32()?, p.u32()?);
            let points = p.many(np, 8, |p| Some([p.u32()?, p.u32()?]))?;
            let rects = p.many(nr, 16, |p| Some([p.u32()?, p.u32()?, p.u32()?, p.u32()?]))?;
            Opcode::FixBadPixelsList {
                phase,
                points,
                rects,
            }
        }
        7 => {
            let area = p.area()?;
            let n = p.u32()?;
            let table = p.many(n, 2, Params::u16)?;
            if table.is_empty() {
                return None;
            }
            Opcode::MapTable { area, table }
        }
        8 => {
            let area = p.area()?;
            let degree = p.u32()?;
            if degree > 8 {
                return None;
            }
            let coefficients = p.many(degree + 1, 8, Params::f64)?;
            if coefficients.iter().any(|c| !c.is_finite()) {
                return None;
            }
            Opcode::MapPolynomial { area, coefficients }
        }
        9 => {
            let area = p.area()?;
            let points = [p.u32()?, p.u32()?];
            let spacing = [p.f64()?, p.f64()?];
            let origin = [p.f64()?, p.f64()?];
            let planes = p.u32()?;
            let n = points[0]
                .checked_mul(points[1])?
                .checked_mul(planes)
                .filter(|&n| n > 0)?;
            let gains = p.many(n, 4, Params::f32)?;
            let usable = |v: f64, d: f64| v.is_finite() && d.is_finite() && d > 0.;
            if !usable(origin[0], spacing[0]) || !usable(origin[1], spacing[1]) {
                return None;
            }
            // Gains of 1 (Samsung's phones write such maps) change nothing.
            if gains.iter().all(|&g| g == 1.) {
                return Some(None);
            }
            Opcode::GainMap {
                area,
                map: GainMap {
                    points: [points[0] as usize, points[1] as usize],
                    spacing,
                    origin,
                    planes: planes as usize,
                    gains,
                },
            }
        }
        10..=13 => {
            let area = p.area()?;
            let n = p.u32()?;
            let values = p.many(n, 4, Params::f32)?;
            Opcode::PerLine {
                area,
                rows: matches!(id, 10 | 12),
                scale: id >= 12,
                values,
            }
        }
        // WarpRectilinear, WarpFisheye, TrimBounds, WarpRectilinear2 and
        // unknown opcodes.
        _ => return Some(None),
    };
    Some(Some(op))
}

/// The image a list modifies: `width × height` samples of one plane, from
/// `origin` (column, row) in `data`, rows `stride` samples apart.
pub(crate) struct Image<'a> {
    pub data: &'a mut [u16],
    pub stride: usize,
    pub origin: [usize; 2],
    pub width: usize,
    pub height: usize,
}

impl Image<'_> {
    fn index(&self, row: usize, col: usize) -> usize {
        (self.origin[1] + row) * self.stride + self.origin[0] + col
    }
    fn get(&self, row: usize, col: usize) -> u16 {
        self.data.get(self.index(row, col)).copied().unwrap_or(0)
    }
    fn set(&mut self, row: usize, col: usize, v: u16) {
        let i = self.index(row, col);
        if let Some(d) = self.data.get_mut(i) {
            *d = v;
        }
    }
    /// Whether the image lies inside `data`.
    fn fits(&self) -> bool {
        let end = (self.origin[1] + self.height)
            .checked_mul(self.stride)
            .is_some_and(|rows| rows <= self.data.len());
        end && self.origin[0] + self.width <= self.stride
    }
}

/// Applies `ops` to `image`, in order.
pub(crate) fn apply(ops: &[Opcode], image: &mut Image, cancel: &AtomicBool) -> Result<()> {
    if !image.fits() || image.width == 0 || image.height == 0 {
        return Ok(());
    }
    for op in ops {
        match op {
            Opcode::FixVignetteRadial { k, centre } => vignette(image, k, *centre, cancel)?,
            Opcode::FixBadPixelsConstant { constant, phase } => {
                let mut bad = Mask::new(image.width, image.height)?;
                for row in 0..image.height {
                    if row % CANCEL_ROWS == 0 && cancel.load(Ordering::Relaxed) {
                        return Err(Error::Cancelled);
                    }
                    for col in 0..image.width {
                        if u32::from(image.get(row, col)) == *constant {
                            bad.set(row, col);
                        }
                    }
                }
                patch(image, &bad, *phase, cancel)?;
            }
            Opcode::FixBadPixelsList {
                phase,
                points,
                rects,
            } => {
                let mut bad = Mask::new(image.width, image.height)?;
                for &[row, col] in points {
                    bad.set_checked(row, col);
                }
                bad.set_rects(rects, cancel)?;
                patch(image, &bad, *phase, cancel)?;
            }
            Opcode::MapTable { area, table } => {
                let last = table.len() - 1;
                map(image, area, cancel, |_, _, v| {
                    table[usize::from(v).min(last)]
                })?;
            }
            Opcode::MapPolynomial { area, coefficients } => {
                map(image, area, cancel, |_, _, v| {
                    let x = f64::from(v) / 65535.;
                    let y = coefficients.iter().rev().fold(0., |acc, c| acc * x + c);
                    level(y)
                })?;
            }
            Opcode::GainMap { area, map: gains } => {
                let (w, h) = (image.width as f64, image.height as f64);
                map(image, area, cancel, |row, col, v| {
                    let g = gains.gain(row as f64 / h, col as f64 / w);
                    level(f64::from(v) / 65535. * f64::from(g))
                })?;
            }
            Opcode::PerLine {
                area,
                rows,
                scale,
                values,
            } => {
                let (top, left) = (area.top as usize, area.left as usize);
                let (rp, cp) = (area.row_pitch as usize, area.col_pitch as usize);
                map(image, area, cancel, |row, col, v| {
                    let i = if *rows {
                        (row - top) / rp
                    } else {
                        (col - left) / cp
                    };
                    let Some(&d) = values.get(i) else {
                        return v;
                    };
                    let x = f64::from(v) / 65535.;
                    level(if *scale {
                        x * f64::from(d)
                    } else {
                        x + f64::from(d)
                    })
                })?;
            }
        }
    }
    Ok(())
}

/// A fraction of the range as a 16-bit sample, rounded and clipped.
fn level(v: f64) -> u16 {
    if v.is_nan() {
        0
    } else {
        (v.clamp(0., 1.) * 65535.).round() as u16
    }
}

impl GainMap {
    /// The gain at (`v`, `h`), fractions of the image's height and width:
    /// bilinear between the map's points, the edge points' outside them.
    fn gain(&self, v: f64, h: f64) -> f32 {
        let at = |x: f64, axis: usize| {
            let n = self.points[axis];
            let f = ((x - self.origin[axis]) / self.spacing[axis]).clamp(0., (n - 1) as f64);
            let i = (f.floor() as usize).min(n - 1);
            (i, (i + 1).min(n - 1), (f - i as f64) as f32)
        };
        let (r0, r1, tv) = at(v, 0);
        let (c0, c1, th) = at(h, 1);
        let g = |r: usize, c: usize| {
            self.gains
                .get((r * self.points[1] + c) * self.planes)
                .copied()
                .unwrap_or(1.)
        };
        let top = g(r0, c0) + (g(r0, c1) - g(r0, c0)) * th;
        let bottom = g(r1, c0) + (g(r1, c1) - g(r1, c0)) * th;
        top + (bottom - top) * tv
    }
}

/// Maps every sample of `area` (clipped to the image) by `f(row, column,
/// value)`. The image has one plane: an area starting at a later plane
/// modifies nothing.
fn map(
    image: &mut Image,
    area: &Area,
    cancel: &AtomicBool,
    f: impl Fn(usize, usize, u16) -> u16,
) -> Result<()> {
    if area.plane != 0 {
        return Ok(());
    }
    let bottom = (area.bottom as usize).min(image.height);
    let right = (area.right as usize).min(image.width);
    let rows = (area.top as usize..bottom).step_by(area.row_pitch as usize);
    for (n, row) in rows.enumerate() {
        if n % CANCEL_ROWS == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for col in (area.left as usize..right).step_by(area.col_pitch as usize) {
            let v = image.get(row, col);
            image.set(row, col, f(row, col, v));
        }
    }
    Ok(())
}

/// FixVignetteRadial over the whole image.
fn vignette(image: &mut Image, k: &[f64; 5], centre: [f64; 2], cancel: &AtomicBool) -> Result<()> {
    let (x1, y1) = ((image.width - 1) as f64, (image.height - 1) as f64);
    let (cx, cy) = (centre[0] * x1, centre[1] * y1);
    let mx = cx.abs().max((x1 - cx).abs());
    let my = cy.abs().max((y1 - cy).abs());
    let m2 = mx * mx + my * my;
    if m2 <= 0. {
        return Ok(());
    }
    let area = Area {
        top: 0,
        left: 0,
        bottom: image.height as u32,
        right: image.width as u32,
        plane: 0,
        planes: 1,
        row_pitch: 1,
        col_pitch: 1,
    };
    map(image, &area, cancel, |row, col, v| {
        let (dx, dy) = (col as f64 - cx, row as f64 - cy);
        let r2 = (dx * dx + dy * dy) / m2;
        let g = 1. + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))));
        level(f64::from(v) / 65535. * g)
    })
}

/// One bit per pixel of the image.
struct Mask {
    bits: Vec<u64>,
    width: usize,
    height: usize,
    any: bool,
}

impl Mask {
    fn new(width: usize, height: usize) -> Result<Self> {
        let n = width
            .checked_mul(height)
            .ok_or(Error::TooLarge("bad pixel mask"))?
            .div_ceil(64);
        let mut bits = Vec::new();
        bits.try_reserve_exact(n)
            .map_err(|_| Error::TooLarge("bad pixel mask"))?;
        bits.resize(n, 0);
        Ok(Self {
            bits,
            width,
            height,
            any: false,
        })
    }
    fn set(&mut self, row: usize, col: usize) {
        let i = row * self.width + col;
        if let Some(w) = self.bits.get_mut(i / 64) {
            *w |= 1 << (i % 64);
            self.any = true;
        }
    }
    fn set_checked(&mut self, row: u32, col: u32) {
        let (row, col) = (row as usize, col as usize);
        if row < self.height && col < self.width {
            self.set(row, col);
        }
    }
    /// Marks the union of `rects` (top, left, bottom, right; bottom and
    /// right exclusive), clipped to the mask: a sweep down the rows keeps
    /// the rectangles covering each column as a difference array, so the
    /// work is the mask's area plus the rectangles' count, however many
    /// overlap.
    fn set_rects(&mut self, rects: &[[u32; 4]], cancel: &AtomicBool) -> Result<()> {
        let (w, h) = (self.width, self.height);
        // (row, left, right, +1 where a rectangle starts, -1 where it ends).
        let mut events: Vec<(usize, usize, usize, i32)> = Vec::new();
        for &[top, left, bottom, right] in rects {
            let (top, bottom) = (top as usize, (bottom as usize).min(h));
            let (left, right) = (left as usize, (right as usize).min(w));
            if top < bottom && left < right {
                events
                    .try_reserve(2)
                    .map_err(|_| Error::TooLarge("bad pixel rectangles"))?;
                events.push((top, left, right, 1));
                events.push((bottom, left, right, -1));
            }
        }
        if events.is_empty() {
            return Ok(());
        }
        events.sort_unstable();
        let mut diff = Vec::new();
        diff.try_reserve_exact(w + 1)
            .map_err(|_| Error::TooLarge("bad pixel mask"))?;
        diff.resize(w + 1, 0i32);
        let (mut next, mut active) = (0, 0i64);
        for row in 0..h {
            if row % CANCEL_ROWS == 0 && cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            while let Some(&(at, left, right, delta)) = events.get(next)
                && at == row
            {
                diff[left] += delta;
                diff[right] -= delta;
                active += i64::from(delta);
                next += 1;
            }
            if active == 0 {
                if next == events.len() {
                    break;
                }
                continue;
            }
            let mut covered = 0;
            for (col, d) in diff[..w].iter().enumerate() {
                covered += d;
                if covered > 0 {
                    self.set(row, col);
                }
            }
        }
        Ok(())
    }
    fn get(&self, row: usize, col: usize) -> bool {
        let i = row * self.width + col;
        self.bits
            .get(i / 64)
            .is_some_and(|w| w >> (i % 64) & 1 == 1)
    }
}

/// Interpolates every bad pixel from its colour's good neighbours in a
/// Bayer mosaic whose top-left pixel is red (`phase` 0), green on a red row
/// (1), green on a blue row (2) or blue (3): the nearest ring of them that
/// has any (a green's diagonal neighbours first, then two pixels away
/// along the row and column and diagonally), else the first good pixel of
/// the colour along each of the four directions, within 16 two-pixel steps.
fn patch(image: &mut Image, bad: &Mask, phase: u32, cancel: &AtomicBool) -> Result<()> {
    if !bad.any {
        return Ok(());
    }
    // Greens are where (row + column) has this parity.
    let green_parity = usize::from(matches!(phase, 0 | 3));
    let (w, h) = (image.width as isize, image.height as isize);
    let good = |image: &Image, r: isize, c: isize| -> Option<u32> {
        (r >= 0 && c >= 0 && r < h && c < w && !bad.get(r as usize, c as usize))
            .then(|| u32::from(image.get(r as usize, c as usize)))
    };
    const DIAGONAL: [(isize, isize); 4] = [(-1, -1), (-1, 1), (1, -1), (1, 1)];
    const NEAR: [(isize, isize); 4] = [(-2, 0), (2, 0), (0, -2), (0, 2)];
    const FAR: [(isize, isize); 4] = [(-2, -2), (-2, 2), (2, -2), (2, 2)];
    const NEAR_FAR: [(isize, isize); 8] = [
        (-2, 0),
        (2, 0),
        (0, -2),
        (0, 2),
        (-2, -2),
        (-2, 2),
        (2, -2),
        (2, 2),
    ];
    for row in 0..image.height {
        if row % CANCEL_ROWS == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for col in 0..image.width {
            if !bad.get(row, col) {
                continue;
            }
            let (r, c) = (row as isize, col as isize);
            let green = (row + col) % 2 == green_parity;
            let rings: &[&[(isize, isize)]] = if green {
                &[&DIAGONAL, &NEAR_FAR]
            } else {
                &[&NEAR, &FAR]
            };
            let mut found = None;
            for ring in rings {
                let values: Vec<u32> = ring
                    .iter()
                    .filter_map(|&(dr, dc)| good(image, r + dr, c + dc))
                    .collect();
                if !values.is_empty() {
                    found = Some(values);
                    break;
                }
            }
            let values = found.unwrap_or_else(|| {
                // Whole two-pixel steps, so every pixel tried is the colour's.
                NEAR.iter()
                    .filter_map(|&(dr, dc)| {
                        (1..=16).find_map(|k| good(image, r + dr * k, c + dc * k))
                    })
                    .collect()
            });
            if !values.is_empty() {
                let n = values.len() as u32;
                let sum: u32 = values.iter().sum();
                image.set(row, col, ((sum + n / 2) / n) as u16);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An opcode list of `(id, flags, parameters)`.
    pub(crate) fn list(ops: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = (ops.len() as u32).to_be_bytes().to_vec();
        for (id, flags, params) in ops {
            out.extend(id.to_be_bytes());
            out.extend(0x0103_0000u32.to_be_bytes());
            out.extend(flags.to_be_bytes());
            out.extend((params.len() as u32).to_be_bytes());
            out.extend(params);
        }
        out
    }
    pub(crate) fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    pub(crate) fn doubles(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    pub(crate) fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    /// A GainMap's parameters over `area` (top, left, bottom, right, pitch)
    /// with `points` (vertical, horizontal) gains from the origin to 1.0.
    pub(crate) fn gain_map(area: [u32; 5], points: [u32; 2], gains: &[f32]) -> Vec<u8> {
        let [t, l, b, r, pitch] = area;
        let mut p = longs(&[t, l, b, r, 0, 1, pitch, pitch, points[0], points[1]]);
        let spacing = |n: u32| if n > 1 { 1. / f64::from(n - 1) } else { 1. };
        p.extend(doubles(&[spacing(points[0]), spacing(points[1]), 0., 0.]));
        p.extend(longs(&[1]));
        p.extend(floats(gains));
        p
    }

    fn run(list: &[u8], data: &mut [u16], width: usize) {
        let ops = parse(list).expect("list parses");
        let height = data.len() / width;
        let mut image = Image {
            data,
            stride: width,
            origin: [0, 0],
            width,
            height,
        };
        apply(&ops, &mut image, &AtomicBool::new(false)).unwrap();
    }

    /// A gain map of 2 × 2 points over a 3 × 3 image, every pixel: row 0 and
    /// column 0 sit on the first points, the rest between them (positions
    /// are row / height and column / width).
    #[test]
    fn gain_map_interpolates_between_points() {
        let l = list(&[(9, 1, gain_map([0, 0, 3, 3, 1], [2, 2], &[1., 2., 3., 4.]))]);
        let mut data = vec![10000u16; 9];
        run(&l, &mut data, 3);
        // Gain at (v, h) = 1 + h + 2 v (bilinear over 1, 2 / 3, 4).
        let want = |r: f64, c: f64| {
            ((10000. / 65535. * (1. + c / 3. + 2. * r / 3.)) * 65535.).round() as u16
        };
        for r in 0..3 {
            for c in 0..3 {
                assert_eq!(data[r * 3 + c], want(r as f64, c as f64), "{r} {c}");
            }
        }
    }

    /// The pitch and bounds limit the pixels a gain touches; results clip at
    /// the range's top.
    #[test]
    fn gain_map_keeps_to_its_area_and_clips() {
        let l = list(&[(9, 0, gain_map([1, 0, 4, 4, 2], [1, 1], &[2.]))]);
        let mut data = vec![40000u16; 16];
        run(&l, &mut data, 4);
        for r in 0..4 {
            for c in 0..4 {
                let want = if r % 2 == 1 && c % 2 == 0 {
                    65535
                } else {
                    40000
                };
                assert_eq!(data[r * 4 + c], want, "{r} {c}");
            }
        }
    }

    #[test]
    fn maps_through_a_table_and_a_polynomial() {
        // A table of three entries: values past it take the last.
        let mut p = longs(&[0, 0, 1, 4, 0, 1, 1, 1, 3]);
        p.extend([0, 7, 0, 8, 0, 9]);
        let mut data = vec![0, 1, 2, 500];
        run(&list(&[(7, 0, p)]), &mut data, 4);
        assert_eq!(data, [7, 8, 9, 9]);
        // y = 0.25 + 0.5 x over columns 1–2 only.
        let mut p = longs(&[0, 1, 1, 3, 0, 1, 1, 1, 1]);
        p.extend(doubles(&[0.25, 0.5]));
        let mut data = vec![65535u16; 4];
        run(&list(&[(8, 0, p)]), &mut data, 4);
        assert_eq!(data, [65535, 49151, 49151, 65535]);
    }

    #[test]
    fn adds_and_scales_per_row_and_column() {
        // DeltaPerRow on rows 0 and 2 (pitch 2), ScalePerColumn on both columns.
        let mut delta = longs(&[0, 0, 4, 2, 0, 1, 2, 1, 2]);
        delta.extend(floats(&[0.5, 0.25]));
        let mut scale = longs(&[0, 0, 4, 2, 0, 1, 1, 1, 2]);
        scale.extend(floats(&[1., 0.5]));
        let mut data = vec![0u16; 8];
        run(&list(&[(10, 0, delta), (13, 0, scale)]), &mut data, 2);
        assert_eq!(data, [32768, 16384, 0, 0, 16384, 8192, 0, 0]);
    }

    #[test]
    fn fixes_bad_pixels_from_their_colour() {
        // An RGGB 6 × 6 mosaic: red 100, green 200, blue 300; a red and a
        // green pixel marked 0.
        let mut data: Vec<u16> = (0..36)
            .map(|i| match (i / 6 % 2, i % 6 % 2) {
                (0, 0) => 100,
                (1, 1) => 300,
                _ => 200,
            })
            .collect();
        let want = data.clone();
        data[2 * 6 + 2] = 0;
        data[3 * 6 + 2] = 0;
        let mut fixed = data.clone();
        run(&list(&[(4, 1, longs(&[0, 0]))]), &mut fixed, 6);
        assert_eq!(fixed, want);
        // The same as a list: a point and a one-pixel rectangle.
        let mut fixed = data.clone();
        let p = longs(&[0, 1, 1, 2, 2, 3, 2, 4, 3]);
        run(&list(&[(5, 0, p)]), &mut fixed, 6);
        assert_eq!(fixed, want);
    }

    /// Far from good neighbours, a bad pixel takes its own colour's nearest
    /// good ones, whole Bayer periods away: an 11 × 11 RGGB mosaic (red
    /// 100, green 200, blue 300) whose nine red sites at rows and columns
    /// 2, 4, 6 are bad keeps red 100 at the centre.
    #[test]
    fn fallback_search_keeps_to_the_colour() {
        let colour = |r: usize, c: usize| match (r % 2, c % 2) {
            (0, 0) => 100u16,
            (1, 1) => 300,
            _ => 200,
        };
        let mut data: Vec<u16> = (0..121).map(|i| colour(i / 11, i % 11)).collect();
        let mut p = longs(&[0, 9, 0]);
        for r in [2, 4, 6] {
            for c in [2, 4, 6] {
                p.extend(longs(&[r, c]));
            }
        }
        run(&list(&[(5, 0, p)]), &mut data, 11);
        let want: Vec<u16> = (0..121).map(|i| colour(i / 11, i % 11)).collect();
        assert_eq!(data, want);
    }

    /// Rectangles mark their union, clipped to the image, however many
    /// overlap; their work stops when cancelled.
    #[test]
    fn bad_rectangles_mark_their_union() {
        let mut bad = Mask::new(6, 4).unwrap();
        let none = AtomicBool::new(false);
        let rects = [
            [0, 1, 2, 3],
            [1, 2, 3, 4],
            [1, 2, 3, 4],
            [3, 5, 9, 9],
            [2, 2, 2, 5],
        ];
        bad.set_rects(&rects, &none).unwrap();
        let marked: Vec<(usize, usize)> = (0..4)
            .flat_map(|r| (0..6).map(move |c| (r, c)))
            .filter(|&(r, c)| bad.get(r, c))
            .collect();
        assert_eq!(
            marked,
            [
                (0, 1),
                (0, 2),
                (1, 1),
                (1, 2),
                (1, 3),
                (2, 2),
                (2, 3),
                (3, 5)
            ]
        );
        // 512 copies of a rectangle over a 1024 × 1024 image, cancelled.
        let mut p = longs(&[0, 0, 512]);
        for _ in 0..512 {
            p.extend(longs(&[0, 0, 1024, 1024]));
        }
        let ops = parse(&list(&[(5, 0, p)])).unwrap();
        let mut data = vec![1u16; 1024 * 1024];
        let mut image = Image {
            data: &mut data,
            stride: 1024,
            origin: [0, 0],
            width: 1024,
            height: 1024,
        };
        let cancelled = AtomicBool::new(true);
        assert_eq!(apply(&ops, &mut image, &cancelled), Err(Error::Cancelled));
        let started = std::time::Instant::now();
        apply(&ops, &mut image, &none).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    /// The vignette's gain at the farthest pixel is 1 + Σk; its centre keeps
    /// its value.
    #[test]
    fn corrects_radial_vignetting() {
        let mut p = doubles(&[1., 0., 0., 0., 0.]);
        p.extend(doubles(&[0.5, 0.5]));
        let mut data = vec![20000u16; 9];
        run(&list(&[(3, 0, p)]), &mut data, 3);
        assert_eq!(data[4], 20000);
        assert_eq!(data[0], 40000);
        // Half way along an axis: r² = 1 / 2.
        assert_eq!(data[1], 30000);
    }

    /// Warps, later versions and unknown opcodes are skipped; a list that
    /// ends early does not parse.
    #[test]
    fn skips_what_it_does_not_apply() {
        let identity = gain_map([0, 0, 2, 2, 1], [2, 2], &[1.; 4]);
        let l = list(&[
            (1, 0, vec![0; 8]),
            (99, 1, vec![]),
            (6, 0, longs(&[0, 0, 1, 1])),
            (9, 1, identity),
        ]);
        assert_eq!(parse(&l), Some(vec![]));
        let mut later = list(&[(4, 0, longs(&[0, 0]))]);
        later[8..12].copy_from_slice(&0x0108_0000u32.to_be_bytes());
        assert_eq!(parse(&later), Some(vec![]));
        assert_eq!(parse(&[]), Some(vec![]));
        let full = list(&[(9, 0, gain_map([0, 0, 2, 2, 1], [2, 2], &[1.; 4]))]);
        for n in 1..full.len() {
            assert_eq!(parse(&full[..n]), None, "{n} bytes");
        }
    }

    /// Hostile counts and sizes are refused before allocating, and pitches
    /// of zero or areas outside the image do nothing.
    #[test]
    fn hostile_lists_are_refused() {
        let huge = list(&[(5, 0, longs(&[0, u32::MAX, u32::MAX]))]);
        assert_eq!(parse(&huge), None);
        let mut map = longs(&[0, 0, 2, 2, 0, 1, 1, 1, u32::MAX, u32::MAX]);
        map.extend(doubles(&[1., 1., 0., 0.]));
        map.extend(longs(&[u32::MAX]));
        assert_eq!(parse(&list(&[(9, 0, map)])), None);
        let zero_pitch = longs(&[0, 0, 2, 2, 0, 1, 0, 1, 1, 0, 0]);
        assert_eq!(parse(&list(&[(7, 0, zero_pitch)])), None);
        let count = u32::MAX.to_be_bytes().to_vec();
        assert_eq!(parse(&count), None);
        let mut data = vec![7u16; 4];
        let outside = longs(&[100, 100, u32::MAX, u32::MAX, 0, 1, 1, 1, 1, 0, 0]);
        run(&list(&[(7, 0, outside)]), &mut data, 2);
        assert_eq!(data, [7; 4]);
    }

    #[test]
    fn stops_when_cancelled() {
        let ops = parse(&list(&[(9, 0, gain_map([0, 0, 4, 4, 1], [1, 1], &[2.]))])).unwrap();
        let mut data = vec![1u16; 16];
        let mut image = Image {
            data: &mut data,
            stride: 4,
            origin: [0, 0],
            width: 4,
            height: 4,
        };
        let cancelled = AtomicBool::new(true);
        assert_eq!(apply(&ops, &mut image, &cancelled), Err(Error::Cancelled));
    }
}
