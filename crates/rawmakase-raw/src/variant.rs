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
    /// Panasonic RW2 (raw formats 3 and 4): delta-coded 16-byte blocks, the
    /// chunks of format 4 rotated.
    Rw2Delta,
    /// Panasonic RW2 (raw format 5): packed 12 or 14-bit samples in 16-byte
    /// blocks, chunks rotated.
    Rw2PackedRotated,
    /// Panasonic RW2 (raw format 6): seeded delta-coded 16-byte blocks of 12
    /// or 14-bit samples.
    Rw2Seeded,
    /// Panasonic RW2 (raw format 7) and RAW (raw format 2): packed 14 or
    /// 12-bit samples in 16-byte blocks.
    Rw2Packed,
    /// Panasonic RW2 (raw format 8): prefix-coded strips of 12 to 16-bit
    /// samples.
    Rw2Huffman,
    /// Panasonic and Leica RAW of the RW2 structure, without a raw format:
    /// 12-bit samples in 16-bit words.
    Rw2Unpacked,
    /// Sony ARW2: 16-sample blocks of 11-bit codes on a tone curve.
    ArwCurve,
    /// Sony ARW and SR2, uncompressed 16-bit words.
    ArwUncompressed,
    /// Sony ARW, packed 12-bit samples.
    ArwPacked,
    /// Sony ARW, lossless JPEG tiles of 2 × 2 colour cells.
    ArwLosslessJpeg,
    /// Sony ARW's small RAWs: lossless JPEG tiles of luma and chroma.
    ArwYcbcr,
    /// Sony's first ARW coding: Huffman-coded columns.
    ArwV1,
    /// Sony ARW's "compressed RAW 2": LLVC (SMPTE RDD 34) tiles.
    ArwV6,
    /// Nikon NEF, Huffman-coded (lossless or lossy, with or without a split).
    NefHuffman,
    /// Nikon NEF, High Efficiency (JPEG XS).
    NefHighEfficiency,
    /// Nikon NEF or NRW, 16-bit words.
    NefUnpacked,
    /// Nikon NEF or NRW, 12-bit samples packed, in one of the layouts the
    /// bodies use.
    NefPacked,
    /// Nikon NEF, 14-bit samples packed, rows padded.
    NefPacked14,
    /// Nikon NEF small RAW: luma and chroma, converted to RGB.
    NefSmallRaw,
    /// Fujifilm RAF, 16-bit words row by row.
    RafUnpacked,
    /// Fujifilm RAF of a Super CCD SR: rows of S pixels, then R pixels.
    RafSuperCcdSr,
    /// Fujifilm RAF of a digital back: 16-bit words in vertical tiles.
    RafTiled,
    /// Fujifilm RAF, 12-bit samples packed least significant bit first.
    RafPacked,
    /// Fujifilm RAF, samples packed most significant bit first in 32-bit
    /// words.
    RafPackedWords,
    /// Fujifilm RAF, compressed stripes (lossless or lossy).
    RafCompressed,
    /// Canon CR3, lossless CRX: median-predicted, Rice-coded Bayer planes.
    Cr3Lossless,
    /// Canon CR3, C-RAW: CRX planes in three wavelet levels, quantized.
    Cr3Craw,
    /// Olympus ORF and ORI, Olympus's compressed coding.
    OrfCompressed,
    /// Olympus ORF, packed 12-bit samples (in words, blocks or two fields).
    OrfPacked,
    /// Olympus ORF, 16-bit words.
    OrfUnpacked,
    /// Pentax PEF, Pentax's Huffman-coded differences.
    PefHuffman,
    /// Pentax PEF, packed 12-bit samples.
    PefPacked,
    /// Pentax PEF, 16-bit words.
    PefUnpacked,
    /// Samsung SRW, 16-bit words.
    SrwUnpacked,
    /// Samsung SRW, packed 12-bit rows.
    SrwPacked,
    /// Samsung SRW (compression 32770), rows of blocks of 16.
    SrwBlocks,
    /// Samsung SRW (compression 32772), Huffman-coded differences.
    SrwHuffman,
    /// Samsung SRW (compression 32773), tiles of 16.
    SrwTiles,
    /// Epson ERF, packed 12-bit samples in 16-byte blocks of ten.
    ErfPacked,
    /// Mamiya MEF, packed 12-bit samples.
    MefPacked,
    /// Canon CR2, a mosaic in one sliced lossless JPEG strip.
    Cr2LosslessJpeg,
    /// Canon CR2 sRAW and mRAW: luma and chroma in one sliced lossless JPEG
    /// strip, converted to RGB.
    Cr2Sraw,
    /// A RAW file with no header (CHDK's, action cameras'), known by its
    /// size: packed samples.
    HeaderlessPacked,
    /// Canon CRW: Canon's compressed sensor data in a CIFF file.
    CrwCompressed,
    /// Phase One IIQ L (and L16): rows of differences in runs of even and
    /// odd columns.
    IiqL,
    /// Phase One IIQ S (the first): IIQ L's rows, low values on a curve.
    IiqS,
    /// Phase One IIQ, uncompressed 16-bit words.
    IiqUncompressed,
    /// Hasselblad 3FR and FFF, 16-bit words.
    FffUncompressed,
    /// Hasselblad 3FR and FFF, Hasselblad's coded pairs of differences.
    FffCompressed,
    /// Leaf MOS, one lossless JPEG tile of the image's rows.
    MosLosslessJpeg,
    /// Leaf MOS, 16-bit words.
    MosUncompressed,
    /// Sinar STI, 16-bit words.
    StiUncompressed,
    /// Raspberry Pi: a JPEG followed by Broadcom's raw block of 10-bit
    /// samples packed four in five bytes.
    BrcmPacked,
    /// Minolta MRW, packed 12-bit samples.
    MrwPacked,
    /// Minolta MRW, 16-bit big-endian words.
    MrwUnpacked,
    /// Kodak DCS TIFF, one lossless JPEG strip of codes on a curve.
    DcsLosslessJpeg,
    /// Kodak KDC (EasyShare and P-series), packed 12-bit samples.
    KdcPacked,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::DngUncompressed => "dng-uncompressed",
            Variant::DngLosslessJpeg => "dng-ljpeg",
            Variant::DngDeflate => "dng-deflate",
            Variant::DngUncompressedFloat => "dng-float",
            Variant::Rw2Delta => "rw2-delta",
            Variant::Rw2PackedRotated => "rw2-packed-rotated",
            Variant::Rw2Seeded => "rw2-seeded",
            Variant::Rw2Packed => "rw2-packed",
            Variant::Rw2Huffman => "rw2-huffman",
            Variant::Rw2Unpacked => "rw2-unpacked",
            Variant::ArwCurve => "arw-curve",
            Variant::ArwUncompressed => "arw-uncompressed",
            Variant::ArwPacked => "arw-packed",
            Variant::ArwLosslessJpeg => "arw-ljpeg",
            Variant::ArwYcbcr => "arw-ycbcr",
            Variant::ArwV1 => "arw-v1",
            Variant::ArwV6 => "arw-v6",
            Variant::NefHuffman => "nef-huffman",
            Variant::NefHighEfficiency => "nef-high-efficiency",
            Variant::NefUnpacked => "nef-unpacked",
            Variant::NefPacked => "nef-packed",
            Variant::NefPacked14 => "nef-packed-14",
            Variant::NefSmallRaw => "nef-small-raw",
            Variant::RafUnpacked => "raf-unpacked",
            Variant::RafSuperCcdSr => "raf-super-ccd-sr",
            Variant::RafTiled => "raf-tiled",
            Variant::RafPacked => "raf-packed",
            Variant::RafPackedWords => "raf-packed-words",
            Variant::RafCompressed => "raf-compressed",
            Variant::Cr3Lossless => "cr3-lossless",
            Variant::Cr3Craw => "cr3-craw",
            Variant::OrfCompressed => "orf-compressed",
            Variant::OrfPacked => "orf-packed",
            Variant::OrfUnpacked => "orf-unpacked",
            Variant::PefHuffman => "pef-huffman",
            Variant::PefPacked => "pef-packed",
            Variant::PefUnpacked => "pef-unpacked",
            Variant::SrwUnpacked => "srw-unpacked",
            Variant::SrwPacked => "srw-packed",
            Variant::SrwBlocks => "srw-blocks",
            Variant::SrwHuffman => "srw-huffman",
            Variant::SrwTiles => "srw-tiles",
            Variant::ErfPacked => "erf-packed",
            Variant::MefPacked => "mef-packed",
            Variant::Cr2LosslessJpeg => "cr2-ljpeg",
            Variant::Cr2Sraw => "cr2-sraw",
            Variant::HeaderlessPacked => "headerless-packed",
            Variant::CrwCompressed => "crw",
            Variant::IiqL => "iiq-l",
            Variant::IiqS => "iiq-s",
            Variant::IiqUncompressed => "iiq-uncompressed",
            Variant::FffUncompressed => "3fr-uncompressed",
            Variant::FffCompressed => "3fr-compressed",
            Variant::MosLosslessJpeg => "mos-ljpeg",
            Variant::MosUncompressed => "mos-uncompressed",
            Variant::StiUncompressed => "sti-uncompressed",
            Variant::BrcmPacked => "brcm-packed",
            Variant::MrwPacked => "mrw-packed",
            Variant::MrwUnpacked => "mrw-unpacked",
            Variant::DcsLosslessJpeg => "dcs-ljpeg",
            Variant::KdcPacked => "kdc-packed",
        }
    }
}

/// Where a variant's data is: each container's own plan.
#[derive(Clone, Debug)]
pub(crate) enum Plan {
    None,
    Dng(crate::container::dng::DngPlan),
    Rw2(crate::container::rw2::Rw2Plan),
    Arw(crate::container::arw::ArwPlan),
    Nef(crate::container::nef::NefPlan),
    Raf(crate::container::raf::RafPlan),
    Cr3(crate::container::cr3::Cr3Plan),
    Orf(crate::container::orf::OrfPlan),
    Pef(crate::container::pef::PefPlan),
    Srw(crate::container::srw::SrwPlan),
    Erf(crate::container::erf::ErfPlan),
    Cr2(crate::container::cr2::Cr2Plan),
    Headerless(crate::container::headerless::HeaderlessPlan),
    Crw(crate::container::ciff::CrwPlan),
    Iiq(crate::container::iiq::IiqPlan),
    Fff(crate::container::fff::FffPlan),
    Mos(crate::container::mos::MosPlan),
    Sti(crate::container::sti::StiPlan),
    Brcm(crate::container::brcm::BrcmPlan),
    Mrw(crate::container::mrw::MrwPlan),
    Dcs(crate::container::dcs::DcsPlan),
    Kdc(crate::container::kdc::KdcPlan),
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
    if crate::container::raf::is_raf(bytes) {
        return crate::container::raf::open(bytes);
    }
    if crate::container::cr3::is_cr3(bytes) {
        return crate::container::cr3::open(bytes);
    }
    if crate::container::mrw::is_mrw(bytes) {
        return crate::container::mrw::open(bytes);
    }
    if crate::container::ciff::is_ciff(bytes) {
        return crate::container::ciff::open(bytes);
    }
    if crate::container::rw2::is_rw2(bytes) {
        let t = crate::container::tiff::TiffFile::parse(bytes, 0)
            .ok_or_else(|| crate::error::corrupt("RW2 directory"))?;
        return crate::container::rw2::open(t);
    }
    if let Some(t) = crate::container::tiff::TiffFile::parse(bytes, 0) {
        let ifd0 = &t.ifds[0];
        if ifd0.has(50706) {
            return crate::container::dng::open(bytes, t);
        }
        if crate::container::iiq::is_iiq(bytes) {
            return crate::container::iiq::open(bytes, t);
        }
        if crate::container::arw::is_sony(&t) {
            return crate::container::arw::open(t);
        }
        if crate::container::nef::is_nef(&t) {
            return crate::container::nef::open(bytes, t);
        }
        if crate::container::orf::is_orf(bytes, &t) {
            return crate::container::orf::open(t);
        }
        if crate::container::pef::is_pef(&t) {
            return crate::container::pef::open(t);
        }
        if crate::container::srw::is_srw(&t) {
            return crate::container::srw::open(t);
        }
        if crate::container::fff::is_fff(&t) {
            return crate::container::fff::open(t);
        }
        if crate::container::mos::is_mos(&t) {
            return crate::container::mos::open(t);
        }
        if crate::container::sti::is_sti(&t) {
            return crate::container::sti::open(t);
        }
        if crate::container::dcs::is_dcs(&t) {
            return crate::container::dcs::open(t);
        }
        if crate::container::kdc::is_kdc(&t) {
            return crate::container::kdc::open(t);
        }
        if crate::container::erf::is_erf(&t) {
            return crate::container::erf::open(t);
        }
        if crate::container::cr2::is_cr2(bytes) {
            return crate::container::cr2::open(bytes, t);
        }
        // A TIFF of no RAW container this crate reads.
        return Err(Error::Unsupported("TIFF of no known RAW container".into()));
    }
    // A TIFF whose directories do not read.
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Err(crate::error::corrupt("TIFF directory"));
    }
    // An ISO base media file (an `ftyp` box first) other than a CR3.
    if bytes.get(4..8) == Some(b"ftyp") {
        return Err(Error::Unsupported(
            "ISO base media file of no known RAW".into(),
        ));
    }
    // A JPEG followed by Broadcom's raw block (Raspberry Pi).
    if crate::container::brcm::is_brcm(bytes) {
        return crate::container::brcm::open(bytes);
    }
    // Files of a known non-RAW kind are not RAW whatever their size: JPEG
    // (without Broadcom's block), PNG, GIF, RIFF, PDF and ZIP.
    const NOT_RAW: [&[u8]; 7] = [
        &[0xff, 0xd8, 0xff],
        b"\x89PNG\r\n\x1a\n",
        b"GIF87a",
        b"GIF89a",
        b"RIFF",
        b"%PDF-",
        b"PK\x03\x04",
    ];
    if NOT_RAW.iter().any(|m| bytes.starts_with(m)) {
        return Err(Error::NotRaw);
    }
    // Last: a file of no container may be one the table knows by size.
    crate::container::headerless::open(bytes).ok_or(Error::NotRaw)
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
        (
            Plan::Rw2(plan),
            Variant::Rw2Delta
            | Variant::Rw2PackedRotated
            | Variant::Rw2Seeded
            | Variant::Rw2Packed
            | Variant::Rw2Huffman
            | Variant::Rw2Unpacked,
        ) => crate::container::rw2::decode(info, plan, bytes, cancel),
        (
            Plan::Arw(plan),
            Variant::ArwCurve
            | Variant::ArwUncompressed
            | Variant::ArwPacked
            | Variant::ArwLosslessJpeg
            | Variant::ArwYcbcr
            | Variant::ArwV1
            | Variant::ArwV6,
        ) => crate::container::arw::decode(info, plan, bytes, cancel),
        (
            Plan::Nef(plan),
            Variant::NefHuffman
            | Variant::NefHighEfficiency
            | Variant::NefUnpacked
            | Variant::NefPacked
            | Variant::NefPacked14
            | Variant::NefSmallRaw,
        ) => crate::container::nef::decode(info, plan, bytes, cancel),
        (
            Plan::Raf(plan),
            Variant::RafUnpacked
            | Variant::RafSuperCcdSr
            | Variant::RafTiled
            | Variant::RafPacked
            | Variant::RafPackedWords
            | Variant::RafCompressed,
        ) => crate::container::raf::decode(info, plan, bytes, cancel),
        (Plan::Cr3(plan), Variant::Cr3Lossless | Variant::Cr3Craw) => {
            crate::container::cr3::decode(info, plan, bytes, cancel)
        }
        (Plan::Orf(plan), Variant::OrfCompressed | Variant::OrfPacked | Variant::OrfUnpacked) => {
            crate::container::orf::decode(info, plan, bytes, cancel)
        }
        (Plan::Pef(plan), Variant::PefHuffman | Variant::PefPacked | Variant::PefUnpacked) => {
            crate::container::pef::decode(info, plan, bytes, cancel)
        }
        (
            Plan::Srw(plan),
            Variant::SrwUnpacked
            | Variant::SrwPacked
            | Variant::SrwBlocks
            | Variant::SrwHuffman
            | Variant::SrwTiles,
        ) => crate::container::srw::decode(info, plan, bytes, cancel),
        (Plan::Erf(plan), Variant::ErfPacked | Variant::MefPacked) => {
            crate::container::erf::decode(info, plan, bytes, cancel)
        }
        (Plan::Cr2(plan), Variant::Cr2LosslessJpeg | Variant::Cr2Sraw) => {
            crate::container::cr2::decode(info, plan, bytes, cancel)
        }
        (Plan::Headerless(plan), Variant::HeaderlessPacked) => {
            crate::container::headerless::decode(info, plan, bytes, cancel)
        }
        (Plan::Crw(plan), Variant::CrwCompressed) => {
            crate::container::ciff::decode(info, plan, bytes, cancel)
        }
        (Plan::Iiq(plan), Variant::IiqL | Variant::IiqS | Variant::IiqUncompressed) => {
            crate::container::iiq::decode(info, plan, bytes, cancel)
        }
        (Plan::Fff(plan), Variant::FffUncompressed | Variant::FffCompressed) => {
            crate::container::fff::decode(info, plan, bytes, cancel)
        }
        (Plan::Mos(plan), Variant::MosLosslessJpeg | Variant::MosUncompressed) => {
            crate::container::mos::decode(info, plan, bytes, cancel)
        }
        (Plan::Sti(plan), Variant::StiUncompressed) => {
            crate::container::sti::decode(info, plan, bytes, cancel)
        }
        (Plan::Brcm(plan), Variant::BrcmPacked) => {
            crate::container::brcm::decode(info, plan, bytes, cancel)
        }
        (Plan::Mrw(plan), Variant::MrwPacked | Variant::MrwUnpacked) => {
            crate::container::mrw::decode(info, plan, bytes, cancel)
        }
        (Plan::Dcs(plan), Variant::DcsLosslessJpeg) => {
            crate::container::dcs::decode(info, plan, bytes, cancel)
        }
        (Plan::Kdc(plan), Variant::KdcPacked) => {
            crate::container::kdc::decode(info, plan, bytes, cancel)
        }
        (Plan::None, _) => Err(Error::Unsupported("no sensor data".into())),
        (_, variant) => Err(Error::Unsupported(format!(
            "{} with another container's plan",
            variant.name()
        ))),
    }
}
