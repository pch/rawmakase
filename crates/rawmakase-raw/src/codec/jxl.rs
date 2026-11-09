//! JPEG XL codestreams (ISO/IEC 18181-1) as DNG 1.7 stores tiles (compression
//! 52546), decoded with jxl-oxide: one frame of one or three channels,
//! integer or floating-point, lossless (modular) or lossy (VarDCT in XYB),
//! a lossy one's samples linear or on sRGB's curve.
use crate::error::{Error, Result, corrupt};
use jxl_oxide::color::{ColourSpace, Customxy, Primaries, TransferFunction, WhitePoint};
use jxl_oxide::image::BitDepth;
use jxl_oxide::{AllocTracker, EnumColourEncoding, InitializeResult, JxlImage, RenderingIntent};

/// A decoded tile: the requested channels interleaved, `width` pixels a
/// row, on the samples' own scale (integers 0 to 2^bits − 1, floats as
/// stored).
pub(crate) struct Tile {
    pub width: usize,
    pub samples: Vec<f32>,
}

/// The most a tile's decode may allocate per sample of the tile it fills:
/// jxl-oxide's buffers for one frame, with room for its filters; and at
/// least `MIN_BYTES`, for its fixed costs on small tiles.
const BYTES_PER_SAMPLE: usize = 64;
const MIN_BYTES: usize = 16 << 20;

/// Decodes a tile of at most `max` (width, height) pixels of `channels`
/// colour channels. A lossy tile is coded in XYB; it is converted to the
/// codestream's RGB here, by matrix in linear light and then onto its curve
/// (linear, or sRGB's), so values above 1 (an HDR merge's) keep their scale
/// and colours outside sRGB theirs: jxl-oxide's own conversion would clip or
/// gamut-map them into 0–1.
pub(crate) fn decode(data: &[u8], max: [usize; 2], channels: usize) -> Result<Tile> {
    let limit = max[0]
        .saturating_mul(max[1])
        .saturating_mul(channels)
        .saturating_mul(BYTES_PER_SAMPLE)
        .max(MIN_BYTES);
    // Adobe's tiles say 16-bit Modular buffers suffice and then hold LF
    // coefficients past them (an HDR merge's sun); libjxl always decodes with
    // 32-bit buffers, and so does this.
    let mut uninit = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(limit))
        .force_wide_buffers(true)
        .build_uninit();
    let jxl =
        |e: Box<dyn std::error::Error + Send + Sync>| corrupt(format!("DNG JPEG XL tile: {e}"));
    let used = uninit.feed_bytes(data).map_err(jxl)?;
    let mut image = match uninit.try_init().map_err(jxl)? {
        InitializeResult::Initialized(image) => image,
        InitializeResult::NeedMoreData(_) => return Err(corrupt("DNG JPEG XL tile header")),
    };
    let (width, height) = (image.width() as usize, image.height() as usize);
    if width == 0 || height == 0 || width > max[0] || height > max[1] {
        return Err(corrupt("DNG JPEG XL tile size"));
    }
    let metadata = &image.image_header().metadata;
    let scale = match metadata.bit_depth {
        BitDepth::IntegerSample { bits_per_sample } if (1..=16).contains(&bits_per_sample) => {
            ((1u32 << bits_per_sample) - 1) as f32
        }
        BitDepth::FloatSample { .. } => 1.,
        _ => return Err(Error::Unsupported("DNG JPEG XL tile bit depth".into())),
    };
    let grey = metadata.grayscale();
    if usize::from(!grey) * 2 + 1 != channels {
        return Err(corrupt("DNG JPEG XL tile channels"));
    }
    let to_rgb = if metadata.xyb_encoded {
        let jxl_oxide::color::ColourEncoding::Enum(encoding) = &metadata.colour_encoding else {
            return Err(Error::Unsupported(
                "DNG JPEG XL tile with an ICC profile".into(),
            ));
        };
        // The samples are what the encoder was given, on the curve the
        // codestream names: linear (an HDR merge's) or sRGB's (a panorama's).
        let curve = match encoding.tf {
            TransferFunction::Linear => false,
            TransferFunction::Srgb => true,
            tf => {
                return Err(Error::Unsupported(format!(
                    "DNG JPEG XL tile in {:?} with the {tf:?} transfer",
                    encoding.colour_space
                )));
            }
        };
        if grey {
            return Err(Error::Unsupported("DNG JPEG XL grey lossy tile".into()));
        }
        // Decoded to linear sRGB, which jxl-oxide reaches from XYB by matrix
        // alone, and taken to the encoding's primaries here: its own
        // conversion would clip or gamut-map what lies outside sRGB.
        let matrix = srgb_to(encoding)?;
        image.request_color_encoding(EnumColourEncoding::srgb_linear(RenderingIntent::Relative));
        Some((matrix, curve))
    } else {
        None
    };
    let mut rest = data.get(used..).unwrap_or_default();
    while !rest.is_empty() {
        let n = image.feed_bytes(rest).map_err(jxl)?;
        if n == 0 {
            break;
        }
        rest = &rest[n..];
    }
    image.finalize().map_err(jxl)?;
    if image.num_loaded_keyframes() != 1 {
        return Err(corrupt("DNG JPEG XL tile frames"));
    }
    let render = image.render_frame(0).map_err(jxl)?;
    let mut stream = render.stream_no_alpha();
    if stream.channels() as usize != channels
        || (stream.width() as usize, stream.height() as usize) != (width, height)
    {
        return Err(corrupt("DNG JPEG XL tile channels"));
    }
    let mut samples = vec![0f32; width * height * channels];
    if stream.write_to_buffer(&mut samples) != samples.len() {
        return Err(corrupt("DNG JPEG XL tile"));
    }
    if let Some((m, curve)) = to_rgb {
        for p in samples.as_chunks_mut::<3>().0 {
            let [r, g, b] = p.map(f64::from);
            for (i, v) in p.iter_mut().enumerate() {
                let linear = m[i][0] * r + m[i][1] * g + m[i][2] * b;
                *v = if curve { srgb_encode(linear) } else { linear } as f32;
            }
        }
    }
    if scale != 1. {
        for v in &mut samples {
            *v *= scale;
        }
    }
    Ok(Tile { width, samples })
}

/// IEC 61966-2-1's sRGB curve, from linear light to the encoded value,
/// mirrored for negative values (outside the gamut after the matrix), as
/// libjxl extends it.
fn srgb_encode(v: f64) -> f64 {
    let a = v.abs();
    let e = if a <= 0.0031308 {
        12.92 * a
    } else {
        1.055 * a.powf(1. / 2.4) - 0.055
    };
    e.copysign(v)
}

/// Linear sRGB (D65) to the linear RGB of `encoding`, which must have a D65
/// white: the matrix to XYZ of sRGB's primaries, then from XYZ to the
/// encoding's. The sRGB primaries are those jxl-oxide's XYB conversion uses.
fn srgb_to(encoding: &EnumColourEncoding) -> Result<[[f64; 3]; 3]> {
    if encoding.colour_space != ColourSpace::Rgb || encoding.white_point != WhitePoint::D65 {
        return Err(Error::Unsupported(
            "DNG JPEG XL tile of a white other than D65".into(),
        ));
    }
    let xy = |c: &Customxy| [f64::from(c.x) / 1e6, f64::from(c.y) / 1e6];
    const SRGB: [[f64; 2]; 3] = [
        [0.639998686, 0.330010138],
        [0.300003784, 0.600003357],
        [0.150002046, 0.059997204],
    ];
    let primaries = match &encoding.primaries {
        Primaries::Srgb => SRGB,
        Primaries::Bt2100 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        Primaries::P3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
        Primaries::Custom { red, green, blue } => [xy(red), xy(green), xy(blue)],
    };
    let target = rgb_to_xyz(primaries)?;
    let inverse = invert(target).ok_or_else(|| corrupt("DNG JPEG XL tile primaries"))?;
    Ok(multiply(inverse, rgb_to_xyz(SRGB)?))
}

/// RGB to XYZ for primaries `p` (CIE xy) and a D65 white.
fn rgb_to_xyz(p: [[f64; 2]; 3]) -> Result<[[f64; 3]; 3]> {
    const D65: [f64; 2] = [0.3127, 0.329];
    if p.iter().any(|[_, y]| *y <= 0.) {
        return Err(corrupt("DNG JPEG XL tile primaries"));
    }
    let columns = p.map(|[x, y]| [x / y, 1., (1. - x - y) / y]);
    let m = [0, 1, 2].map(|r| columns.map(|c| c[r]));
    let white = [D65[0] / D65[1], 1., (1. - D65[0] - D65[1]) / D65[1]];
    let inverse = invert(m).ok_or_else(|| corrupt("DNG JPEG XL tile primaries"))?;
    let s = [0, 1, 2].map(|r| (0..3).map(|k| inverse[r][k] * white[k]).sum::<f64>());
    Ok(m.map(|row| [row[0] * s[0], row[1] * s[1], row[2] * s[2]]))
}

fn multiply(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let cof = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det = (0..3).map(|c| m[0][c] * cof(0, c)).sum::<f64>();
    (det.abs() > 1e-12).then(|| [0, 1, 2].map(|r| [0, 1, 2].map(|c| cof(c, r) / det)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic codestreams made with libjxl 0.12's cjxl, both declaring
    // linear BT.2100 RGB (`-x color_space=RGB_D65_202_Rel_Lin`):
    // `jxl-hdr-xyb.jxl`, 32 × 32 floats, -d 0.5: a background of about 500,
    // 1200, 900 and a disc of 32000, 32500, 32768 at (16, 16), as an HDR
    // merge's sun; `jxl-int16-lossless.jxl`, 8 × 4 16-bit samples, -d 0:
    // pixel (x, y) is (x × 8191 + y × 3, y × 16000 + x, 65535 − x × 7000).
    const HDR: &[u8] = include_bytes!("../../tests/data/jxl-hdr-xyb.jxl");
    const INT16: &[u8] = include_bytes!("../../tests/data/jxl-int16-lossless.jxl");
    // Made with jixel 0.5.7 (as RAWmakase's JPEG XL export), declaring BT.2100
    // primaries with the sRGB transfer, as Lightroom's panorama DNGs do: 32 × 32
    // 16-bit samples, distance 0.5, neutral grey: pixel (x, y) is x × 2000 +
    // y × 50 + 300 in each channel. Neutral, because jixel converts its input
    // to XYB as sRGB whatever primaries it names; grey is grey in both.
    const SRGB_CURVE: &[u8] = include_bytes!("../../tests/data/jxl-int16-srgb-bt2100.jxl");

    fn pixel(tile: &Tile, x: usize, y: usize) -> [f32; 3] {
        let at = (y * tile.width + x) * 3;
        [0, 1, 2].map(|c| tile.samples[at + c])
    }

    /// A lossy tile's values above 1 keep their scale, as libjxl's djxl
    /// decodes them (its values here): no gamut mapping into 0–1, no
    /// overflow of 16-bit buffers in the bright disc.
    #[test]
    fn lossy_float_tiles_keep_hdr_values() {
        let tile = decode(HDR, [32, 32], 3).unwrap();
        assert_eq!(tile.width, 32);
        for (x, y, djxl) in [
            (16, 16, [32004.6, 32496.9, 32778.7]),
            (2, 3, [648.59, 1557.27, 1169.65]),
            (30, 28, [390.81, 942.19, 706.77]),
        ] {
            let got = pixel(&tile, x, y);
            for (g, d) in got.iter().zip(djxl) {
                assert!(
                    (g - d).abs() / d < 1e-3,
                    "({x}, {y}): {got:?}, djxl {djxl:?}"
                );
            }
        }
    }

    /// Lossless integer tiles decode to their samples exactly.
    #[test]
    fn lossless_integer_tiles_are_exact() {
        let tile = decode(INT16, [8, 4], 3).unwrap();
        for y in 0..4 {
            for x in 0..8 {
                let want = [
                    (x * 8191 + y * 3) % 65536,
                    (y * 16000 + x) % 65536,
                    65535 - x * 7000,
                ]
                .map(|v| v as f32);
                assert_eq!(pixel(&tile, x, y), want, "({x}, {y})");
            }
        }
    }

    /// A lossy tile whose codestream declares a transfer curve (Lightroom's
    /// panoramas: BT.2100 with sRGB's) decodes to the samples it was made
    /// from, on that curve, as libjxl decodes it to the image's own encoding.
    #[test]
    fn lossy_tiles_on_a_transfer_curve_keep_their_samples() {
        let tile = decode(SRGB_CURVE, [32, 32], 3).unwrap();
        let mut worst = 0f32;
        for y in 0..32 {
            for x in 0..32 {
                let want = [(x * 2000 + y * 50 + 300) as f32; 3];
                for (g, w) in pixel(&tile, x, y).iter().zip(want) {
                    worst = worst.max((g - w).abs() / 65535.);
                }
            }
        }
        assert!(worst < 0.01, "worst error {worst} of full scale");
    }

    /// A tile larger than the DNG's tile, or of other channels, is refused.
    #[test]
    fn tiles_of_another_shape_are_refused() {
        assert!(decode(HDR, [16, 32], 3).is_err());
        assert!(decode(HDR, [32, 32], 1).is_err());
        assert!(decode(&HDR[..HDR.len() / 2], [32, 32], 3).is_err());
    }

    #[test]
    fn srgb_to_bt2100_is_the_known_matrix() {
        let e = EnumColourEncoding {
            colour_space: ColourSpace::Rgb,
            white_point: WhitePoint::D65,
            primaries: Primaries::Bt2100,
            tf: TransferFunction::Linear,
            rendering_intent: RenderingIntent::Relative,
        };
        let m = srgb_to(&e).unwrap();
        // ITU-R BT.2087's BT.709 to BT.2020 matrix.
        let known = [
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ];
        for (row, k) in m.iter().zip(known) {
            for (v, k) in row.iter().zip(k) {
                assert!((v - k).abs() < 2e-4, "{m:?}");
            }
        }
        // White stays white.
        for row in m {
            assert!((row.iter().sum::<f64>() - 1.).abs() < 1e-6);
        }
    }
}
