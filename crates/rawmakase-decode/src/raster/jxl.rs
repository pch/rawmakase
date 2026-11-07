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
}

/// What a JPEG XL file's or codestream's headers say.
pub(super) struct Header {
    pub(super) hdr: bool,
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
