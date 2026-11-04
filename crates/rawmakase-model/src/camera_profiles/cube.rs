//! Lookup tables from `.cube` files, as Adobe's Cube LUT specification 1.0 writes
//! them. A 3D cube is a regular grid of colours: `LUT_3D_SIZE` divisions per side, one
//! output colour per node, read by tetrahedral interpolation — the same interpolation
//! `camera_profiles::rgb_table` already uses for Adobe's own 3D RGB tables, and for the
//! same reason: it is what the DNG SDK does. A 1D cube (`LUT_1D_SIZE` alone) is a curve
//! per channel.
//!
//! A cube carries no primaries, so it says nothing about the space its numbers live in
//! and is conventionally authored in a display space, usually gamma-encoded sRGB. RAWmakase
//! imports one as a look profile's RGB table (`rgb_table::RgbTable::from_cube`), as
//! Camera Raw's Create Profile does; this module only knows how to read a cube and look
//! a colour up in it.
//!
//! Data is stored with the red index changing fastest, then green, then blue, as the
//! specification requires, so node `(r, g, b)` is at `r + g * size + b * size * size`.
use super::rgb_table::tetrahedral;
use anyhow::{Result, bail, ensure};

/// A `.cube` 3D lookup table: a regular grid of `size` divisions per side over
/// `domain_min` to `domain_max`, tetrahedrally interpolated.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Lut3d {
    size: usize,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    /// The 1D table the input goes through first, when the file carries one.
    shaper: Option<Shaper>,
    /// `size * size * size` output colours, red index fastest.
    data: Vec<[f32; 3]>,
}

/// A 1D table a file puts in front of its 3D one: a per-channel curve the input goes
/// through before the lookup. Resolving calls these "shaper" LUTs, and they are why a
/// file may carry both sizes. Reading the 3D table and dropping this one renders
/// plausible, wrong colours rather than failing.
#[derive(Clone, Debug, PartialEq)]
struct Shaper {
    /// Entries per channel.
    size: usize,
    /// `LUT_1D_INPUT_RANGE`, which the input is read over. 0-1 by default.
    input_range: [f32; 2],
    /// One triple per entry; each of its three channels is that channel's curve.
    table: Vec<[f32; 3]>,
}

impl Shaper {
    /// The colour the curves give for `rgb`, each channel interpolated on its own.
    fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [low, high] = self.input_range;
        std::array::from_fn(|c| {
            let unit = ((rgb[c] - low) / (high - low)).clamp(0.0, 1.0);
            let at = unit * (self.size - 1) as f32;
            // Clamped so the last entry, whose position is exactly one, still has a
            // pair of entries to be interpolated between.
            let index = (at as usize).min(self.size - 2);
            let within = at - index as f32;
            self.table[index][c] + within * (self.table[index + 1][c] - self.table[index][c])
        })
    }
}

/// What a `.cube` file holds: a 3D table, or a 1D one alone.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Cube {
    Three(Lut3d),
    One(Lut1d),
}

/// A 1D `.cube`: a curve per channel, `size` entries over `domain_min` to
/// `domain_max`, interpolated linearly.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Lut1d {
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    /// One triple per entry; each of its three channels is that channel's curve.
    table: Vec<[f32; 3]>,
}

impl Lut1d {
    /// The curve's entries.
    pub(super) fn size(&self) -> usize {
        self.table.len()
    }

    /// The domain the input is read over.
    pub(super) fn domain(&self) -> ([f32; 3], [f32; 3]) {
        (self.domain_min, self.domain_max)
    }

    /// The entry at `i`, one value per channel.
    pub(super) fn entry(&self, i: usize) -> [f32; 3] {
        self.table[i]
    }

    /// Each channel of `rgb` through its curve, clamped into the domain.
    pub(super) fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = self.table.len() - 1;
        std::array::from_fn(|c| {
            let unit = ((rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c]))
                .clamp(0.0, 1.0);
            let at = unit * last as f32;
            let index = (at as usize).min(last - 1);
            let within = at - index as f32;
            self.table[index][c] + within * (self.table[index + 1][c] - self.table[index][c])
        })
    }
}

/// The most entries a 1D cube may have, as the specification allows.
const MAX_1D: usize = 65536;

/// The largest side a cube may declare.
const MAX_SIDE: usize = 256;
/// The most colours a cube of that size needs, and so the most that may be collected
/// before anything is checked: bounded here so a file claiming a huge size is refused
/// rather than collected in full.
const MAX_ENTRIES: usize = MAX_SIDE * MAX_SIDE * MAX_SIDE;

/// A number a cube may hold. `f32::from_str` takes "inf" and "NaN", and
/// `DOMAIN_MAX inf` would put every colour at one end of the table, so the whole image
/// would come out one flat value rather than failing.
fn finite(value: f32, line: usize) -> Result<f32> {
    ensure!(
        value.is_finite(),
        "line {line}: {value} is not a number a cube can hold"
    );
    Ok(value)
}

/// The `TITLE` a cube names itself with, kept for a browser to show.
pub(super) fn title_of(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (keyword, rest) = split_keyword(strip_comment(line).trim());
        (keyword == "TITLE").then(|| unquote(rest))
    })
}

impl Cube {
    /// Reads a cube. Comments start at `#` and run to the end of the line.
    pub(super) fn parse(text: &str) -> Result<Self> {
        // A file written on Windows often starts with a UTF-8 byte order mark, which
        // would otherwise land on the first keyword and be read as a data line.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut size = None;
        let mut domain_min: Option<[f32; 3]> = None;
        let mut domain_max: Option<[f32; 3]> = None;
        let mut input_range: Option<[f32; 2]> = None;
        let mut input_range_1d: Option<[f32; 2]> = None;
        let mut data: Vec<[f32; 3]> = Vec::new();
        let mut size_1d: Option<usize> = None;
        for (number, line) in text.lines().enumerate() {
            let at = number + 1;
            let line = strip_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            let (keyword, rest) = split_keyword(line);
            match keyword {
                "TITLE" => {}
                "LUT_3D_SIZE" => size = Some(one_usize(rest, at)?),
                "LUT_1D_SIZE" => size_1d = Some(one_usize(rest, at)?),
                "LUT_1D_INPUT_RANGE" => input_range_1d = Some(two_f32(rest, at)?),
                "DOMAIN_MIN" => domain_min = Some(three_f32(rest, at)?),
                "DOMAIN_MAX" => domain_max = Some(three_f32(rest, at)?),
                "LUT_3D_INPUT_RANGE" => input_range = Some(two_f32(rest, at)?),
                // Not honoured: no tool has been named that writes it, and guessing at
                // a range would be worse than declining.
                "LUT_3D_OUTPUT_RANGE" => bail!(
                    "line {at}: LUT_3D_OUTPUT_RANGE is not supported; the table's \
         answers are read as they are"
                ),
                // Anything else is a data line: three floats, in the order written.
                _ => {
                    // Bounded before anything else looks at it, so a file claiming a
                    // huge size is refused rather than collected in full.
                    ensure!(
                        data.len() < MAX_ENTRIES,
                        "the cube has more than {MAX_ENTRIES} colours, which is more \
         than a size of {MAX_SIDE} can need"
                    );
                    data.push(three_f32(rest, at)?);
                }
            }
        }

        let Some(size) = size else {
            let Some(size) = size_1d else {
                bail!("no LUT_3D_SIZE or LUT_1D_SIZE in the cube");
            };
            return one_dimensional(
                size,
                data,
                input_range_1d,
                input_range,
                domain_min,
                domain_max,
            );
        };
        let (domain_min, domain_max) = domain(domain_min, domain_max, input_range)?;
        ensure!(
            size >= 2,
            "LUT_3D_SIZE of {size} is too small to interpolate between"
        );
        ensure!(
            size <= MAX_SIDE,
            "LUT_3D_SIZE of {size} is larger than {MAX_SIDE}"
        );
        let expected = size
            .checked_mul(size)
            .and_then(|n| n.checked_mul(size))
            .ok_or_else(|| anyhow::anyhow!("LUT_3D_SIZE of {size} overflows"))?;
        // A file may carry a 1D table in front of the 3D one, its shaper. Its data
        // comes first, one line per entry, and is kept rather than dropped: dropping it
        // would render wrong colours that look plausible.
        let shaper = match size_1d {
            None => None,
            Some(one) => {
                ensure!(
                    one >= 2,
                    "LUT_1D_SIZE of {one} is too small to interpolate between"
                );
                ensure!(
                    data.len() == one + expected,
                    "a cube of size {size} with a {one}-entry shaper needs {} colours, \
         but this one has {}",
                    one + expected,
                    data.len()
                );
                let table = data.drain(..one).collect();
                let range = input_range_1d.unwrap_or([0.0, 1.0]);
                ensure!(
                    range[1] > range[0],
                    "LUT_1D_INPUT_RANGE of {:?} is not a range",
                    range
                );
                Some(Shaper {
                    size: one,
                    input_range: range,
                    table,
                })
            }
        };
        ensure!(
            data.len() == expected,
            "a cube of size {size} needs {expected} colours, but this one has {}",
            data.len()
        );
        Ok(Self::Three(Lut3d {
            size,
            domain_min,
            domain_max,
            shaper,
            data,
        }))
    }
}

/// The input range a cube covers. DOMAIN_MIN/MAX and an input range keyword
/// (`LUT_3D_INPUT_RANGE`, or `LUT_1D_INPUT_RANGE` in a 1D cube) are two ways of
/// saying it, not two stages to compose: composing them would cancel the domain out
/// entirely. DOMAIN wins, and the other is read only when it is all there is. Saying
/// so beats picking one silently.
fn domain(
    domain_min: Option<[f32; 3]>,
    domain_max: Option<[f32; 3]>,
    input_range: Option<[f32; 2]>,
) -> Result<([f32; 3], [f32; 3])> {
    let (domain_min, domain_max) = match (domain_min, domain_max, input_range) {
        (Some(low), Some(high), other) => {
            if let Some([input_low, input_high]) = other
                && (low != [input_low; 3] || high != [input_high; 3])
            {
                bail!(
                    "DOMAIN_MIN {low:?} / DOMAIN_MAX {high:?} and the input range \
         {input_low} {input_high} say different input ranges"
                );
            }
            (low, high)
        }
        (None, None, Some([low, high])) => ([low; 3], [high; 3]),
        (None, None, None) => ([0.0; 3], [1.0; 3]),
        _ => bail!("DOMAIN_MIN and DOMAIN_MAX go together, or neither does"),
    };
    for channel in 0..3 {
        ensure!(
            domain_max[channel] > domain_min[channel],
            "DOMAIN_MAX {domain_max:?} is not above DOMAIN_MIN {domain_min:?}"
        );
    }
    Ok((domain_min, domain_max))
}

/// A 1D cube: `size` entries, nothing else. In a 1D cube `LUT_1D_INPUT_RANGE` is the
/// input range, as `LUT_3D_INPUT_RANGE` is in a 3D one, which this one has no use for.
fn one_dimensional(
    size: usize,
    data: Vec<[f32; 3]>,
    input_range_1d: Option<[f32; 2]>,
    input_range_3d: Option<[f32; 2]>,
    domain_min: Option<[f32; 3]>,
    domain_max: Option<[f32; 3]>,
) -> Result<Cube> {
    ensure!(
        input_range_3d.is_none(),
        "a 1D cube has no 3D table for LUT_3D_INPUT_RANGE to describe"
    );
    ensure!(
        size >= 2,
        "LUT_1D_SIZE of {size} is too small to interpolate between"
    );
    ensure!(
        size <= MAX_1D,
        "LUT_1D_SIZE of {size} is larger than {MAX_1D}"
    );
    ensure!(
        data.len() == size,
        "a 1D cube of size {size} needs {size} entries, but this one has {}",
        data.len()
    );
    let (domain_min, domain_max) = domain(domain_min, domain_max, input_range_1d)?;
    Ok(Cube::One(Lut1d {
        domain_min,
        domain_max,
        table: data,
    }))
}

impl Lut3d {
    /// The cube's divisions per side.
    pub(super) fn size(&self) -> usize {
        self.size
    }

    /// The cube's domain, which its input colours are read over.
    pub(super) fn domain(&self) -> ([f32; 3], [f32; 3]) {
        (self.domain_min, self.domain_max)
    }

    /// Whether a 1D shaper table goes in front of the 3D one.
    pub(super) fn has_shaper(&self) -> bool {
        self.shaper.is_some()
    }

    /// The input range the shaper reads (`LUT_1D_INPUT_RANGE`), if there is one.
    pub(super) fn shaper_input_range(&self) -> Option<[f32; 2]> {
        self.shaper.as_ref().map(|s| s.input_range)
    }

    pub(super) fn shaper_size(&self) -> Option<usize> {
        self.shaper.as_ref().map(|s| s.table.len())
    }

    /// The colour at a grid node, red index fastest.
    pub(super) fn node(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + g * self.size + b * self.size * self.size]
    }

    /// The looked-up colour, without a strength: `rgb` mapped through the domain,
    /// clamped into it, and interpolated tetrahedrally.
    pub(super) fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let rgb = self.shaper.as_ref().map_or(rgb, |s| s.apply(rgb));
        let n = self.size;
        let last = n - 1;
        let edge = last as f32;
        let fraction: [f32; 3] = std::array::from_fn(|c| {
            let over_domain =
                (rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c]);
            over_domain.clamp(0.0, 1.0) * edge
        });
        // The cell, and the position within it. Clamped so the last node, whose
        // fraction is 1, still has a cell to be interpolated in.
        let base: [usize; 3] = std::array::from_fn(|c| (fraction[c] as usize).min(last - 1));
        let within: [f32; 3] = std::array::from_fn(|c| fraction[c] - base[c] as f32);
        tetrahedral(
            |r, g, b| self.node(base[0] + r, base[1] + g, base[2] + b),
            within,
        )
    }
}

/// The line without its comment. A `#` inside quotes is part of a title rather than
/// the start of a comment, so quotes are respected.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (None, '#') => return &line[..at],
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if open == c => quote = None,
            _ => {}
        }
    }
    line
}

/// The line's first word, as one of [`KEYWORDS`] so the caller can match it by
/// identity, and the rest of the line. Anything else comes back as `("", line)`, which
/// the caller reads as a data line.
fn split_keyword(line: &str) -> (&str, &str) {
    let (keyword, rest) = match line.split_once(char::is_whitespace) {
        Some((keyword, rest)) => (keyword, rest.trim()),
        None => (line, ""),
    };
    KEYWORDS
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(keyword))
        .map_or(("", line), |known| (known, rest))
}

/// The keywords `split_keyword` recognises.
const KEYWORDS: [&str; 8] = [
    "TITLE",
    "LUT_3D_SIZE",
    "LUT_1D_SIZE",
    "LUT_1D_INPUT_RANGE",
    "DOMAIN_MIN",
    "DOMAIN_MAX",
    "LUT_3D_INPUT_RANGE",
    "LUT_3D_OUTPUT_RANGE",
];

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

fn one_usize(rest: &str, line: usize) -> Result<usize> {
    let (value, extra) = match rest.split_once(char::is_whitespace) {
        Some((value, extra)) => (value, extra.trim()),
        None => (rest, ""),
    };
    ensure!(
        extra.is_empty(),
        "line {line}: expected one number after the keyword"
    );
    value
        .parse()
        .map_err(|e| anyhow::anyhow!("line {line}: {value:?} is not a size: {e}"))
}

fn three_f32(rest: &str, line: usize) -> Result<[f32; 3]> {
    let mut values = rest.split_whitespace();
    let mut parsed = [0.0f32; 3];
    for slot in &mut parsed {
        let value = values.next();
        ensure!(value.is_some(), "line {line}: expected three numbers");
        *slot = finite(
            value
                .unwrap_or_default()
                .parse()
                .map_err(|e| anyhow::anyhow!("line {line}: {value:?} is not a number: {e}"))?,
            line,
        )?;
    }
    ensure!(
        values.next().is_none(),
        "line {line}: expected three numbers, got more"
    );
    Ok(parsed)
}

fn two_f32(rest: &str, line: usize) -> Result<[f32; 2]> {
    let mut values = rest.split_whitespace();
    let mut parsed = [0.0f32; 2];
    for slot in &mut parsed {
        let value = values.next();
        ensure!(value.is_some(), "line {line}: expected two numbers");
        *slot = finite(
            value
                .unwrap_or_default()
                .parse()
                .map_err(|e| anyhow::anyhow!("line {line}: {value:?} is not a number: {e}"))?,
            line,
        )?;
    }
    ensure!(
        values.next().is_none(),
        "line {line}: expected two numbers, got more"
    );
    Ok(parsed)
}
/// The reader's tests read 3D cubes alone.
#[cfg(test)]
impl Lut3d {
    fn parse(text: &str) -> Result<Self> {
        match Cube::parse(text)? {
            Cube::Three(lut) => Ok(lut),
            Cube::One(_) => bail!("a 1D cube"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cube of `size` per side whose node `(r, g, b)` is `f(r, g, b)`, written in the
    /// order the specification stores them: red index first.
    fn cube_text(size: usize, f: impl Fn(usize, usize, usize) -> [f32; 3]) -> String {
        let mut text = format!("TITLE \"test\"\nLUT_3D_SIZE {size}\n");
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let [red, green, blue] = f(r, g, b);
                    text += &format!("{red} {green} {blue}\n");
                }
            }
        }
        text
    }

    fn identity(size: usize) -> String {
        let last = (size - 1) as f32;
        cube_text(size, |r, g, b| {
            [r as f32 / last, g as f32 / last, b as f32 / last]
        })
    }

    /// A cube that changes nothing has to change nothing *exactly*, or it is not much
    /// use as the thing everything else is checked against.
    ///
    /// This is exact rather than nearly: the sizes a `.cube` normally uses are 17, 33
    /// and 65, whose `size - 1` are 16, 32 and 64, so every node and every position
    /// within a cell is a dyadic rational that f32 holds without rounding. A cube of
    /// some other size would drift by an ulp or two, which is why the guard below is a
    /// tolerance and this comparison is not.
    #[test]
    fn an_identity_cube_gives_the_colour_back_exactly() {
        for size in [17, 33, 65] {
            let lut = Lut3d::parse(&identity(size)).unwrap();
            let mut worst: f32 = 0.;
            for i in 0..4000u32 {
                // Values that are deliberately not round, so rounding cannot pass by luck.
                let colour = [
                    (i * 37 % 1009) as f32 / 1009.,
                    (i * 91 % 997) as f32 / 997.,
                    (i * 53 % 991) as f32 / 991.,
                ];
                assert_eq!(lut.lookup(colour), colour, "size {size}, {colour:?}");
                worst = worst.max(0.);
            }
            assert_eq!(worst, 0.);
        }
    }

    /// Every node must come back exactly. At a node the position within the cell is
    /// zero, so the interpolation returns the corner itself and nothing rounds.
    #[test]
    fn every_node_comes_back_exactly() {
        let size = 9;
        let lut = Lut3d::parse(&identity(size)).unwrap();
        let last = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let node = [r as f32 / last, g as f32 / last, b as f32 / last];
                    assert_eq!(lut.lookup(node), node, "node {r},{g},{b}");
                }
            }
        }
    }

    /// The red index has to change fastest, or every cube would come out with its
    /// channels swapped. A cube that darkens along red alone catches that.
    #[test]
    fn the_red_index_changes_fastest() {
        let size = 5;
        let lut = Lut3d::parse(&cube_text(size, |r, _g, _b| {
            [r as f32 / (size - 1) as f32, 0., 0.]
        }))
        .unwrap();
        // The last red step is the last node written, and the only one with blue = 1's
        // neighbour at green = 0.
        assert_eq!(lut.node(size - 1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(lut.node(0, 0, 0), [0.0, 0.0, 0.0]);
        assert_eq!(lut.node(0, size - 1, 0), [0.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([0.5, 0.0, 0.0])[0], 0.5);
    }

    /// A cube that doubles red: the answer at any point is twice the input's red, and
    /// the other channels are left alone.
    #[test]
    fn a_cube_can_change_one_channel_alone() {
        let size = 17;
        let lut = Lut3d::parse(&cube_text(size, |r, g, b| {
            let last = (size - 1) as f32;
            [2.0 * r as f32 / last, g as f32 / last, b as f32 / last]
        }))
        .unwrap();
        let out = lut.lookup([0.25, 0.5, 0.75]);
        assert!((out[0] - 0.5).abs() < 1e-5, "{out:?}");
        assert!((out[1] - 0.5).abs() < 1e-5, "{out:?}");
        assert!((out[2] - 0.75).abs() < 1e-5, "{out:?}");
    }

    /// `DOMAIN_MIN`/`DOMAIN_MAX` say what the input range is, so a cube over 0–2 reads
    /// half its range at 1.0, and colours outside it are held at the ends.
    #[test]
    fn the_domain_is_where_the_input_is_read_over() {
        let size = 5;
        let mut text = format!("LUT_3D_SIZE {size}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n");
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    let n = r as f32 / (size - 1) as f32;
                    text += &format!("{n} 0 0\n");
                }
            }
        }
        let lut = Lut3d::parse(&text).unwrap();
        assert_eq!(lut.domain(), ([0.0; 3], [2.0; 3]));
        // Halfway up the domain is the middle of the table.
        let half = lut.lookup([1.0, 0.0, 0.0])[0];
        assert!((half - 0.5).abs() < 1e-5, "{half}");
        // Outside the domain it clamps to the ends rather than extrapolating.
        assert_eq!(lut.lookup([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([2.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([9.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([-9.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    /// A 1D cube is a curve per channel, read as one rather than as a 3D table, which
    /// would produce noise.
    #[test]
    fn a_1d_cube_is_read_as_a_curve_per_channel() {
        let text = "LUT_1D_SIZE 3\n0 0 1\n0.25 0.5 0.5\n1 1 0\n";
        let Cube::One(lut) = Cube::parse(text).unwrap() else {
            panic!("not read as 1D");
        };
        assert_eq!((lut.size(), lut.entry(1)), (3, [0.25, 0.5, 0.5]));
        // Each channel on its own curve, linear between entries.
        assert_eq!(lut.lookup([0.5, 0.5, 0.5]), [0.25, 0.5, 0.5]);
        assert_eq!(lut.lookup([0.75, 0.25, 1.0]), [0.625, 0.25, 0.0]);
        // Held at the ends outside the domain.
        assert_eq!(lut.lookup([-1.0, 2.0, 0.0]), [0.0, 1.0, 1.0]);
    }

    /// In a 1D cube, `LUT_1D_INPUT_RANGE` says the input range as DOMAIN does, and
    /// the two have to agree.
    #[test]
    fn a_1d_cubes_input_range_is_its_domain() {
        let Cube::One(lut) =
            Cube::parse("LUT_1D_SIZE 2\nLUT_1D_INPUT_RANGE 0 2\n0 0 0\n1 1 1\n").unwrap()
        else {
            panic!("not read as 1D");
        };
        assert_eq!(lut.domain(), ([0.0; 3], [2.0; 3]));
        assert_eq!(lut.lookup([1.0; 3]), [0.5; 3]);
        let error = Cube::parse(
            "LUT_1D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\nLUT_1D_INPUT_RANGE 0 2\n0 0 0\n1 1 1\n",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("different input ranges"),
            "{error}"
        );
        let error = Cube::parse("LUT_1D_SIZE 3\n0 0 0\n1 1 1\n").unwrap_err();
        assert!(error.to_string().contains("needs 3"), "{error}");
        let error = Cube::parse("LUT_1D_SIZE 1\n0 0 0\n").unwrap_err();
        assert!(error.to_string().contains("too small"), "{error}");
    }

    /// A cube whose data does not fill its grid has been truncated somewhere, and
    /// interpolating across the gap would be a guess. The message says how many are
    /// missing.
    #[test]
    fn a_short_cube_is_refused_with_the_count() {
        let text = "LUT_3D_SIZE 4\n0 0 0\n1 1 1\n";
        let error = Lut3d::parse(text).unwrap_err().to_string();
        assert!(error.contains("64"), "{error}");
        assert!(error.contains('2'), "{error}");
    }

    #[test]
    fn a_cube_without_a_size_is_refused() {
        let error = Cube::parse("0 0 0\n").unwrap_err().to_string();
        assert!(error.contains("LUT_3D_SIZE"), "{error}");
    }

    #[test]
    fn a_size_too_small_to_interpolate_is_refused() {
        let error = Lut3d::parse("LUT_3D_SIZE 1\n0 0 0\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains('1'), "{error}");
    }

    /// Comments run to the end of the line, keywords are matched whatever their case,
    /// and the title is the quoted value with the quotes off.
    #[test]
    fn comments_and_keyword_case_are_tolerated() {
        let size = 2;
        let text = format!(
            "# a comment\nlut_3d_size {size}\nTITLE \"My Look\"  # trailing comment\n\
             0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
        );
        let lut = Lut3d::parse(&text).unwrap();
        assert_eq!(lut.size(), 2);
        assert_eq!(title_of(&text).as_deref(), Some("My Look"));
    }

    /// `LUT_3D_INPUT_RANGE` is the other way a cube states what input range it covers,
    /// so a cube that uses it instead of DOMAIN_MIN/MAX reads its input over that range.
    #[test]
    fn an_input_range_is_read_instead_of_an_absent_domain() {
        let size = 5;
        let mut text = format!("LUT_3D_SIZE {size}\nLUT_3D_INPUT_RANGE 0 2\n");
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    let n = r as f32 / (size - 1) as f32;
                    text += &format!("{n} 0 0\n");
                }
            }
        }
        let lut = Lut3d::parse(&text).unwrap();
        assert_eq!(lut.domain(), ([0.0; 3], [2.0; 3]));
        let half = lut.lookup([1.0, 0.0, 0.0])[0];
        assert!((half - 0.5).abs() < 1e-5, "{half}");
        // Held at the ends outside it, as with a domain.
        assert_eq!(lut.lookup([0.0, 0.0, 0.0])[0], 0.0);
        assert_eq!(lut.lookup([2.0, 0.0, 0.0])[0], 1.0);
        assert_eq!(lut.lookup([9.0, 0.0, 0.0])[0], 1.0);
    }

    /// Saying the same range two ways is fine when they agree, which is what a cube
    /// written by a tool that emits both will do.
    #[test]
    fn an_input_range_may_repeat_the_domain() {
        let size = 2;
        let text = format!(
            "LUT_3D_SIZE {size}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n\
             LUT_3D_INPUT_RANGE 0 1\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
             0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
        );
        let lut = Lut3d::parse(&text).unwrap();
        assert_eq!(lut.domain(), ([0.0; 3], [1.0; 3]));
    }

    /// Two different input ranges are a real ambiguity — composing them would cancel the
    /// domain out entirely — so it is reported rather than resolved by guessing.
    #[test]
    fn a_disagreeing_input_range_is_refused() {
        let size = 2;
        let text = format!(
            "LUT_3D_SIZE {size}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n\
             LUT_3D_INPUT_RANGE 0 1\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
             0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
        );
        let error = Lut3d::parse(&text).unwrap_err().to_string();
        assert!(error.contains("different input ranges"), "{error}");
    }

    /// Half a domain given is ambiguous in a different way: there is no default for one
    /// key without the other.
    #[test]
    fn half_a_domain_is_refused() {
        let error = Lut3d::parse("LUT_3D_SIZE 2\nDOMAIN_MAX 1 1 1\n0 0 0\n").unwrap_err();
        assert!(error.to_string().contains("go together"), "{error}");
    }

    /// `LUT_3D_OUTPUT_RANGE` says where the table's answers sit, and they are brought
    /// back to 0–1 so a strength blend means what it says. A table written over 0–1023
    /// answers in 0–1 like any other.
    #[test]
    fn an_output_range_is_refused_by_name() {
        let error = Lut3d::parse("LUT_3D_SIZE 2\nLUT_3D_OUTPUT_RANGE 0 1\n0 0 0\n").unwrap_err();
        let error = error.to_string();
        assert!(error.contains("LUT_3D_OUTPUT_RANGE"), "{error}");
    }

    /// A 65³ cube is the largest a `.cube` usually gets, and it has to parse and look
    /// up without caring how big it is.
    #[test]
    fn a_65_cube_parses_and_looks_up() {
        let lut = Lut3d::parse(&identity(65)).unwrap();
        assert_eq!(lut.size(), 65);
        assert_eq!(lut.lookup([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]);
    }
}

#[cfg(test)]
mod real_files {
    //! Files as tools actually write them, rather than as the specification describes
    //! them. Each of these was a defect before it was a test: the reader had only ever
    //! been given clean input.

    use super::*;

    /// A cube of `size` per side whose red index runs fastest, output on the red axis.
    fn cube(size: usize) -> String {
        let mut text = format!("LUT_3D_SIZE {size}\n");
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    text += &format!("{} 0 0\n", r as f32 / (size - 1) as f32);
                }
            }
        }
        text
    }

    /// A file written on Windows often starts with a byte order mark, which would
    /// otherwise land on the first keyword and be read as a data line.
    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_keyword() {
        let lut = Lut3d::parse(&format!("\u{feff}{}", cube(2))).unwrap();
        assert_eq!(lut.size(), 2);
        assert_eq!(lut.lookup([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
    }

    /// A `#` is a comment marker everywhere except inside a title, where it is a
    /// perfectly ordinary character.
    #[test]
    fn a_hash_inside_a_title_is_part_of_the_title() {
        assert_eq!(
            title_of(&format!("TITLE \"Ash #1\"\n{}", cube(2))).as_deref(),
            Some("Ash #1")
        );
        // A comment after it still goes.
        assert_eq!(
            title_of("TITLE \"Ash\"  # a note\n").as_deref(),
            Some("Ash")
        );
    }

    /// Some tools write the domain as one number, meaning all three channels.
    #[test]
    fn a_domain_of_one_number_is_refused() {
        let error = Lut3d::parse("LUT_3D_SIZE 2\nDOMAIN_MIN 0\nDOMAIN_MAX 1\n").unwrap_err();
        assert!(error.to_string().contains("three numbers"), "{error}");
    }

    #[test]
    fn a_1d_table_in_front_of_the_3d_one_is_applied_as_a_shaper() {
        let size = 3;
        let mut text = format!("LUT_1D_SIZE 2\nLUT_3D_SIZE {size}\n");
        // A shaper that sends every input to zero, so where the colour ends up cannot
        // depend on what the 3D table would have said.
        text += "0 0 0\n0 0 0\n";
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    let n = r as f32 / (size - 1) as f32;
                    text += &format!("{n} 0 0\n");
                }
            }
        }
        let lut = Lut3d::parse(&text).unwrap();
        // The shaper was applied, so even white lands on the 3D table's first node.
        // Dropping it would have sent white to the last, which is the plausible wrong
        // colour rather than an obvious failure.
        assert_eq!(lut.lookup([1.0, 1.0, 1.0]), [0.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn an_identity_shaper_leaves_the_3d_table_in_charge() {
        let size = 3;
        let mut text = format!("LUT_1D_SIZE 2\nLUT_3D_SIZE {size}\n");
        text += "0 0 0\n1 1 1\n";
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    let n = r as f32 / (size - 1) as f32;
                    text += &format!("{n} 0 0\n");
                }
            }
        }
        let lut = Lut3d::parse(&text).unwrap();
        assert_eq!(lut.lookup([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(lut.lookup([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_shapers_own_input_range_is_honoured() {
        let size = 3;
        let mut text = format!("LUT_1D_SIZE 2\nLUT_1D_INPUT_RANGE 0 2\nLUT_3D_SIZE {size}\n");
        text += "0 0 0\n1 1 1\n";
        for _b in 0..size {
            for _g in 0..size {
                for r in 0..size {
                    let n = r as f32 / (size - 1) as f32;
                    text += &format!("{n} 0 0\n");
                }
            }
        }
        let lut = Lut3d::parse(&text).unwrap();
        // 1.0 is halfway up a 0-2 range, so the shaper puts it at the middle.
        assert_eq!(lut.lookup([1.0, 1.0, 1.0]), [0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_shaper_of_the_wrong_length_is_refused() {
        let text = "LUT_1D_SIZE 5\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let error = Lut3d::parse(text).unwrap_err().to_string();
        assert!(error.contains("shaper"), "{error}");
    }

    #[test]
    fn values_that_are_not_finite_are_refused() {
        for bad in ["inf", "-inf", "NaN", "infinity"] {
            let data = format!(
                "LUT_3D_SIZE 2\n0 0 0\n{bad} 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
            );
            let error = Lut3d::parse(&data).unwrap_err().to_string();
            assert!(error.contains("not a number"), "data {bad}: {error}");
            let domain = format!("LUT_3D_SIZE 2\nDOMAIN_MAX {bad} {bad} {bad}\n");
            let error = Lut3d::parse(&domain).unwrap_err().to_string();
            assert!(error.contains("not a number"), "domain {bad}: {error}");
        }
    }

    #[test]
    fn data_that_does_not_add_up_is_refused() {
        let text = "LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n0 0 0\n1 1 1\n1 1 1\n";
        let error = Lut3d::parse(text).unwrap_err().to_string();
        assert!(error.contains("needs"), "{error}");
    }

    #[test]
    fn a_1d_cube_alone_is_not_read_as_a_3d_one() {
        assert!(matches!(
            Cube::parse("LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").unwrap(),
            Cube::One(_)
        ));
    }

    /// The shapes clean files already had, kept so the tolerance above stays deliberate
    /// rather than accidental.
    #[test]
    fn ordinary_awkwardness_was_already_handled() {
        // Windows line endings.
        assert!(Lut3d::parse(&cube(2).replace('\n', "\r\n")).is_ok());
        // No newline after the last line.
        assert!(
            Lut3d::parse("LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1")
                .is_ok()
        );
        // Tabs and runs of spaces.
        assert!(
            Lut3d::parse(
                "LUT_3D_SIZE\t2\n  0   0 0 \n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
            )
            .is_ok()
        );
        // Scientific notation.
        assert!(
            Lut3d::parse(
                "LUT_3D_SIZE 2\n0e0 0 0\n1.0e0 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n"
            )
            .is_ok()
        );
    }
}
