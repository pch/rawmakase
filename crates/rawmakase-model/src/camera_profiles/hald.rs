//! HaldCLUT images: a 3D lookup table stored as a PNG, the format RawTherapee's
//! Film Simulation and G'MIC read. A HaldCLUT of level `n` is a square image
//! `n³` pixels on a side holding a table of `n²` divisions per side. Its pixels,
//! read row by row, are the table's nodes with the red index changing fastest, then
//! green, then blue, which is the order a `.cube` keeps its data in, so pixel `i` is
//! node `r + g·n² + b·n⁴`. The values are the image's encoded values, conventionally
//! sRGB: RawTherapee applies them unconverted, and it only remembers a colour space
//! that a file name ends with. See <http://www.quelsolaar.com/technology/clut.html>.
//!
//! A HaldCLUT read here is an ordinary [`Lut3d`] and is imported exactly as a `.cube`
//! is (`cube_look`).
use super::cube::Lut3d;
use anyhow::{Context, Result, bail, ensure};

/// The most a HaldCLUT file may hold. Level 16, the largest read, is 4096 pixels on
/// a side; RawTherapee's level-12 and level-16 film files are 1–4 MB.
pub(super) const MAX_BYTES: u64 = 64_000_000;

/// The levels read. Level 16 has 256 divisions, the most a `.cube` may have
/// (`cube::MAX_SIDE`); level 1 has a single node.
const LEVELS: std::ops::RangeInclusive<usize> = 2..=16;

/// Whether `bytes` are a PNG of a HaldCLUT's size, read from its header alone.
pub(super) fn has_hald_size(bytes: &[u8]) -> bool {
    decoder(bytes)
        .read_info()
        .is_ok_and(|reader| level(reader.info()).is_ok())
}

/// A decoder for a HaldCLUT's PNG. Palettes become RGB and bit depths under 8 become
/// 8; 16 bits stay 16. Its own allocations are limited to what the largest level at
/// 16-bit RGBA needs; the image's buffer is [`read`]'s, sized from the header after
/// [`level`] has bounded it.
fn decoder(bytes: &[u8]) -> png::Decoder<std::io::Cursor<&[u8]>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let side = LEVELS.end().pow(3);
    decoder.set_limits(png::Limits {
        bytes: side * side * 8,
    });
    decoder
}

/// The HaldCLUT level an image of the header's size holds, or why it holds none.
fn level(info: &png::Info) -> Result<usize> {
    let (width, height) = (info.width as usize, info.height as usize);
    ensure!(
        width == height,
        "not a HaldCLUT: the image is {width}×{height}, not square"
    );
    let Some(level) = LEVELS.clone().find(|n| n.pow(3) == width) else {
        bail!(
            "not a HaldCLUT: {width}×{width} is not a HaldCLUT's size (8³ = 512 or \
             12³ = 1728 pixels on a side, for example)"
        );
    };
    Ok(level)
}

/// Reads a HaldCLUT PNG: 8- or 16-bit, RGB or grayscale, with or without alpha, or a
/// palette. Alpha is ignored, and a grayscale image is a table whose output is gray.
pub(super) fn read(bytes: &[u8]) -> Result<Lut3d> {
    let mut reader = decoder(bytes)
        .read_info()
        .context("not a PNG image RAWmakase can read")?;
    // Checked from the header, before the image is decoded.
    let level = level(reader.info())?;
    let size = level * level;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .context("the image is too large")?
    ];
    let frame = reader.next_frame(&mut buffer)?;
    let channels = frame.color_type.samples();
    let wide = frame.bit_depth == png::BitDepth::Sixteen;
    let sample = |pixel: &[u8], channel: usize| -> f32 {
        if wide {
            f32::from(u16::from_be_bytes([
                pixel[2 * channel],
                pixel[2 * channel + 1],
            ])) / 65535.
        } else {
            f32::from(pixel[channel]) / 255.
        }
    };
    let gray = matches!(
        frame.color_type,
        png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha
    );
    let data: Vec<[f32; 3]> = buffer[..frame.buffer_size()]
        .chunks_exact(channels * if wide { 2 } else { 1 })
        .map(|pixel| {
            if gray {
                [sample(pixel, 0); 3]
            } else {
                std::array::from_fn(|c| sample(pixel, c))
            }
        })
        .collect();
    ensure!(
        data.len() == size.pow(3),
        "the image holds {} pixels, not the {} a level-{level} HaldCLUT has",
        data.len(),
        size.pow(3)
    );
    Ok(Lut3d::from_nodes(size, data))
}

/// Whether a table read from an image looks like a lookup table rather than a photo
/// that happens to have a HaldCLUT's size, such as a pack's 512×512 sample image.
///
/// Along each axis of a lookup table each output channel mostly rises or mostly
/// falls, so a channel's total variation along a line of nodes is close to the
/// change from one end of the line to the other. In a photo read as a table, nodes
/// a step apart along green or blue are pixels a row segment or several rows apart,
/// and the variation along a line is many times the net change. The ratio of the two,
/// summed over every line and channel, is 1 for the identity.
pub(super) fn looks_like_lut(lut: &Lut3d) -> bool {
    variation(lut) <= MAX_VARIATION
}

/// The total variation along every line of nodes as a multiple of the net change
/// ([`looks_like_lut`]), or infinity for a table that changes nowhere.
fn variation(lut: &Lut3d) -> f64 {
    let n = lut.size();
    let node = |axis: usize, line: usize, i: usize| {
        let (a, b) = (line % n, line / n);
        match axis {
            0 => lut.node(i, a, b),
            1 => lut.node(a, i, b),
            _ => lut.node(a, b, i),
        }
    };
    let (mut variation, mut net) = (0f64, 0f64);
    for axis in 0..3 {
        for line in 0..n * n {
            for c in 0..3 {
                let mut previous = node(axis, line, 0)[c];
                for i in 1..n {
                    let value = node(axis, line, i)[c];
                    variation += f64::from((value - previous).abs());
                    previous = value;
                }
                net += f64::from((previous - node(axis, line, 0)[c]).abs());
            }
        }
    }
    if net > 0. {
        variation / net
    } else {
        f64::INFINITY
    }
}

/// The most total variation [`looks_like_lut`] accepts, as a multiple of the net
/// change. RawTherapee's pack of 269 film HaldCLUTs measures 1.0–1.98, ordinary photos
/// scaled to 512 and 1728 pixels on a side 5.7 and above.
const MAX_VARIATION: f64 = 3.;

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// How [`hald_png`] stores its pixels.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Encoding {
        Rgb8,
        Rgb16,
        Rgba8,
        /// The table's red output as the gray.
        Gray8,
        Gray16,
        /// The table's red output as the gray, with alpha.
        GrayAlpha8,
        /// The table's colours through a palette, which holds them only when the
        /// level is small enough for every node to have its own entry.
        Palette,
    }

    /// The PNG of a level-`level` HaldCLUT whose node at `(r, g, b)` (0–1) is `f` of
    /// it.
    pub(crate) fn hald_png(
        level: u32,
        f: impl Fn([f32; 3]) -> [f32; 3],
        encoding: Encoding,
    ) -> Vec<u8> {
        let side = level.pow(3);
        let size = (level * level) as usize;
        let last = (size - 1) as f32;
        let nodes: Vec<[f32; 3]> = (0..size.pow(3))
            .map(|i| f([i % size, i / size % size, i / size / size].map(|v| v as f32 / last)))
            .collect();
        let eight = |v: f32| (v * 255.).round() as u8;
        let sixteen = |v: f32| ((v * 65535.).round() as u16).to_be_bytes();
        let (color, depth, data): (_, _, Vec<u8>) = match encoding {
            Encoding::Rgb8 => (
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                nodes.iter().flat_map(|p| p.map(eight)).collect(),
            ),
            Encoding::Rgb16 => (
                png::ColorType::Rgb,
                png::BitDepth::Sixteen,
                nodes
                    .iter()
                    .flat_map(|p| p.map(sixteen))
                    .flatten()
                    .collect(),
            ),
            Encoding::Rgba8 => (
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                nodes
                    .iter()
                    .flat_map(|p| [eight(p[0]), eight(p[1]), eight(p[2]), 128])
                    .collect(),
            ),
            Encoding::Gray8 => (
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                nodes.iter().map(|p| eight(p[0])).collect(),
            ),
            Encoding::GrayAlpha8 => (
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Eight,
                nodes.iter().flat_map(|p| [eight(p[0]), 200]).collect(),
            ),
            Encoding::Gray16 => (
                png::ColorType::Grayscale,
                png::BitDepth::Sixteen,
                nodes.iter().flat_map(|p| sixteen(p[0])).collect(),
            ),
            Encoding::Palette => (
                png::ColorType::Indexed,
                png::BitDepth::Eight,
                (0..nodes.len()).map(|i| i as u8).collect(),
            ),
        };
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, side, side);
            encoder.set_color(color);
            encoder.set_depth(depth);
            if let Encoding::Palette = encoding {
                assert!(nodes.len() <= 256);
                encoder.set_palette(nodes.iter().flat_map(|p| p.map(eight)).collect::<Vec<_>>());
            }
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&data)
                .unwrap();
        }
        bytes
    }

    /// PNG's CRC-32, for patching a chunk.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A creative grade with channels crossing, so a wrong node order shows.
    fn grade([r, g, b]: [f32; 3]) -> [f32; 3] {
        [
            0.1 + 0.8 * r * r,
            0.3 * r + 0.6 * g,
            0.2 + 0.5 * b + 0.2 * g,
        ]
    }

    fn assert_nodes(lut: &Lut3d, f: impl Fn([f32; 3]) -> [f32; 3], tolerance: f32) {
        let n = lut.size();
        let last = (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let want = f([r, g, b].map(|v| v as f32 / last));
                    let got = lut.node(r, g, b);
                    for c in 0..3 {
                        assert!(
                            (got[c] - want[c]).abs() <= tolerance,
                            "{r} {g} {b}: {got:?} {want:?}"
                        );
                    }
                }
            }
        }
    }

    /// Pixel `i` is node `r + g·n² + b·n⁴`, red fastest, at each bit depth and with
    /// alpha ignored; a level of `n` has `n²` divisions.
    #[test]
    fn pixels_are_nodes_with_red_fastest() {
        for (level, encoding, tolerance) in [
            (2, Encoding::Rgb8, 0.5 / 255.),
            (3, Encoding::Rgb16, 0.5 / 65535.),
            (4, Encoding::Rgba8, 0.5 / 255.),
        ] {
            let lut = read(&hald_png(level, grade, encoding)).unwrap();
            assert_eq!(lut.size(), (level * level) as usize, "{encoding:?}");
            assert_nodes(&lut, grade, tolerance + 1e-6);
        }
    }

    /// A grayscale HaldCLUT, as RawTherapee's black-and-white films are, is a table
    /// whose output is that gray in every channel.
    #[test]
    fn grayscale_is_gray_in_every_channel() {
        let gray = |p: [f32; 3]| [0.3 * p[0] + 0.6 * p[1] + 0.1 * p[2]; 3];
        for (encoding, tolerance) in [
            (Encoding::Gray8, 0.5 / 255.),
            (Encoding::Gray16, 0.5 / 65535.),
            (Encoding::GrayAlpha8, 0.5 / 255.),
        ] {
            assert_nodes(
                &read(&hald_png(3, gray, encoding)).unwrap(),
                gray,
                tolerance + 1e-6,
            );
        }
    }

    /// A palette image is expanded to its colours.
    #[test]
    fn a_palette_is_expanded() {
        assert_nodes(
            &read(&hald_png(2, grade, Encoding::Palette)).unwrap(),
            grade,
            0.5 / 255. + 1e-6,
        );
    }

    /// Images that are not square, or square but not a level's cube of a side, are
    /// refused from their header.
    #[test]
    fn other_sizes_are_refused() {
        for (width, height) in [(64, 8), (10, 10), (1, 1), (4097, 4097)] {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, width, height);
                encoder.set_color(png::ColorType::Grayscale);
                let mut writer = encoder.write_header().unwrap();
                writer
                    .write_image_data(&vec![0; (width * height) as usize])
                    .unwrap();
            }
            let error = read(&bytes).unwrap_err();
            assert!(
                format!("{error:#}").contains("not a HaldCLUT"),
                "{width}×{height}: {error:#}"
            );
        }
        assert!(read(b"not a png").is_err());
        assert!(!has_hald_size(b"not a png"));
    }

    /// The size is checked from the header, before the image is decoded: a header
    /// claiming a huge image, with the data of a small one after it, is refused for
    /// its size, and one of a HaldCLUT's size whose data ends early fails to decode
    /// rather than panicking.
    #[test]
    fn the_header_is_checked_before_decoding() {
        let complete = hald_png(3, grade, Encoding::Rgb8);
        assert!(has_hald_size(&complete));
        // The IHDR chunk follows the 8-byte signature: length, type, 13 bytes of data
        // starting with the width and height, then the CRC of type and data.
        let mut huge = complete.clone();
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = crc32(&huge[12..29]);
        huge[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(!has_hald_size(&huge));
        let error = read(&huge).unwrap_err();
        assert!(
            format!("{error:#}").contains("not a HaldCLUT's size"),
            "{error:#}"
        );
        let cut = &complete[..complete.len() / 2];
        assert!(has_hald_size(cut));
        assert!(read(cut).is_err());
    }

    /// Lookup tables pass the check, the identity and a sharp grade with clipping
    /// among them; a photo-like image of a HaldCLUT's size and a flat one do not.
    /// The film pack RawTherapee links measured at most 1.98, and photos scaled to
    /// 512 and 1728 pixels at least 5.7.
    #[test]
    fn the_content_check_tells_tables_from_photos() {
        let clip =
            |[r, g, b]: [f32; 3]| [(1.4 * r - 0.1).clamp(0., 1.), (g * g * 1.2).min(1.), 1. - b];
        for f in [&(|p| p) as &dyn Fn([f32; 3]) -> [f32; 3], &grade, &clip] {
            let lut = read(&hald_png(8, f, Encoding::Rgb8)).unwrap();
            assert!(looks_like_lut(&lut), "{}", variation(&lut));
        }
        // A smooth scene, row by row: a gradient sky with textured ground below.
        let side = 512.;
        let photo = |[r, g, b]: [f32; 3]| {
            // Back to the pixel the node is.
            let i = (r * 63.).round() + (g * 63.).round() * 64. + (b * 63.).round() * 4096.;
            let (x, y) = (i % side / side, (i / side).floor() / side);
            let texture = 0.15 * ((x * 90.).sin() * (y * 70.).cos());
            if y < 0.5 {
                [0.4 + 0.3 * y, 0.6 + 0.2 * y, 0.9]
            } else {
                [0.3 + texture, 0.35 + 0.5 * texture, 0.2]
            }
        };
        let image = read(&hald_png(8, photo, Encoding::Rgb8)).unwrap();
        assert!(!looks_like_lut(&image), "{}", variation(&image));
        let flat = read(&hald_png(2, |_| [0.5; 3], Encoding::Rgb8)).unwrap();
        assert!(!looks_like_lut(&flat));
    }
}
