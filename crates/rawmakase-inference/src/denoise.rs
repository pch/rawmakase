//! AI Denoise's adapter for RawNIND UtNet2 (Bayer, [`RAWNIND`]): the tiler and the gain
//! fit, on plain slices.
//!
//! The caller packs the photo's mosaic as four planes of an RGGB mosaic ([`Planes`]),
//! picks the order tiles run in ([`Grid::order`]), and gets the model's output back at
//! sensor resolution ([`Output`]), in unbalanced camera RGB at the model's own scale.
//! [`fit_gain`] matches that scale to the mosaic's. Where the photo comes from, how the
//! result is assembled into a camera image, cached and scheduled belongs to the callers.

use rawmakase_cancel::Cancel;

use crate::error::InferenceError;
use crate::manifest::{DenoiseSpec, RAWNIND};

/// Four packed planes of an RGGB mosaic, one after the other: R, G1 (beside R), G2 (below
/// R) and B, each `width × height`, every value (raw − black) / (white − black) clipped to
/// 0..=1 and not white balanced.
#[derive(Clone, Copy)]
pub struct Planes<'a> {
    pub width: usize,
    pub height: usize,
    pub data: &'a [f32],
}

/// The kept core of one tile, in packed pixels, and where the tile's grid cell starts
/// (`left`, `top`): its input runs from `overlap` before that. The first column and row
/// of a grid with an origin also keep the pixels before it, from the tile's context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub left: usize,
    pub top: usize,
}
impl Tile {
    fn overlaps(&self, [x, y, w, h]: [usize; 4]) -> bool {
        self.x < x + w && x < self.x + self.width && self.y < y + h && y < self.y + self.height
    }
}

/// How a photo of `width × height` packed pixels is cut into tiles: cores of
/// `tile − 2 × overlap` packed pixels, each run with `overlap` pixels of mirrored or real
/// context on every side and only its core kept.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub tile: usize,
    pub overlap: usize,
    /// Where the grid's first cell starts (`[x, y]`, 0 or 1): the packed pixel that is
    /// rawnind_jddc's first after its alignment crop, so tiles and the network's pooling
    /// see the mosaic on upstream's grid (docs/rendering-quality.md#ai-denoise).
    pub origin: [usize; 2],
}
impl Grid {
    /// The grid of [`RAWNIND`] for a photo of `width × height` packed pixels.
    pub fn new(width: usize, height: usize) -> Self {
        Self::with(&RAWNIND, width, height)
    }
    pub fn with(spec: &DenoiseSpec, width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            tile: spec.tile,
            overlap: spec.overlap,
            origin: [0, 0],
        }
    }
    /// The same grid starting at `origin` (see [`Grid::origin`]).
    pub fn with_origin(self, origin: [usize; 2]) -> Self {
        Self { origin, ..self }
    }
    fn step(&self) -> usize {
        self.tile - 2 * self.overlap
    }
    /// Every tile, row by row.
    pub fn tiles(&self) -> Vec<Tile> {
        let step = self.step();
        // Cells along one side: (where the core starts, its length, the cell's start).
        let cells = |size: usize, origin: usize| {
            let origin = origin.min(size.saturating_sub(1));
            let mut cells = Vec::new();
            let mut start = origin;
            while start < size || cells.is_empty() {
                let from = if cells.is_empty() { 0 } else { start };
                cells.push((from, (start + step).min(size) - from, start));
                start += step;
            }
            cells
        };
        let mut tiles = Vec::new();
        for (y, height, top) in cells(self.height, self.origin[1]) {
            for &(x, width, left) in &cells(self.width, self.origin[0]) {
                tiles.push(Tile {
                    x,
                    y,
                    width,
                    height,
                    left,
                    top,
                });
            }
        }
        tiles
    }
    /// Every tile, those covering `first` (a packed `[x, y, width, height]`, the region
    /// shown at 100%) before the rest, and how many cover it.
    pub fn order(&self, first: Option<[usize; 4]>) -> (Vec<Tile>, usize) {
        let tiles = self.tiles();
        let Some(region) = first else {
            return (tiles, 0);
        };
        let (mut first, rest): (Vec<_>, Vec<_>) =
            tiles.into_iter().partition(|t| t.overlaps(region));
        let count = first.len();
        first.extend(rest);
        (first, count)
    }
}

/// Runs one tile: `[1, 4, tile, tile]` in, `[1, 3, 2 × tile, 2 × tile]` out. The ONNX
/// model ([`crate::runtime::Denoiser`]) in the app, a stand-in in tests.
pub trait TileModel {
    fn run(&mut self, input: &[f32], cancel: Cancel<'_>) -> Result<Vec<f32>, InferenceError>;
}

/// The model's output at sensor resolution (twice the packed size), three planes (red,
/// green, blue) one after the other, at the model's own scale; zero where no tile ran yet.
pub struct Output {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
    /// Packed pixels some tile has filled, row by row.
    done: Vec<bool>,
}
impl Output {
    /// An empty output for `grid`.
    pub fn new(grid: &Grid) -> Self {
        let (width, height) = (grid.width * 2, grid.height * 2);
        Self {
            width,
            height,
            data: vec![0.; 3 * width * height],
            done: vec![false; grid.width * grid.height],
        }
    }
    /// Whether the packed pixel (`x`, `y`) has been filled.
    pub fn is_done(&self, x: usize, y: usize) -> bool {
        self.done[y * (self.width / 2) + x]
    }
    /// Whether every packed pixel has been filled.
    pub fn is_complete(&self) -> bool {
        self.done.iter().all(|d| *d)
    }
}

/// Index `i` of a dimension of `n` samples mirrored at both ends without repeating the
/// edge sample (NumPy's `reflect`), folding as often as needed.
fn mirror(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let i = i.rem_euclid(period);
    (if i >= n as isize { period - i } else { i }) as usize
}

/// The model's input for `tile`: the tile's core with `overlap` pixels of context, real
/// where the photo has them and mirrored past its edges.
pub fn tile_input(planes: Planes<'_>, grid: &Grid, tile: Tile) -> Vec<f32> {
    let side = grid.tile;
    let plane = planes.width * planes.height;
    let mut input = vec![0.; 4 * side * side];
    let x0 = tile.left as isize - grid.overlap as isize;
    let y0 = tile.top as isize - grid.overlap as isize;
    let columns: Vec<usize> = (0..side)
        .map(|i| mirror(x0 + i as isize, planes.width))
        .collect();
    for c in 0..4 {
        let source = &planes.data[c * plane..(c + 1) * plane];
        for row in 0..side {
            let y = mirror(y0 + row as isize, planes.height);
            let line = &source[y * planes.width..(y + 1) * planes.width];
            let target = &mut input[(c * side + row) * side..(c * side + row + 1) * side];
            for (t, &x) in target.iter_mut().zip(&columns) {
                *t = line[x];
            }
        }
    }
    input
}

/// Runs `tiles` in order through `model`, keeping each tile's core in `out`. After each
/// tile `after(tiles done)` is called; the cancel flag is checked between tiles and the
/// model checks it during one. A tile with a non-finite value fails the run: the caller
/// discards the attempt (and may retry it on another provider).
pub fn run(
    planes: Planes<'_>,
    grid: &Grid,
    tiles: &[Tile],
    model: &mut dyn TileModel,
    out: &mut Output,
    cancel: Cancel<'_>,
    mut after: impl FnMut(usize, &Output),
) -> Result<(), InferenceError> {
    if planes.width != grid.width
        || planes.height != grid.height
        || planes.data.len() != 4 * planes.width * planes.height
        || planes.width == 0
        || planes.height == 0
    {
        return Err(InferenceError::OutputInvalid(
            "the packed mosaic does not match its size".into(),
        ));
    }
    let side = grid.tile;
    let (w, h) = (out.width, out.height);
    for (index, &tile) in tiles.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        let input = tile_input(planes, grid, tile);
        let result = model.run(&input, cancel)?;
        let o = 2 * side;
        if result.len() != 3 * o * o {
            return Err(InferenceError::OutputInvalid(format!(
                "the denoiser answered with {} values, expected {}",
                result.len(),
                3 * o * o
            )));
        }
        if result.iter().any(|v| !v.is_finite()) {
            return Err(InferenceError::OutputInvalid(
                "the denoiser produced a value that is not finite".into(),
            ));
        }
        // The core's offset in the output: `overlap` in, less what the first column and
        // row keep before their cell.
        let sx = 2 * (grid.overlap + tile.x - tile.left);
        let sy = 2 * (grid.overlap + tile.y - tile.top);
        for c in 0..3 {
            for row in 0..2 * tile.height {
                let from = (c * o + sy + row) * o + sx;
                let to = (c * h + 2 * tile.y + row) * w + 2 * tile.x;
                out.data[to..to + 2 * tile.width]
                    .copy_from_slice(&result[from..from + 2 * tile.width]);
            }
        }
        for y in tile.y..tile.y + tile.height {
            let row = y * grid.width;
            out.done[row + tile.x..row + tile.x + tile.width].fill(true);
        }
        after(index + 1, out);
    }
    Ok(())
}

/// The mosaic the gain is fitted against: per packed pixel, red, the mean of the two
/// greens and blue, (raw − black) / (white − black) without clipping and not white
/// balanced (the half-size demosaic divided by the white balance), and whether any of its
/// four samples is clipped.
pub struct GainReference<'a> {
    pub width: usize,
    pub height: usize,
    pub rgb: &'a [[f32; 3]],
    pub clipped: &'a [bool],
}

/// Packed pixels per side of the blocks the gain is fitted on.
const GAIN_BLOCK: usize = 8;
/// A block counts when every channel's mean reaches the first of these levels (of sensor
/// white) that leaves [`GAIN_MIN_BLOCKS`] blocks: the darkest blocks are left out, as the
/// model, fed a mosaic clipped at black, lifts their means
/// (docs/rendering-quality.md#ai-denoise).
const GAIN_LEVELS: [f32; 3] = [0.03, 0.01, 0.003];
const GAIN_MIN_BLOCKS: usize = 64;

/// The one scalar that brings the model's `out`put to the mosaic's scale: least squares
/// over blocks of [`GAIN_BLOCK`] packed pixels that tiles have filled, with no clipped
/// sample and no channel too dark. A fit with too few blocks, or not finite and positive,
/// is an error with the reason.
pub fn fit_gain(out: &Output, reference: &GainReference<'_>) -> Result<f32, String> {
    let (pw, ph) = (out.width / 2, out.height / 2);
    if reference.width != pw
        || reference.height != ph
        || reference.rgb.len() != pw * ph
        || reference.clipped.len() != pw * ph
    {
        return Err("the gain reference does not match the output".into());
    }
    let plane = out.width * out.height;
    let mut blocks: Vec<([f64; 3], [f64; 3])> = Vec::new();
    for by in (0..ph / GAIN_BLOCK).map(|b| b * GAIN_BLOCK) {
        'block: for bx in (0..pw / GAIN_BLOCK).map(|b| b * GAIN_BLOCK) {
            let mut r = [0f64; 3];
            let mut o = [0f64; 3];
            for y in by..by + GAIN_BLOCK {
                for x in bx..bx + GAIN_BLOCK {
                    let i = y * pw + x;
                    if reference.clipped[i] || !out.is_done(x, y) {
                        continue 'block;
                    }
                    for c in 0..3 {
                        r[c] += f64::from(reference.rgb[i][c]);
                        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            o[c] += f64::from(
                                out.data[c * plane + (2 * y + dy) * out.width + 2 * x + dx],
                            ) / 4.;
                        }
                    }
                }
            }
            let n = (GAIN_BLOCK * GAIN_BLOCK) as f64;
            blocks.push((r.map(|v| v / n), o.map(|v| v / n)));
        }
    }
    for level in GAIN_LEVELS {
        let level = f64::from(level);
        let used: Vec<_> = blocks
            .iter()
            .filter(|(r, _)| r.iter().all(|v| *v >= level))
            .collect();
        if used.len() < GAIN_MIN_BLOCKS {
            continue;
        }
        let (num, den) = used.iter().fold((0., 0.), |(num, den), (r, o)| {
            (
                num + (0..3).map(|c| r[c] * o[c]).sum::<f64>(),
                den + (0..3).map(|c| o[c] * o[c]).sum::<f64>(),
            )
        });
        let gain = (num / den) as f32;
        return if gain.is_finite() && gain > 0. {
            Ok(gain)
        } else {
            Err(format!(
                "the denoiser's scale could not be matched (gain {gain})"
            ))
        };
    }
    Err(
        "the photo has too little unclipped detail above black to match the denoiser's scale"
            .into(),
    )
}

/// Packed pixels per side of the blocks the dark correction compares means over.
const DARK_BLOCK: usize = 16;
/// Block means (the darkest channel's, of sensor white) below which the correction
/// applies in full, and above which not at all.
const DARK_FULL: f32 = 0.01;
const DARK_NONE: f32 = 0.03;

/// Brings the model's deep shadows back to the mosaic's mean. Fed a mosaic clipped at
/// black, the model lifts the means of the darkest areas, by up to 2.5 EV below 0.5% of
/// white at ISO 200,000 (docs/rendering-quality.md#ai-denoise); the mosaic's own mean there,
/// over blocks of [`DARK_BLOCK`] packed pixels, is unbiased (development keeps negative
/// samples). Each channel of a dark block is scaled down to the mosaic's mean, never up;
/// the scale is interpolated between block centres, so no block edge shows, and fades out
/// between [`DARK_FULL`] and [`DARK_NONE`]. `rgb` is the output with its gain applied.
pub fn correct_dark(rgb: &mut [f32], reference: &GainReference<'_>) {
    let (pw, ph) = (reference.width, reference.height);
    let (w, h) = (2 * pw, 2 * ph);
    if rgb.len() != 3 * w * h || pw == 0 || ph == 0 {
        return;
    }
    let (bw, bh) = (pw.div_ceil(DARK_BLOCK), ph.div_ceil(DARK_BLOCK));
    let plane = w * h;
    // Per block and channel: the scale toward the mosaic's mean.
    let mut scale = vec![[1f32; 3]; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (x0, y0) = (bx * DARK_BLOCK, by * DARK_BLOCK);
            let (x1, y1) = ((x0 + DARK_BLOCK).min(pw), (y0 + DARK_BLOCK).min(ph));
            let mut r = [0f64; 3];
            let mut o = [0f64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let reference = reference.rgb[y * pw + x];
                    for c in 0..3 {
                        r[c] += f64::from(reference[c]);
                        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            o[c] += f64::from(rgb[c * plane + (2 * y + dy) * w + 2 * x + dx]) / 4.;
                        }
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let r = r.map(|v| (v / n).max(0.) as f32);
            let o = o.map(|v| (v / n) as f32);
            let level = r.iter().copied().fold(f32::INFINITY, f32::min);
            let weight = ((DARK_NONE - level) / (DARK_NONE - DARK_FULL)).clamp(0., 1.);
            if weight == 0. {
                continue;
            }
            scale[by * bw + bx] = std::array::from_fn(|c| {
                let s = if o[c] > 0. { (r[c] / o[c]).min(1.) } else { 1. };
                1. + weight * (s - 1.)
            });
        }
    }
    if scale.iter().all(|s| *s == [1.; 3]) {
        return;
    }
    // Interpolated between block centres, in sensor pixels.
    let side = (2 * DARK_BLOCK) as f32;
    let at = |bx: isize, by: isize| {
        scale[(by.clamp(0, bh as isize - 1) as usize) * bw + bx.clamp(0, bw as isize - 1) as usize]
    };
    for y in 0..h {
        let fy = (y as f32 + 0.5) / side - 0.5;
        let by = fy.floor();
        let ty = fy - by;
        for x in 0..w {
            let fx = (x as f32 + 0.5) / side - 0.5;
            let bx = fx.floor();
            let tx = fx - bx;
            let (bx, by) = (bx as isize, by as isize);
            let (a, b, c, d) = (
                at(bx, by),
                at(bx + 1, by),
                at(bx, by + 1),
                at(bx + 1, by + 1),
            );
            for k in 0..3 {
                let s = (a[k] * (1. - tx) + b[k] * tx) * (1. - ty)
                    + (c[k] * (1. - tx) + d[k] * tx) * ty;
                if s != 1. {
                    rgb[k * plane + y * w + x] *= s;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A stand-in for the network: each packed pixel's four samples placed at sensor
    /// resolution as camera RGB (R; the mean of the greens; B), scaled by `scale`. It
    /// ignores the context, so stitching tiles must equal one run over the whole photo.
    struct Nearest {
        side: usize,
        scale: f32,
        runs: usize,
        poison_after: Option<usize>,
    }
    impl TileModel for Nearest {
        fn run(&mut self, input: &[f32], cancel: Cancel<'_>) -> Result<Vec<f32>, InferenceError> {
            if cancel.is_cancelled() {
                return Err(InferenceError::Cancelled);
            }
            self.runs += 1;
            let s = self.side;
            let o = 2 * s;
            let mut out = vec![0.; 3 * o * o];
            for y in 0..s {
                for x in 0..s {
                    let p = |c: usize| input[(c * s + y) * s + x];
                    let rgb = [p(0), (p(1) + p(2)) / 2., p(3)];
                    for (c, v) in rgb.into_iter().enumerate() {
                        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            out[(c * o + 2 * y + dy) * o + 2 * x + dx] = v * self.scale;
                        }
                    }
                }
            }
            if self.poison_after.is_some_and(|n| self.runs > n) {
                out[7] = f32::NAN;
            }
            Ok(out)
        }
    }

    fn planes(width: usize, height: usize) -> Vec<f32> {
        (0..4 * width * height)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000. * 0.8 + 0.1)
            .collect()
    }

    fn small_grid(width: usize, height: usize) -> Grid {
        Grid {
            width,
            height,
            tile: 16,
            overlap: 4,
            origin: [0, 0],
        }
    }

    #[test]
    fn mirror_matches_numpy_reflect() {
        // np.pad(np.arange(4), 6, mode="reflect")
        let expected: Vec<usize> = vec![0, 1, 2, 3, 2, 1, 0, 1, 2, 3, 2, 1, 0, 1, 2, 3];
        let got: Vec<usize> = (-6..10).map(|i| mirror(i, 4)).collect();
        assert_eq!(got, expected);
        assert_eq!(mirror(0, 1), 0);
        assert_eq!(mirror(-3, 1), 0);
    }

    fn whole(model: &mut Nearest, width: usize, height: usize, data: &[f32]) -> Vec<f32> {
        // One tile holding the whole photo: the reference stitching must reproduce.
        let side = width.max(height);
        let grid = Grid {
            width,
            height,
            tile: side,
            overlap: 0,
            origin: [0, 0],
        };
        let mut out = Output::new(&grid);
        let planes = Planes {
            width,
            height,
            data,
        };
        let mut big = Nearest {
            side,
            scale: model.scale,
            runs: 0,
            poison_after: None,
        };
        run(
            planes,
            &grid,
            &grid.tiles(),
            &mut big,
            &mut out,
            Cancel::NEVER,
            |_, _| {},
        )
        .unwrap();
        out.data
    }

    #[test]
    fn stitched_tiles_equal_one_run_at_any_size_and_order() {
        for (width, height) in [(8, 8), (13, 9), (40, 23), (1, 17)] {
            let data = planes(width, height);
            let grid = small_grid(width, height);
            let mut model = Nearest {
                side: 16,
                scale: 3.,
                runs: 0,
                poison_after: None,
            };
            let expected = whole(&mut model, width, height, &data);
            for region in [None, Some([width / 2, height / 2, 1, 1])] {
                let (tiles, first) = grid.order(region);
                assert_eq!(tiles.len(), grid.tiles().len());
                assert_eq!(first, usize::from(region.is_some()));
                let mut out = Output::new(&grid);
                let mut calls = Vec::new();
                run(
                    Planes {
                        width,
                        height,
                        data: &data,
                    },
                    &grid,
                    &tiles,
                    &mut model,
                    &mut out,
                    Cancel::NEVER,
                    |done, out| calls.push((done, out.is_complete())),
                )
                .unwrap();
                assert_eq!(out.data, expected, "{width}x{height} {region:?}");
                assert!(out.is_complete());
                assert_eq!(calls.last(), Some(&(tiles.len(), true)));
            }
        }
    }

    #[test]
    fn tiles_cover_the_photo_once_and_the_region_runs_first() {
        let grid = small_grid(40, 23);
        let tiles = grid.tiles();
        let mut hits = vec![0; 40 * 23];
        for t in &tiles {
            for y in t.y..t.y + t.height {
                for x in t.x..t.x + t.width {
                    hits[y * 40 + x] += 1;
                }
            }
        }
        assert!(hits.iter().all(|h| *h == 1));
        let (ordered, first) = grid.order(Some([9, 9, 8, 1]));
        assert_eq!(first, 2);
        assert!(
            ordered[..2]
                .iter()
                .all(|t| t.y == 8 && (t.x == 8 || t.x == 16))
        );
    }

    #[test]
    fn input_mirrors_past_the_edges() {
        let (width, height) = (5, 3);
        let data: Vec<f32> = (0..4 * width * height).map(|v| v as f32).collect();
        let grid = Grid {
            width,
            height,
            tile: 8,
            overlap: 2,
            origin: [0, 0],
        };
        let input = tile_input(
            Planes {
                width,
                height,
                data: &data,
            },
            &grid,
            grid.tiles()[0],
        );
        // Plane 0, first row of the tile is photo row mirror(-2) = 2, columns mirror(-2..6).
        let row: Vec<f32> = input[..8].to_vec();
        let cols = [2, 1, 0, 1, 2, 3, 4, 3];
        assert_eq!(row, cols.map(|x| (2 * width + x) as f32).to_vec());
        // Plane 3 starts at 3 * width * height.
        assert_eq!(input[3 * 64 + 2 * 8 + 2], (3 * width * height) as f32);
    }

    #[test]
    fn cancel_between_and_during_tiles_and_non_finite_output_fail() {
        let (width, height) = (40, 23);
        let data = planes(width, height);
        let grid = small_grid(width, height);
        let planes = Planes {
            width,
            height,
            data: &data,
        };
        let cancel = AtomicBool::new(false);
        let mut model = Nearest {
            side: 16,
            scale: 1.,
            runs: 0,
            poison_after: None,
        };
        let mut out = Output::new(&grid);
        let result = run(
            planes,
            &grid,
            &grid.tiles(),
            &mut model,
            &mut out,
            Cancel::new(&cancel),
            |done, _| {
                if done == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result, Err(InferenceError::Cancelled));
        assert_eq!(model.runs, 2);
        assert!(!out.is_complete());

        let mut poisoned = Nearest {
            side: 16,
            scale: 1.,
            runs: 0,
            poison_after: Some(3),
        };
        let mut out = Output::new(&grid);
        let result = run(
            planes,
            &grid,
            &grid.tiles(),
            &mut poisoned,
            &mut out,
            Cancel::NEVER,
            |_, _| {},
        );
        assert!(matches!(result, Err(InferenceError::OutputInvalid(_))));
        assert_eq!(poisoned.runs, 4);
    }

    fn reference_of(width: usize, height: usize, data: &[f32]) -> (Vec<[f32; 3]>, Vec<bool>) {
        let plane = width * height;
        let rgb = (0..plane)
            .map(|i| {
                [
                    data[i],
                    (data[plane + i] + data[2 * plane + i]) / 2.,
                    data[3 * plane + i],
                ]
            })
            .collect();
        (rgb, vec![false; plane])
    }

    #[test]
    fn gain_matches_a_known_scale_and_provisional_gain_agrees() {
        let (width, height) = (96, 80);
        let data = planes(width, height);
        let grid = small_grid(width, height);
        let (rgb, mut clipped) = reference_of(width, height, &data);
        let mut model = Nearest {
            side: 16,
            scale: 1. / 4.5e-6,
            runs: 0,
            poison_after: None,
        };
        let mut out = Output::new(&grid);
        let mut provisional = None;
        let (tiles, _) = grid.order(None);
        let total = tiles.len();
        let reference = GainReference {
            width,
            height,
            rgb: &rgb,
            clipped: &clipped,
        };
        run(
            Planes {
                width,
                height,
                data: &data,
            },
            &grid,
            &tiles,
            &mut model,
            &mut out,
            Cancel::NEVER,
            |done, out| {
                if done == total * 3 / 4 {
                    provisional = Some(fit_gain(out, &reference));
                }
            },
        )
        .unwrap();
        let gain = fit_gain(&out, &reference).unwrap();
        assert!((gain / 4.5e-6 - 1.).abs() < 1e-4, "{gain}");
        let provisional = provisional.unwrap().unwrap();
        assert!((provisional / gain - 1.).abs() < 1e-4);
        // Clipped blocks are left out; with every block clipped the fit fails with a reason.
        clipped.fill(true);
        let reference = GainReference {
            width,
            height,
            rgb: &rgb,
            clipped: &clipped,
        };
        assert!(
            fit_gain(&out, &reference)
                .unwrap_err()
                .contains("too little")
        );
    }

    #[test]
    fn gain_skips_dark_blocks_and_rejects_degenerate_fits() {
        let (width, height) = (160, 80);
        let mut data = planes(width, height);
        // A black photo: nothing to fit.
        let black = vec![0.; data.len()];
        let grid = small_grid(width, height);
        let mut model = Nearest {
            side: 16,
            scale: 1.,
            runs: 0,
            poison_after: None,
        };
        let mut out = Output::new(&grid);
        fn planes_of(data: &[f32]) -> Planes<'_> {
            Planes {
                width: 160,
                height: 80,
                data,
            }
        }
        run(
            planes_of(&black),
            &grid,
            &grid.tiles(),
            &mut model,
            &mut out,
            Cancel::NEVER,
            |_, _| {},
        )
        .unwrap();
        let (rgb, clipped) = reference_of(width, height, &black);
        let reference = GainReference {
            width,
            height,
            rgb: &rgb,
            clipped: &clipped,
        };
        assert!(fit_gain(&out, &reference).is_err());
        // The left half of the photo lifted by the model where it is dark (as the network
        // lifts means of a mosaic clipped at black): those blocks do not move the gain.
        let plane = width * height;
        for c in 0..4 {
            for y in 0..height {
                for x in 0..width / 2 {
                    data[c * plane + y * width + x] = 0.002;
                }
            }
        }
        let mut out = Output::new(&grid);
        run(
            planes_of(&data),
            &grid,
            &grid.tiles(),
            &mut model,
            &mut out,
            Cancel::NEVER,
            |_, _| {},
        )
        .unwrap();
        for c in 0..3 {
            for y in 0..out.height {
                for x in 0..out.width / 2 {
                    out.data[c * out.width * out.height + y * out.width + x] *= 2.;
                }
            }
        }
        let (rgb, clipped) = reference_of(width, height, &data);
        let reference = GainReference {
            width,
            height,
            rgb: &rgb,
            clipped: &clipped,
        };
        let gain = fit_gain(&out, &reference).unwrap();
        assert!((gain - 1.).abs() < 1e-5, "{gain}");
    }

    #[test]
    fn dark_correction_brings_lifted_shadows_to_the_mosaics_mean_and_leaves_the_rest() {
        let (pw, ph) = (64, 32);
        // Left half dark (0.004 of white), right half mid grey.
        let rgb_ref: Vec<[f32; 3]> = (0..pw * ph)
            .map(|i| {
                if i % pw < pw / 2 {
                    [0.004; 3]
                } else {
                    [0.2; 3]
                }
            })
            .collect();
        let clipped = vec![false; pw * ph];
        let reference = GainReference {
            width: pw,
            height: ph,
            rgb: &rgb_ref,
            clipped: &clipped,
        };
        // The model lifted the dark half by 1 EV.
        let (w, h) = (2 * pw, 2 * ph);
        let mut rgb = vec![0f32; 3 * w * h];
        for c in 0..3 {
            for y in 0..h {
                for x in 0..w {
                    rgb[c * w * h + y * w + x] = if x < w / 2 { 0.008 } else { 0.2 };
                }
            }
        }
        correct_dark(&mut rgb, &reference);
        // Deep in the dark half: the mosaic's mean; deep in the bright half: untouched.
        assert!(
            (rgb[h / 2 * w + 4] - 0.004).abs() < 1e-6,
            "{}",
            rgb[h / 2 * w + 4]
        );
        assert_eq!(rgb[h / 2 * w + w - 4], 0.2);
        // Never brighter than the model made it.
        assert!(rgb.iter().all(|v| *v <= 0.2 + 1e-6));
    }

    /// A grid starting one packed pixel in (a mirrored row or column before the mosaic)
    /// keeps upstream's cells: cores from the origin on, the pixels before it kept from
    /// the first tile's context; every pixel once, stitched as one run.
    #[test]
    fn a_grid_with_an_origin_keeps_upstreams_cells_and_covers_everything() {
        let (width, height) = (40, 23);
        let data = planes(width, height);
        let grid = small_grid(width, height).with_origin([1, 1]);
        let tiles = grid.tiles();
        let mut hits = vec![0; width * height];
        for t in &tiles {
            assert!((t.left - 1) % 8 == 0 && (t.top - 1) % 8 == 0, "{t:?}");
            for y in t.y..t.y + t.height {
                for x in t.x..t.x + t.width {
                    hits[y * width + x] += 1;
                }
            }
        }
        assert!(hits.iter().all(|h| *h == 1));
        let mut model = Nearest {
            side: 16,
            scale: 2.,
            runs: 0,
            poison_after: None,
        };
        let expected = whole(&mut model, width, height, &data);
        let mut out = Output::new(&grid);
        run(
            Planes {
                width,
                height,
                data: &data,
            },
            &grid,
            &tiles,
            &mut model,
            &mut out,
            Cancel::NEVER,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(out.data, expected);
    }
}
