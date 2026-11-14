//! A JPEG XL file through jxl-oxide's `image` decoder. jxl-oxide renders the
//! image upright by its codestream, so no orientation is applied after it.
use super::Unsupported;
use anyhow::{Context, Result};
use image::ImageDecoder;
use std::io::{BufReader, Read};
use std::path::Path;

pub(super) struct JxlFile {
    inner: jxl_oxide::integration::JxlDecoder<BufReader<std::fs::File>>,
    header: Header,
}

impl JxlFile {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let header = Header::read(&mut std::fs::File::open(path)?)?;
        let inner =
            jxl_oxide::integration::JxlDecoder::new(BufReader::new(std::fs::File::open(path)?))
                .context("Could not read the JPEG XL file")?;
        Ok(Self { inner, header })
    }

    /// Why Develop refuses it. An animation is refused whatever else it is; HDR is
    /// last, as the reason `raster::prepare` lifts for HDR that decodes exactly.
    pub(super) fn refusal(&self) -> Option<Unsupported> {
        if self.header.animated {
            Some(Unsupported("animated".into()))
        } else if self.header.hdr {
            Some(Unsupported("HDR".into()))
        } else {
            None
        }
    }

    /// Whether its codestream turns or mirrors it.
    pub(super) fn turned(&self) -> bool {
        self.header.turned
    }

    /// How its HDR samples decode, by its codestream's own colour encoding.
    pub(super) fn hdr_decoding(&self) -> Option<super::HdrDecoding> {
        self.header.decoding
    }
}

/// How the samples of a codestream whose colour encoding is enum values decode, when
/// its transfer is PQ or HLG, read from the encoding itself: the profile jxl-oxide
/// makes of it names CICP primaries by the primaries alone, so Display P3's (D65
/// white) come out as DCI-P3's (11, with DCI's white). BT.709, BT.2020 and Display P3
/// decode as their gamuts; any other primaries or white, DCI-P3's and custom
/// chromaticities among them, through
/// [`CicpPrimaries::from_chromaticities`](rawmakase_model::color::hdr_output::CicpPrimaries::from_chromaticities).
/// `None` for an ICC profile, grey, XYB or an SDR transfer, and for linear light:
/// jxl-oxide's profile names only PQ and HLG by `cicp`, so a linear JPEG XL has always
/// opened as an SDR file, and its edits keep rendering so.
fn decoding(encoding: &jxl_oxide::color::ColourEncoding) -> Option<super::HdrDecoding> {
    use jxl_oxide::color::{ColourEncoding, ColourSpace, Primaries, TransferFunction, WhitePoint};
    use rawmakase_model::color::hdr_output::{CicpPrimaries, HdrGamut, HdrOutput, HdrTransfer};
    let ColourEncoding::Enum(e) = encoding else {
        return None;
    };
    if e.colour_space != ColourSpace::Rgb {
        return None;
    }
    let transfer = match e.tf {
        TransferFunction::Pq => HdrTransfer::Pq,
        TransferFunction::Hlg => HdrTransfer::Hlg,
        _ => return None,
    };
    let gamut = match (&e.primaries, &e.white_point) {
        (Primaries::Srgb, WhitePoint::D65) => Some(HdrGamut::Rec709),
        (Primaries::Bt2100, WhitePoint::D65) => Some(HdrGamut::Rec2020),
        (Primaries::P3, WhitePoint::D65) => Some(HdrGamut::DisplayP3),
        _ => None,
    };
    if let Some(gamut) = gamut {
        return Some(super::HdrDecoding::Signal(HdrOutput {
            gamut,
            transfer,
            ..Default::default()
        }));
    }
    let xy = |c: &jxl_oxide::color::Customxy| [f64::from(c.x) / 1e6, f64::from(c.y) / 1e6];
    let rgb = match &e.primaries {
        Primaries::Srgb => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        Primaries::Bt2100 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        Primaries::P3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
        Primaries::Custom { red, green, blue } => [xy(red), xy(green), xy(blue)],
    };
    let white = match &e.white_point {
        WhitePoint::D65 => [0.3127, 0.3290],
        WhitePoint::Dci => [0.314, 0.351],
        WhitePoint::E => [1. / 3., 1. / 3.],
        WhitePoint::Custom(c) => xy(c),
    };
    let primaries = CicpPrimaries::from_chromaticities(rgb, white)?;
    Some(super::HdrDecoding::Primaries(transfer, primaries))
}

/// What a JPEG XL file's or codestream's headers say.
pub(super) struct Header {
    pub(super) hdr: bool,
    /// How its HDR samples decode, by its colour encoding ([`decoding`]).
    pub(super) decoding: Option<super::HdrDecoding>,
    pub(super) animated: bool,
    /// Its orientation is not the identity.
    pub(super) turned: bool,
    /// Its size, upright.
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Header {
    /// The headers of the file or naked codestream `from` reads, fed in 64 KiB
    /// pieces until they are read, at most 16 MiB.
    pub(super) fn read(from: &mut impl Read) -> Result<Self> {
        use jxl_oxide::InitializeResult;
        let mut uninit = jxl_oxide::JxlImage::builder().build_uninit();
        let mut buf = vec![0u8; 64 << 10];
        for _ in 0..256 {
            let n = from.read(&mut buf)?;
            anyhow::ensure!(n > 0, "The JPEG XL file ends before its header");
            uninit
                .feed_bytes(&buf[..n])
                .map_err(|e| anyhow::anyhow!("Could not read the JPEG XL header: {e}"))?;
            match uninit
                .try_init()
                .map_err(|e| anyhow::anyhow!("Could not read the JPEG XL header: {e}"))?
            {
                InitializeResult::NeedMoreData(more) => uninit = more,
                InitializeResult::Initialized(image) => {
                    let metadata = &image.image_header().metadata;
                    return Ok(Self {
                        hdr: image.hdr_type().is_some(),
                        decoding: decoding(&metadata.colour_encoding),
                        animated: metadata.animation.is_some(),
                        turned: metadata.orientation != 1,
                        width: image.width(),
                        height: image.height(),
                    });
                }
            }
        }
        anyhow::bail!("The JPEG XL header is too large")
    }
}

impl ImageDecoder for JxlFile {
    fn dimensions(&self) -> (u32, u32) {
        self.inner.dimensions()
    }
    fn color_type(&self) -> image::ColorType {
        self.inner.color_type()
    }
    fn icc_profile(&mut self) -> image::ImageResult<Option<Vec<u8>>> {
        self.inner.icc_profile()
    }
    fn orientation(&mut self) -> image::ImageResult<image::metadata::Orientation> {
        Ok(image::metadata::Orientation::NoTransforms)
    }
    fn read_image(self, buf: &mut [u8]) -> image::ImageResult<()> {
        self.inner.read_image(buf)
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> image::ImageResult<()> {
        self.inner.read_image(buf)
    }
}
