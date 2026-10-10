//! A minimal little-endian DNG writer for the synthetic charts and the scene tone
//! stage's probes: one IFD holding an RGGB mosaic as tiles. (RAWmakase rejects three-channel `LinearRaw` DNGs, so the
//! charts are mosaics; flat patch interiors demosaic exactly.) Tiles are lossless
//! JPEG with two interleaved components per row, as Adobe's DNG Converter writes
//! mosaics; LibRaw reads Deflate only for floating-point data.
use super::chart::{Camera, Matrix};

pub const TILE: u32 = 256;

pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    /// Camera values in 0..=1, one per pixel; the mosaic samples one channel of each.
    pub camera: &'a [[f64; 3]],
    pub as_shot_neutral: [f64; 3],
    /// Also write ForwardMatrix tags and a ProfileName, as Adobe's DNG Converter
    /// does; otherwise the file has color matrices only.
    pub profile: bool,
    /// The DefaultBlackRender tag written with the profile.
    pub black_render: BlackRender,
}

/// DefaultBlackRender: `Auto` writes no tag, as the committed charts have.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlackRender {
    Auto,
    /// Tag value 1, as Adobe's camera-matching profiles carry it.
    None,
}

enum Value {
    Byte(Vec<u8>),
    Float(Vec<f32>),
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
}
impl Value {
    fn kind_count(&self) -> (u16, u32) {
        match self {
            Value::Byte(v) => (1, v.len() as u32),
            Value::Float(v) => (11, v.len() as u32),
            Value::Ascii(s) => (2, s.len() as u32 + 1),
            Value::Short(v) => (3, v.len() as u32),
            Value::Long(v) => (4, v.len() as u32),
            Value::Rational(v) => (5, v.len() as u32),
            Value::SRational(v) => (10, v.len() as u32),
        }
    }
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Value::Byte(v) => out.extend(v),
            Value::Float(v) => v.iter().for_each(|x| out.extend(x.to_le_bytes())),
            Value::Ascii(s) => {
                out.extend(s.as_bytes());
                out.push(0);
            }
            Value::Short(v) => v.iter().for_each(|x| out.extend(x.to_le_bytes())),
            Value::Long(v) => v.iter().for_each(|x| out.extend(x.to_le_bytes())),
            Value::Rational(v) => v.iter().for_each(|(n, d)| {
                out.extend(n.to_le_bytes());
                out.extend(d.to_le_bytes());
            }),
            Value::SRational(v) => v.iter().for_each(|(n, d)| {
                out.extend(n.to_le_bytes());
                out.extend(d.to_le_bytes());
            }),
        }
        out
    }
}

const DENOMINATOR: i32 = 1_000_000;
fn srational(m: &Matrix) -> Value {
    Value::SRational(
        m.iter()
            .flatten()
            .map(|v| ((v * DENOMINATOR as f64).round() as i32, DENOMINATOR))
            .collect(),
    )
}

/// Quantises a camera value to the 16-bit white level.
fn sample(v: f64) -> u16 {
    (v.clamp(0., 1.) * 65535.).round() as u16
}

fn tile_samples(width: u32, height: u32, camera: &[[f64; 3]], tx: u32, ty: u32) -> Vec<u16> {
    let mut out = Vec::with_capacity((TILE * TILE) as usize);
    for y in ty * TILE..(ty + 1) * TILE {
        for x in tx * TILE..(tx + 1) * TILE {
            // Tiles past the image edge are padded by repeating the last pixel.
            let (cx, cy) = (x.min(width - 1), y.min(height - 1));
            let p = camera[(cy * width + cx) as usize];
            // RGGB: CFAPattern [0, 1, 1, 2].
            out.push(sample(
                p[[[0, 1], [1, 2]][(y % 2) as usize][(x % 2) as usize]],
            ));
        }
    }
    out
}

/// The mosaic of `camera` values as lossless JPEG tiles.
fn tiles(width: u32, height: u32, camera: &[[f64; 3]]) -> Vec<Vec<u8>> {
    let (across, down) = (width.div_ceil(TILE), height.div_ceil(TILE));
    (0..down)
        .flat_map(|ty| (0..across).map(move |tx| (tx, ty)))
        .map(|(tx, ty)| lossless_jpeg(&tile_samples(width, height, camera, tx, ty), TILE / 2, TILE))
        .collect()
}

pub fn write(image: &Image, camera: &Camera) -> Vec<u8> {
    let tiles = tiles(image.width, image.height, image.camera);
    let mut tags: Vec<(u16, Value)> = vec![
        (254, Value::Long(vec![0])),
        (256, Value::Long(vec![image.width])),
        (257, Value::Long(vec![image.height])),
        (258, Value::Short(vec![16])),
        (259, Value::Short(vec![7])), // lossless JPEG
        (262, Value::Short(vec![32803])),
        (271, Value::Ascii(camera.make.clone())),
        (272, Value::Ascii(camera.model.clone())),
        (274, Value::Short(vec![1])),
        (277, Value::Short(vec![1])),
        (284, Value::Short(vec![1])),
        (305, Value::Ascii("RAWmakase test chart".into())),
        (339, Value::Short(vec![1])),
        (33421, Value::Short(vec![2, 2])),
        (33422, Value::Byte(vec![0, 1, 1, 2])),
        (50706, Value::Byte(vec![1, 4, 0, 0])),
        (50707, Value::Byte(vec![1, 1, 0, 0])),
        (
            50708,
            Value::Ascii(format!("{} {}", camera.make, camera.model)),
        ),
        (50713, Value::Short(vec![1, 1])),
        (50710, Value::Byte(vec![0, 1, 2])),
        (50711, Value::Short(vec![1])),
        (50714, Value::Long(vec![0])),
        (50717, Value::Long(vec![65535])),
        (50721, srational(&camera.color_matrix1)),
        (
            50728,
            Value::Rational(
                image
                    .as_shot_neutral
                    .iter()
                    .map(|v| ((v * DENOMINATOR as f64).round() as u32, DENOMINATOR as u32))
                    .collect(),
            ),
        ),
        (50730, Value::SRational(vec![(0, 1)])),
        (
            50778,
            Value::Short(vec![if camera.color_matrix2.is_some() {
                17
            } else {
                21
            }]),
        ),
    ];
    if let Some(m2) = &camera.color_matrix2 {
        tags.push((50722, srational(m2)));
        tags.push((50779, Value::Short(vec![21])));
    }
    if image.profile {
        tags.push((
            50936,
            Value::Ascii(format!("{} {}", camera.make, camera.model)),
        ));
        let (f1, f2) = camera.forward_matrices();
        tags.push((50964, srational(&f1)));
        if image.black_render == BlackRender::None {
            tags.push((51110, Value::Long(vec![1])));
        }
        if let Some(f2) = f2 {
            tags.push((50965, srational(&f2)));
        }
    }
    assemble(tags, tiles)
}

/// ProPhoto RGB (ROMM) to XYZ, D50 white.
const PROPHOTO_TO_XYZ: Matrix = [
    [0.7976749, 0.1351917, 0.0313534],
    [0.2880402, 0.7118741, 0.0000857],
    [0.0000000, 0.0000000, 0.8252100],
];

/// A scene tone stage probe, as `scripts/corpus/probe_dng.py` writes it: the camera's
/// RGB is linear ProPhoto, with an embedded profile mapping it to XYZ D50 unchanged,
/// neutral As Shot white balance and a linear ProfileToneCurve, so Camera Raw's
/// ProPhoto output is its scene tone stage's. `scene` holds scene values; the sensor
/// holds them divided by 2^`baseline_exposure`, clipped at its white.
pub fn write_probe(width: u32, height: u32, scene: &[[f64; 3]], baseline_exposure: f64) -> Vec<u8> {
    let scale = (-baseline_exposure).exp2();
    let sensor: Vec<[f64; 3]> = scene.iter().map(|p| p.map(|v| v * scale)).collect();
    let tiles = tiles(width, height, &sensor);
    let tags: Vec<(u16, Value)> = vec![
        (254, Value::Long(vec![0])),
        (256, Value::Long(vec![width])),
        (257, Value::Long(vec![height])),
        (258, Value::Short(vec![16])),
        (259, Value::Short(vec![7])), // lossless JPEG
        (262, Value::Short(vec![32803])),
        (271, Value::Ascii("RAWmakase".into())),
        (272, Value::Ascii("Probe".into())),
        (274, Value::Short(vec![1])),
        (277, Value::Short(vec![1])),
        (284, Value::Short(vec![1])),
        (33421, Value::Short(vec![2, 2])),
        (33422, Value::Byte(vec![0, 1, 1, 2])),
        (50706, Value::Byte(vec![1, 4, 0, 0])),
        (50707, Value::Byte(vec![1, 1, 0, 0])),
        (50708, Value::Ascii("RAWmakase Probe".into())),
        (50710, Value::Byte(vec![0, 1, 2])),
        (50711, Value::Short(vec![1])),
        (50714, Value::Long(vec![0])),
        (50717, Value::Long(vec![65535])),
        (50721, srational(&super::chart::invert(&PROPHOTO_TO_XYZ))),
        (50728, Value::Rational(vec![(1, 1); 3])),
        (
            50730,
            Value::SRational(vec![(
                (baseline_exposure * DENOMINATOR as f64).round() as i32,
                DENOMINATOR,
            )]),
        ),
        (50778, Value::Short(vec![23])), // D50
        (50936, Value::Ascii("RAWmakase Probe".into())),
        (50940, Value::Float(vec![0., 0., 1., 1.])), // linear ProfileToneCurve
        (50941, Value::Long(vec![3])),
        (50964, srational(&PROPHOTO_TO_XYZ)),
    ];
    assemble(tags, tiles)
}

/// The file: `tags` plus the tile tags, then `tiles`.
fn assemble(mut tags: Vec<(u16, Value)>, tiles: Vec<Vec<u8>>) -> Vec<u8> {
    tags.extend([
        (322, Value::Long(vec![TILE])),
        (323, Value::Long(vec![TILE])),
        (324, Value::Long(vec![0; tiles.len()])), // patched below
        (
            325,
            Value::Long(tiles.iter().map(|t| t.len() as u32).collect()),
        ),
    ]);
    tags.sort_by_key(|(tag, _)| *tag);

    // Layout: header, IFD, out-of-line values, tiles.
    let ifd_len = 2 + tags.len() * 12 + 4;
    let mut extra = Vec::new();
    let extra_at = 8 + ifd_len;
    let mut entries = Vec::new();
    let mut tile_offsets_at = None;
    for (tag, value) in &tags {
        let (kind, count) = value.kind_count();
        let mut bytes = value.bytes();
        let field = if bytes.len() <= 4 {
            bytes.resize(4, 0);
            bytes
        } else {
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
            let at = (extra_at + extra.len()) as u32;
            if *tag == 324 {
                tile_offsets_at = Some(extra.len());
            }
            extra.extend(&bytes);
            at.to_le_bytes().to_vec()
        };
        entries.push((*tag, kind, count, field));
    }
    if extra.len() % 2 == 1 {
        extra.push(0);
    }
    let mut data_at = (extra_at + extra.len()) as u32;
    let mut offsets = Vec::new();
    for t in &tiles {
        offsets.push(data_at);
        data_at += t.len() as u32 + (t.len() % 2) as u32;
    }
    let offset_bytes: Vec<u8> = offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
    match tile_offsets_at {
        Some(at) => extra[at..at + offset_bytes.len()].copy_from_slice(&offset_bytes),
        None => {
            let e = entries.iter_mut().find(|e| e.0 == 324).unwrap();
            e.3 = offset_bytes;
        }
    }

    let mut out = b"II".to_vec();
    out.extend(42u16.to_le_bytes());
    out.extend(8u32.to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, count, field) in entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(field);
    }
    out.extend(0u32.to_le_bytes());
    out.extend(extra);
    for t in tiles {
        let odd = t.len() % 2 == 1;
        out.extend(t);
        if odd {
            out.push(0);
        }
    }
    out
}

/// Huffman code lengths for difference categories 0–16: short codes for the small
/// differences that flat patches produce. Kraft sum < 1, so no code is all ones.
const CODE_LENGTHS: [u8; 17] = [1, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10];

/// Canonical Huffman codes (value, length) for each category.
fn huffman_codes() -> [(u16, u8); 17] {
    let mut codes = [(0, 0); 17];
    let mut code = 0u16;
    for length in 1..=16u8 {
        for (category, l) in CODE_LENGTHS.iter().enumerate() {
            if *l == length {
                codes[category] = (code, length);
                code += 1;
            }
        }
        code <<= 1;
    }
    codes
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u8,
}
impl Bits {
    fn put(&mut self, value: u32, bits: u8) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let byte = self.acc as u8;
                self.out.push(byte);
                if byte == 0xff {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.put(1, 1);
        }
        self.out
    }
}

/// Encodes `width` × `height` pairs of interleaved 16-bit samples (ITU T.81 process 14,
/// predictor 1: left neighbour of the same component, above at the start of a row).
fn lossless_jpeg(samples: &[u16], width: u32, height: u32) -> Vec<u8> {
    let codes = huffman_codes();
    let mut out = vec![0xff, 0xd8];
    // DHT: table class 0 (DC/lossless), id 0.
    let mut bits = [0u8; 16];
    for l in CODE_LENGTHS {
        bits[l as usize - 1] += 1;
    }
    let dht_len = 2 + 1 + 16 + CODE_LENGTHS.len();
    out.extend([0xff, 0xc4]);
    out.extend((dht_len as u16).to_be_bytes());
    out.push(0x00);
    out.extend(bits);
    let mut values: Vec<u8> = (0..17u8).collect();
    values.sort_by_key(|v| (CODE_LENGTHS[*v as usize], *v));
    out.extend(values);
    // SOF3: 16-bit precision, two components.
    out.extend([0xff, 0xc3, 0, 14, 16]);
    out.extend((height as u16).to_be_bytes());
    out.extend((width as u16).to_be_bytes());
    out.extend([2, 1, 0x11, 0, 2, 0x11, 0]);
    // SOS: both components on table 0, predictor 1, no point transform.
    out.extend([0xff, 0xda, 0, 10, 2, 1, 0x00, 2, 0x00, 1, 0, 0]);
    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let row = (width * 2) as usize;
    for y in 0..height as usize {
        for x in 0..row {
            let predicted = if x >= 2 {
                samples[y * row + x - 2]
            } else if y > 0 {
                samples[(y - 1) * row + x]
            } else {
                1 << 15
            };
            let diff = samples[y * row + x].wrapping_sub(predicted) as i16 as i32;
            let category = if diff == -32768 {
                16
            } else {
                32 - diff.unsigned_abs().leading_zeros()
            };
            let (code, length) = codes[category as usize];
            bits.put(code as u32, length);
            if (1..16).contains(&category) {
                let extra = if diff < 0 { diff - 1 } else { diff } as u32;
                bits.put(extra & ((1 << category) - 1), category as u8);
            }
        }
    }
    out.extend(bits.finish());
    out.extend([0xff, 0xd9]);
    out
}
