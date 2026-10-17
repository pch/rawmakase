//! Epson ERF and Mamiya MEF: a TIFF whose IFD0 is a thumbnail and whose
//! SubIFD is the raw image, packed 12-bit samples most significant bit
//! first; ERF's rows of 16-byte blocks of ten samples and a pad byte
//! (compression 32769), MEF's plainly (compression 1). Epson's maker note is
//! `vendor::epson`'s; MEF's says nothing RAWmakase reads.
//!
//! Sources: TIFF 6 and TIFF/EP (SubIFDs, CFAPattern), and our own
//! measurements of the packing against LibRaw's output on our files.
use super::tiff::{Ifd, NAME_MAX, TiffFile};
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::epson;
use std::sync::atomic::AtomicBool;

/// Where the sensor data is and how it is packed, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct ErfPlan {
    pub offset: u64,
    pub width: u32,
    pub height: u32,
    pub packing: Packing,
    pub black: [u32; 4],
    pub white: u32,
}

/// The raw image's directory: a 12-bit CFA image.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds
        .iter()
        .find(|ifd| t.int(ifd, 262) == Some(32803) && ifd.has(256) && ifd.has(273))
}

/// Whether the TIFF `t` is an Epson ERF or a Mamiya MEF.
pub(crate) fn is_erf(t: &TiffFile) -> bool {
    let make = t.text_upto(&t.ifds[0], 271, NAME_MAX).unwrap_or_default();
    let make = make.trim().to_ascii_uppercase();
    (make.starts_with("SEIKO EPSON") || make.starts_with("MAMIYA")) && raw_ifd(t).is_some()
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let raw = raw_ifd(&t).ok_or_else(|| Error::Unsupported("ERF without raw data".into()))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("ERF width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("ERF height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(raw, 258).unwrap_or(12);
    let compression = t.int(raw, 259).unwrap_or(1);
    let (variant, packing) = match (compression, bits) {
        (32769, 12) => (
            Variant::ErfPacked,
            Packing {
                pad: true,
                ..Packing::msb(12)
            },
        ),
        (1, 12) => (Variant::MefPacked, Packing::msb(12)),
        _ => {
            return Err(Error::Unsupported(format!(
                "ERF or MEF compression {compression} of {bits} bits"
            )));
        }
    };
    let offset = *t
        .ints_upto(raw, 273, 1)
        .unwrap_or_default()
        .first()
        .ok_or_else(|| corrupt("ERF data offset"))?;
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    let colors = t
        .bytes_of(raw, 33422)
        .filter(|p| p.len() == 4 && p.iter().all(|&c| c <= 2))
        .map_or(vec![0, 1, 1, 2], <[u8]>::to_vec);
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    }));
    super::tiff_exif::read(&t, &mut info);
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let notes = epson::read(&t).unwrap_or_default();
    info.cam_mul = notes.cam_mul;
    if let Some((at, len)) = notes.preview {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + at,
            len,
            pixels: 0,
        });
    }
    // The camera's image, centred in the sensor's.
    if let Some([w, h]) = notes.size.filter(|&[w, h]| w <= width && h <= height) {
        info.frame = [(width - w) / 2, (height - h) / 2, w, h];
    }
    let black = notes.black.unwrap_or([0; 4]);
    if black.iter().any(|&b| b > 0xffff) {
        return Err(corrupt("ERF black level"));
    }
    info.plan = Plan::Erf(ErfPlan {
        offset: t.base + u64::from(offset),
        width,
        height,
        packing,
        black,
        white: crate::facts::camera(&info.make, &info.model, false, [width, height])
            .find_map(|c| c.white)
            .unwrap_or((1 << bits) - 1),
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &ErfPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let width = plan.width as usize;
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let file = Bytes::new(bytes, Order::Little);
    // Truncated files keep what they have.
    let available = (bytes.len() as u64).saturating_sub(plan.offset);
    let src = file.slice(plan.offset, available)?;
    let p = plan.packing;
    packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
    let [left, top] = info.margins;
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [left, top, info.width, info.height],
        black: plan.black,
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, point, short, shorts, tiff};

    const BE: Order = Order::Big;

    /// A big-endian ERF or MEF by `make` of `width × height`: the data
    /// `strip` in a SubIFD (compression `compression`) and, for Epson, a
    /// maker note of `entries` (offsets from the TIFF header).
    fn erf(
        make: &str,
        [width, height]: [u32; 2],
        compression: u16,
        strip: &[u8],
        entries: Vec<E>,
    ) -> Vec<u8> {
        // The blobs: data, a JPEG, the note, then the raw directory. The
        // note's PreviewImage (five bytes, so stored out of the entry) is
        // pointed at the JPEG.
        let jpeg = [0xee, 0xd8, 0xff, 0xd9];
        let mut entries = entries;
        entries.push(E(0x0280, 1, vec![0; 5]));
        let note_len = 8 + dir(&entries, 0, BE).len();
        let at = offsets(&[strip, &jpeg, &vec![0; note_len], &[]]);
        let mut note = b"EPSON\0\x01\0".to_vec();
        let mut body = dir(&entries, at[2] + 8, BE);
        let entry = [shorts(BE, &[0x0280, 1]), 5u32.to_be_bytes().to_vec()].concat();
        let pos = body.windows(8).position(|w| w == entry).unwrap();
        body[pos + 8..pos + 12].copy_from_slice(&at[1].to_be_bytes());
        note.extend(body);
        let raw = dir(
            &[
                long(BE, 256, &[width]),
                long(BE, 257, &[height]),
                short(BE, 258, &[12]),
                short(BE, 259, &[compression]),
                short(BE, 262, &[32803]),
                long(BE, 273, &[at[0]]),
                long(BE, 279, &[strip.len() as u32]),
                E(33422, 1, vec![0, 1, 1, 2]),
            ],
            at[3],
            BE,
        );
        let ifd0 = vec![
            ascii(271, make),
            ascii(272, "R-TEST"),
            long(BE, 330, &[at[3]]),
        ];
        let exif = vec![E(0x927c, 7, vec![0; note.len()])];
        let mut file = tiff(b"MM\0*", BE, ifd0, exif, &[strip, &jpeg, &note, &raw]);
        point(&mut file, BE, 0x927c, note.len(), at[2]);
        file
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn erfs_pad_every_ten_samples_and_read_epson_s_note() {
        // Ten samples 0x123 (15 bytes), a pad byte, then two more.
        let mut row = [0x12, 0x31, 0x23].repeat(5);
        row.push(0xaa);
        row.extend([0x45, 0x64, 0x56]);
        row.resize(20, 0);
        let mut wb = vec![0u8; 48];
        wb.extend(shorts(BE, &[203, 323]));
        wb.resize(256, 0);
        let entries = vec![
            long(BE, 0x020b, &[8]),
            long(BE, 0x020c, &[1]),
            E(
                0x0401,
                4,
                crate::container::test_tiff::longs(BE, &[61, 64, 63, 60]),
            ),
            E(0x0e80, 7, wb),
        ];
        let file = erf("SEIKO EPSON CORP.", [12, 1], 32769, &row, entries);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::ErfPacked);
        assert_eq!(info.make, "Epson");
        assert_eq!(
            &s.data[..],
            &[[0x123; 10].as_slice(), &[0x456, 0x456]].concat()
        );
        assert_eq!(s.black, [61, 64, 60, 63]);
        assert_eq!(info.frame, [2, 0, 8, 1]);
        assert!((info.cam_mul[0] - 1.75633).abs() < 1e-4);
        assert!((info.cam_mul[2] - 2.12387).abs() < 1e-4);
        // Epson's preview starts 0xEE, mended to a JPEG's 0xFF.
        let jpeg = crate::preview(&info, &file).unwrap();
        assert_eq!(jpeg[..2], [0xff, 0xd8]);
    }

    #[test]
    fn mefs_pack_plainly_and_bad_files_do_not_panic() {
        let strip = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let file = erf("Mamiya-OP Co.,Ltd.", [2, 2], 1, &strip, vec![]);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::MefPacked);
        assert_eq!(s.data, [0x123, 0x456, 0x789, 0xabc]);
        let files = [
            file,
            erf("SEIKO EPSON CORP.", [12, 1], 32769, &[0x5a; 20], vec![]),
        ];
        for file in files {
            let info = crate::open(&file).unwrap();
            let cancelled = AtomicBool::new(true);
            assert_eq!(
                crate::decode(&info, &file, &cancelled).unwrap_err(),
                Error::Cancelled
            );
            for end in (0..file.len()).step_by(3) {
                let _ = decode(&file[..end]);
            }
            let mut noisy = file.clone();
            for (i, b) in noisy.iter_mut().enumerate().skip(8) {
                *b ^= (i * 7919 % 251) as u8;
            }
            let _ = decode(&noisy);
        }
    }
}
