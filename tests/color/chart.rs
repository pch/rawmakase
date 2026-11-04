//! Synthetic color charts: a fixed patch layout, filled with scene colors and
//! converted to camera RGB with a DNG ColorMatrix.
//!
//! Scene colors are defined in linear sRGB (D65) and converted to XYZ. For charts
//! lit by another illuminant, colors are adapted to that white with Bradford, so a
//! white patch stays neutral to the camera after white balance. Camera values are
//! `0.8 × ColorMatrix × XYZ`: middle gray renders near mid-tone without any
//! BaselineExposure, bright patches stay below clipping, and the top two steps of
//! the gray ramp clip.
use serde::{Deserialize, Serialize};

pub(crate) const COLUMNS: u32 = 24;
pub(crate) const PATCH: u32 = 32;
const GAP: u32 = 6;
const MARGIN: u32 = 32;
const ROWS: u32 = 18;
const CELL: u32 = PATCH + GAP;
pub(crate) const WIDTH: u32 = 2 * MARGIN + COLUMNS * CELL - GAP;
pub(crate) const HEIGHT: u32 = 2 * MARGIN + ROWS * CELL - GAP;
/// Linear value of diffuse white relative to the white level.
const WHITE_LEVEL_SCALE: f64 = 0.8;
const BACKGROUND: [f64; 3] = [0.09; 3];

/// One measured area. `x, y, w, h` is the area that is averaged: the inner half of
/// each patch, or a narrow slice of a sweep.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Patch {
    pub name: String,
    pub group: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A rectangle of the chart filled with one scene color, or a horizontal sweep.
enum Fill {
    Flat([f64; 3]),
    Sweep(fn(f64) -> [f64; 3]),
}
struct Area {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    fill: Fill,
}

pub struct Layout {
    areas: Vec<Area>,
    pub patches: Vec<Patch>,
}

fn cell(column: u32, row: u32) -> (u32, u32) {
    (MARGIN + column * CELL, MARGIN + row * CELL)
}

pub(crate) fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// OKLCh (Björn Ottosson's OKLab) to linear sRGB. Values may fall outside [0, 1].
pub(crate) fn oklch(l: f64, c: f64, h_degrees: f64) -> [f64; 3] {
    let (a, b) = (
        c * h_degrees.to_radians().cos(),
        c * h_degrees.to_radians().sin(),
    );
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
    ]
}

/// The largest OKLCh chroma at this lightness and hue that stays inside sRGB.
fn max_chroma(l: f64, h: f64) -> f64 {
    let (mut lo, mut hi) = (0., 0.5);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.;
        if oklch(l, mid, h)
            .iter()
            .all(|v| (-1e-9..=1. + 1e-9).contains(v))
        {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Published sRGB approximations of the 24 ColorChecker patches (BabelColor averages).
const COLORCHECKER: [u32; 24] = [
    0x735244, 0xc29682, 0x627a9d, 0x576c43, 0x8580b1, 0x67bdaa, 0xd67e2c, 0x505ba6, 0xc15a63,
    0x5e3c6c, 0x9dbc40, 0xe0a32e, 0x383d96, 0x469449, 0xaf363c, 0xe7c71f, 0xbb5695, 0x0885a1,
    0xf3f3f2, 0xc8c8c8, 0xa0a0a0, 0x7a7a79, 0x555555, 0x343434,
];

fn hue_sweep(t: f64) -> [f64; 3] {
    let h = t * 360.;
    oklch(0.7, 0.8 * max_chroma(0.7, h), h)
}
fn lightness_sweep(t: f64) -> [f64; 3] {
    [0.18 * (2f64).powf(t * 11.5 - 8.); 3]
}

impl Layout {
    pub fn new() -> Self {
        let mut layout = Layout {
            areas: Vec::new(),
            patches: Vec::new(),
        };
        // Row 0: gray ramp, −8 to +3.5 EV around middle gray in ½ EV steps.
        for i in 0..COLUMNS {
            let ev = -8. + 0.5 * i as f64;
            layout.flat(
                i,
                0,
                format!("gray {ev:+.1} EV"),
                "ramp",
                [0.18 * 2f64.powf(ev); 3],
            );
        }
        // Rows 1–9: 24 hues × 3 lightness × 30/60/90 % of the largest sRGB chroma.
        for (li, l) in [0.45, 0.65, 0.85].into_iter().enumerate() {
            for (ci, fraction) in [0.3, 0.6, 0.9].into_iter().enumerate() {
                for i in 0..COLUMNS {
                    let h = 15. * i as f64;
                    let c = fraction * max_chroma(l, h);
                    let row = 1 + 3 * li as u32 + ci as u32;
                    let name = format!("hue {h:.0} L{:.0} C{:.0}%", l * 100., fraction * 100.);
                    layout.flat(i, row, name, "hue", oklch(l, c, h));
                }
            }
        }
        // Row 10: beyond sRGB, 1.4× the largest sRGB chroma at L 0.65.
        for i in 0..COLUMNS {
            let h = 15. * i as f64;
            let rgb = oklch(0.65, 1.4 * max_chroma(0.65, h), h);
            layout.flat(i, 10, format!("wide hue {h:.0}"), "wide", rgb);
        }
        // Row 11: ColorChecker.
        for (i, hex) in COLORCHECKER.into_iter().enumerate() {
            let rgb = [16, 8, 0].map(|s| srgb_to_linear(((hex >> s) & 0xff) as f64 / 255.));
            layout.flat(
                i as u32,
                11,
                format!("colorchecker {}", i + 1),
                "colorchecker",
                rgb,
            );
        }
        // Row 12: skin tones, near-neutrals, white/gray/black.
        for i in 0..12 {
            let t = i as f64 / 11.;
            let rgb = oklch(
                0.88 - 0.55 * t,
                0.045 + 0.03 * (std::f64::consts::PI * t).sin(),
                55. - 8. * t,
            );
            layout.flat(i, 12, format!("skin {}", i + 1), "skin", rgb);
        }
        let offsets = [
            (0., 0.),
            (0.01, 0.),
            (-0.01, 0.),
            (0., 0.01),
            (0., -0.01),
            (0.02, 0.),
            (-0.02, 0.),
            (0., 0.02),
            (0., -0.02),
        ];
        for (i, (da, db)) in offsets.into_iter().enumerate() {
            let rgb = oklch(0.62, f64::hypot(da, db), f64::atan2(db, da).to_degrees());
            layout.flat(
                12 + i as u32,
                12,
                format!("neutral a{da:+.2} b{db:+.2}"),
                "neutral",
                rgb,
            );
        }
        for (i, (name, y)) in [("white", 0.9), ("gray", 0.18), ("black", 0.003)]
            .into_iter()
            .enumerate()
        {
            layout.flat(21 + i as u32, 12, name.into(), "neutral", [y; 3]);
        }
        // Rows 13–14: smooth sweeps, measured in 24 narrow slices.
        for (row, name, f) in [
            (13, "hue sweep", hue_sweep as fn(f64) -> [f64; 3]),
            (14, "lightness sweep", lightness_sweep),
        ] {
            let (x, y) = cell(0, row);
            let w = WIDTH - 2 * MARGIN;
            layout.areas.push(Area {
                x,
                y,
                w,
                h: PATCH,
                fill: Fill::Sweep(f),
            });
            for i in 0..COLUMNS {
                let (cx, _) = cell(i, row);
                layout.patches.push(Patch {
                    name: format!("{name} {}", i + 1),
                    group: "sweep".into(),
                    x: cx + PATCH / 2 - 2,
                    y: y + PATCH / 4,
                    w: 4,
                    h: PATCH / 2,
                });
            }
        }
        // Rows 15–17: the same four colors on a black and on a white surround.
        let colors = [
            [0.18; 3],
            oklch(0.6, 0.12, 30.),
            oklch(0.6, 0.12, 150.),
            oklch(0.6, 0.12, 260.),
        ];
        for (side, (surround, level)) in [("black", 0.004), ("white", 0.85)].into_iter().enumerate()
        {
            let (x, y) = cell(12 * side as u32, 15);
            layout.areas.push(Area {
                x,
                y,
                w: 12 * CELL - GAP,
                h: 3 * CELL - GAP,
                fill: Fill::Flat([level; 3]),
            });
            for (i, rgb) in colors.into_iter().enumerate() {
                let (px, py) = cell(12 * side as u32 + 1 + 3 * i as u32, 16);
                layout.flat_at(
                    px,
                    py,
                    format!("{surround} surround {}", i + 1),
                    "surround",
                    rgb,
                );
            }
        }
        layout
    }

    fn flat(&mut self, column: u32, row: u32, name: String, group: &str, rgb: [f64; 3]) {
        let (x, y) = cell(column, row);
        self.flat_at(x, y, name, group, rgb);
    }

    fn flat_at(&mut self, x: u32, y: u32, name: String, group: &str, rgb: [f64; 3]) {
        self.areas.push(Area {
            x,
            y,
            w: PATCH,
            h: PATCH,
            fill: Fill::Flat(rgb),
        });
        self.patches.push(Patch {
            name,
            group: group.into(),
            x: x + PATCH / 4,
            y: y + PATCH / 4,
            w: PATCH / 2,
            h: PATCH / 2,
        });
    }

    /// Scene colors as linear sRGB (D65), row by row.
    pub fn scene(&self) -> Vec<[f64; 3]> {
        let mut pixels = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
        for a in &self.areas {
            for y in a.y..a.y + a.h {
                for x in a.x..a.x + a.w {
                    pixels[(y * WIDTH + x) as usize] = match a.fill {
                        Fill::Flat(rgb) => rgb,
                        Fill::Sweep(f) => f((x - a.x) as f64 / (a.w - 1) as f64),
                    };
                }
            }
        }
        pixels
    }
}

pub(crate) type Matrix = [[f64; 3]; 3];

pub(crate) fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}
pub(crate) fn apply(m: &Matrix, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| (0..3).map(|k| m[r][k] * v[k]).sum())
}
pub(crate) fn invert(m: &Matrix) -> Matrix {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    std::array::from_fn(|r| {
        std::array::from_fn(|c| {
            let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
            let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
            (m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]) / det
        })
    })
}

pub(crate) const SRGB_TO_XYZ: Matrix = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175],
    [0.019_333_9, 0.119_192, 0.950_304_1],
];

/// White points (XYZ, Y = 1) and correlated color temperatures.
#[derive(Clone, Copy, Debug)]
pub enum Illuminant {
    D65,
    D50,
    A,
    F2,
}
impl Illuminant {
    pub fn white(self) -> [f64; 3] {
        let (x, y) = match self {
            Illuminant::D65 => (0.312_71, 0.329_02),
            Illuminant::D50 => (0.345_67, 0.358_50),
            Illuminant::A => (0.447_57, 0.407_45),
            Illuminant::F2 => (0.372_08, 0.375_29),
        };
        [x / y, 1., (1. - x - y) / y]
    }
    pub fn temperature(self) -> f64 {
        match self {
            Illuminant::D65 => 6504.,
            Illuminant::D50 => 5003.,
            Illuminant::A => 2856.,
            Illuminant::F2 => 4230.,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Illuminant::D65 => "d65",
            Illuminant::D50 => "d50",
            Illuminant::A => "a",
            Illuminant::F2 => "f2",
        }
    }
}

const BRADFORD: Matrix = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Bradford adaptation between two whites.
pub(crate) fn adapt(from: [f64; 3], to: [f64; 3]) -> Matrix {
    let src = apply(&BRADFORD, from);
    let dst = apply(&BRADFORD, to);
    let scale: Matrix =
        std::array::from_fn(|r| std::array::from_fn(|c| if r == c { dst[r] / src[r] } else { 0. }));
    mul(&invert(&BRADFORD), &mul(&scale, &BRADFORD))
}

/// A camera as a DNG describes it: one or two XYZ-to-camera matrices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Camera {
    /// File name stem, e.g. `fujifilm-x100f`.
    pub id: String,
    pub make: String,
    pub model: String,
    /// ColorMatrix1, calibrated for Standard light A when `color_matrix2` is present,
    /// otherwise for D65.
    pub color_matrix1: Matrix,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_matrix2: Option<Matrix>,
}

impl Camera {
    /// An invented camera with broad, overlapping primaries and two calibrations,
    /// so the charts carry no manufacturer or Adobe data.
    pub fn synthetic() -> Self {
        fn from_primaries(p: [(f64, f64); 3], white: [f64; 3]) -> Matrix {
            let columns: Matrix = std::array::from_fn(|r| {
                std::array::from_fn(|c| {
                    let (x, y) = p[c];
                    [x / y, 1., (1. - x - y) / y][r]
                })
            });
            let s = apply(&invert(&columns), white);
            let rgb_to_xyz: Matrix =
                std::array::from_fn(|r| std::array::from_fn(|c| columns[r][c] * s[c]));
            // Unequal channel sensitivities, so white balance has real work to do.
            let gains = [0.55, 1., 0.72];
            let xyz_to_camera = invert(&rgb_to_xyz);
            std::array::from_fn(|r| xyz_to_camera[r].map(|v| v * gains[r]))
        }
        let d65 = from_primaries(
            [(0.70, 0.29), (0.21, 0.76), (0.135, 0.045)],
            Illuminant::D65.white(),
        );
        // A real sensor is not an exact linear transform of XYZ, so the matrices
        // fitted under tungsten and daylight differ a little.
        let tilt: Matrix = [[1.04, -0.03, -0.01], [-0.02, 1.01, 0.01], [0., -0.03, 1.03]];
        let a = mul(&tilt, &d65);
        Camera {
            id: "synthetic".into(),
            make: "RAWmakase".into(),
            model: "Synthetic Chart".into(),
            color_matrix1: a,
            color_matrix2: Some(d65),
        }
        .normalised()
    }

    /// Scales each matrix so D50 maps to a camera neutral whose largest value is 1,
    /// as the DNG SDK does when it reads a ColorMatrix. Charts are generated with the
    /// normalised matrices, so readers recover the intended XYZ exactly.
    pub fn normalised(mut self) -> Self {
        let d50 = Illuminant::D50.white();
        self.color_matrix1 = normalise(self.color_matrix1, d50);
        self.color_matrix2 = self.color_matrix2.map(|m| normalise(m, d50));
        self
    }

    /// ForwardMatrix1/2 consistent with the color matrices: white-balanced camera
    /// RGB to XYZ (D50), mapping camera neutral to the D50 white.
    pub fn forward_matrices(&self) -> (Matrix, Option<Matrix>) {
        let forward = |m: &Matrix, illuminant: Illuminant| {
            let white = illuminant.white();
            let n = apply(m, white);
            let diag: Matrix =
                std::array::from_fn(|r| std::array::from_fn(|c| if r == c { n[r] } else { 0. }));
            mul(
                &adapt(white, Illuminant::D50.white()),
                &mul(&invert(m), &diag),
            )
        };
        match self.color_matrix2 {
            Some(m2) => (
                forward(&self.color_matrix1, Illuminant::A),
                Some(forward(&m2, Illuminant::D65)),
            ),
            None => (forward(&self.color_matrix1, Illuminant::D65), None),
        }
    }

    /// The XYZ-to-camera matrix for a white of this temperature, interpolated
    /// linearly in inverse temperature as the DNG specification describes.
    pub fn matrix_for(&self, temperature: f64) -> Matrix {
        let Some(m2) = self.color_matrix2 else {
            return self.color_matrix1;
        };
        let (t1, t2) = (Illuminant::A.temperature(), Illuminant::D65.temperature());
        let g = ((1. / temperature - 1. / t2) / (1. / t1 - 1. / t2)).clamp(0., 1.);
        std::array::from_fn(|r| {
            std::array::from_fn(|c| g * self.color_matrix1[r][c] + (1. - g) * m2[r][c])
        })
    }
}

/// Scales a matrix so the given white maps to a camera neutral whose largest value is 1.
fn normalise(m: Matrix, white: [f64; 3]) -> Matrix {
    let n = apply(&m, white);
    let max = n.into_iter().fold(0., f64::max);
    m.map(|row| row.map(|v| v / max))
}

/// A chart ready to write: camera values in 0..=1 and the neutral for As Shot WB.
pub(crate) struct Rendered {
    pub camera: Vec<[f64; 3]>,
    pub as_shot_neutral: [f64; 3],
}

pub(crate) fn render(layout: &Layout, camera: &Camera, illuminant: Illuminant) -> Rendered {
    let white = illuminant.white();
    let m = camera.matrix_for(illuminant.temperature());
    let to_camera = mul(
        &m,
        &mul(&adapt(Illuminant::D65.white(), white), &SRGB_TO_XYZ),
    );
    let neutral = apply(&m, white);
    let max = neutral.into_iter().fold(0., f64::max);
    let camera = layout
        .scene()
        .into_iter()
        .map(|rgb| apply(&to_camera, rgb).map(|v| (v * WHITE_LEVEL_SCALE).clamp(0., 1.)))
        .collect();
    Rendered {
        camera,
        as_shot_neutral: neutral.map(|v| v / max),
    }
}
