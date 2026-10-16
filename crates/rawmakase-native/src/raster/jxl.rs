//! A JPEG XL file through jxl-oxide's `image` decoder. jxl-oxide renders the
//! image upright by its codestream, so no orientation is applied after it.
use super::Unsupported;
use anyhow::{Context, Result};
use image::ImageDecoder;
use std::io::{BufReader, Read};
use std::path::Path;

pub(super) struct JxlFile {
    inner: jxl_oxide::integration::JxlDecoder<BufReader<std::fs::File>>,
    hdr: bool,
    animated: bool,
}

impl JxlFile {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let (hdr, animated) = header_facts(path)?;
        let inner =
            jxl_oxide::integration::JxlDecoder::new(BufReader::new(std::fs::File::open(path)?))
                .context("Could not read the JPEG XL file")?;
        Ok(Self {
            inner,
            hdr,
            animated,
        })
    }

    pub(super) fn refusal(&self) -> Option<Unsupported> {
        if self.hdr {
            Some(Unsupported("HDR".into()))
        } else if self.animated {
            Some(Unsupported("animated".into()))
        } else {
            None
        }
    }
}

/// Whether the file is HDR and whether it is animated, from its headers: fed
/// in 64 KiB pieces until they are read, at most 16 MiB.
fn header_facts(path: &Path) -> Result<(bool, bool)> {
    use jxl_oxide::InitializeResult;
    let mut file = std::fs::File::open(path)?;
    let mut uninit = jxl_oxide::JxlImage::builder().build_uninit();
    let mut buf = vec![0u8; 64 << 10];
    for _ in 0..256 {
        let n = file.read(&mut buf)?;
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
                let animated = image.image_header().metadata.animation.is_some();
                return Ok((image.hdr_type().is_some(), animated));
            }
        }
    }
    anyhow::bail!("The JPEG XL header is too large")
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
