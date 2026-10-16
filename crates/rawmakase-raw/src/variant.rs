//! The one selector: what a probed file is (container, encoding, layout), and
//! which reader and codec decode each variant.
use crate::error::{Error, Result};
use crate::info::RawInfo;
use crate::sensor::Sensor;
use std::sync::atomic::AtomicBool;

/// A RAW variant: a container with one encoding and layout of its sensor
/// data. A file of a recognised container in a variant not listed here is
/// `Error::Unsupported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Variant {
    /// DNG, uncompressed integer samples of 8 to 16 bits.
    DngUncompressed,
    /// DNG, lossless JPEG tiles or strips.
    DngLosslessJpeg,
    /// DNG, Deflate-compressed (integer or floating point).
    DngDeflate,
    /// DNG, uncompressed floating-point samples.
    DngUncompressedFloat,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::DngUncompressed => "dng-uncompressed",
            Variant::DngLosslessJpeg => "dng-ljpeg",
            Variant::DngDeflate => "dng-deflate",
            Variant::DngUncompressedFloat => "dng-float",
        }
    }
}

/// Where a variant's data is: each container's own plan.
#[derive(Clone, Debug)]
pub(crate) enum Plan {
    None,
    Dng(crate::container::dng::DngPlan),
}

/// Reads the header facts of the RAW file in `bytes`.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let mut info = probe(bytes)?;
    crate::names::apply(&mut info);
    crate::geometry::apply(&mut info);
    crate::color::apply(&mut info);
    Ok(info)
}

/// The container's own reading of the file, names as the file writes them.
fn probe(bytes: &[u8]) -> Result<RawInfo> {
    if let Some(t) = crate::container::tiff::TiffFile::parse(bytes, 0) {
        let ifd0 = &t.ifds[0];
        if ifd0.has(50706) {
            return crate::container::dng::open(bytes, t);
        }
    }
    Err(Error::NotRaw)
}

/// Decodes the sensor data `info` describes.
pub(crate) fn decode(info: &RawInfo, bytes: &[u8], cancel: &AtomicBool) -> Result<Sensor> {
    match (&info.plan, info.variant) {
        (
            Plan::Dng(plan),
            Variant::DngUncompressed
            | Variant::DngLosslessJpeg
            | Variant::DngDeflate
            | Variant::DngUncompressedFloat,
        ) => crate::container::dng::decode(info, plan, bytes, cancel),
        (Plan::None, _) => Err(Error::Unsupported("no sensor data".into())),
    }
}
