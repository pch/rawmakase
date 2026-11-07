//! Codecs 1 and 2: the pixel codes as a lossless JPEG XL codestream (ISO/IEC 18181-1) of
//! three 16-bit integer channels, encoded by `zune-jpegxl` and decoded by
//! `jxl-oxide`. The codestream's header says sRGB, as zune writes it; the samples
//! are the curve's codes in camera space, and nothing reads that colour encoding.
use anyhow::{Context, Result, ensure};
use jxl_oxide::image::BitDepth;
use jxl_oxide::{AllocTracker, InitializeResult, JxlImage};

/// The codestream of `codes`, `width` × `height` pixels of red, green and blue.
pub(super) fn encode(width: u32, height: u32, codes: &[[u16; 3]]) -> Result<Vec<u8>> {
    use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
    ensure!(
        codes.len() == width as usize * height as usize,
        "An editable preview's pixels have the wrong size"
    );
    let bytes: Vec<u8> = codes
        .iter()
        .flatten()
        .flat_map(|c| c.to_ne_bytes())
        .collect();
    // As many threads as the pool it runs in (the app's build pace), rather than
    // zune's own four whatever that pace.
    let threads = u8::try_from(rayon::current_num_threads()).unwrap_or(u8::MAX);
    let options = EncoderOptions::new(
        width as usize,
        height as usize,
        ColorSpace::RGB,
        BitDepth::Sixteen,
    )
    .set_num_threads(threads.max(1));
    let mut out = Vec::new();
    zune_jpegxl::JxlSimpleEncoder::new(&bytes, options)
        .encode(&mut out)
        .map_err(|e| anyhow::anyhow!("Editable preview encoding failed: {e:?}"))?;
    Ok(out)
}

/// The most jxl-oxide may allocate per sample of the image, as the DNG tile
/// decoder allows it, and at least `MIN_BYTES` for its fixed costs.
const BYTES_PER_SAMPLE: usize = 64;
const MIN_BYTES: usize = 16 << 20;

/// The codes of a `width` × `height` codestream; an error for any other size, a
/// codestream that is not three 16-bit integer channels coded losslessly, or one
/// that is damaged.
pub(super) fn decode(data: &[u8], width: u32, height: u32) -> Result<Vec<[u16; 3]>> {
    let (w, h) = (width as usize, height as usize);
    let limit = w
        .saturating_mul(h)
        .saturating_mul(3)
        .saturating_mul(BYTES_PER_SAMPLE)
        .max(MIN_BYTES);
    let jxl = |e: Box<dyn std::error::Error + Send + Sync>| {
        anyhow::anyhow!("An editable preview's pixels are damaged: {e}")
    };
    let mut uninit = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(limit))
        .build_uninit();
    let used = uninit.feed_bytes(data).map_err(jxl)?;
    let mut image = match uninit.try_init().map_err(jxl)? {
        InitializeResult::Initialized(image) => image,
        InitializeResult::NeedMoreData(_) => {
            anyhow::bail!("An editable preview's pixels are cut short")
        }
    };
    ensure!(
        (image.width() as usize, image.height() as usize) == (w, h),
        "An editable preview's pixels are not the size its header gives"
    );
    let metadata = &image.image_header().metadata;
    ensure!(
        matches!(
            metadata.bit_depth,
            BitDepth::IntegerSample {
                bits_per_sample: 16
            }
        ) && !metadata.xyb_encoded
            && !metadata.grayscale(),
        "An editable preview's pixels are not 16-bit lossless RGB"
    );
    let mut rest = data.get(used..).unwrap_or_default();
    while !rest.is_empty() {
        let n = image.feed_bytes(rest).map_err(jxl)?;
        if n == 0 {
            break;
        }
        rest = &rest[n..];
    }
    image.finalize().map_err(jxl)?;
    ensure!(
        image.num_loaded_keyframes() == 1,
        "An editable preview's pixels are cut short"
    );
    let render = image.render_frame(0).map_err(jxl)?;
    let mut stream = render.stream_no_alpha();
    ensure!(
        stream.channels() == 3 && (stream.width() as usize, stream.height() as usize) == (w, h),
        "An editable preview's pixels are not three channels"
    );
    let mut samples = vec![0f32; w * h * 3];
    ensure!(
        stream.write_to_buffer(&mut samples) == samples.len(),
        "An editable preview's pixels are cut short"
    );
    // jxl-oxide gives an integer sample s of a 16-bit image as s / 65535, exactly
    // enough in single precision for the rounding to recover s.
    let code = |v: f32| -> Option<u16> {
        let s = (v * 65535.).round();
        (0. ..=65535.).contains(&s).then_some(s as u16)
    };
    samples
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Some([code(p[0])?, code(p[1])?, code(p[2])?]))
        .collect::<Option<Vec<_>>>()
        .context("An editable preview's pixels are out of range")
}
