//! Pentax PEF (and the Samsung GX bodies' PEFs): a TIFF whose IFD0 is the
//! raw image, coded with Pentax's Huffman-coded differences (compression
//! 65535), as plainly packed 12-bit rows (compression 32773 despite its
//! name, or 1), or as 16-bit words. The maker note is `vendor::pentax`'s.
//!
//! Sources: TIFF 6, Exif 2.32 (CFAPattern), ExifTool's Pentax tag
//! documentation, and our own measurements of the codings against LibRaw's
//! output on our files.
use super::tiff::{NAME_MAX, TiffFile};
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::pentax;
use std::sync::atomic::AtomicBool;

/// Where a PEF's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct PefPlan {
    pub offset: u64,
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    /// The maker note's HuffmanTable and its byte order (Huffman data).
    pub huffman: Option<(Vec<u8>, Order)>,
    /// 16-bit words' byte order.
    pub order: Order,
    pub black: [u32; 4],
    pub white: u32,
}

/// Whether the TIFF `t` is a PEF: Pentax's make, or a Samsung or Ricoh
/// body with a Pentax maker note.
pub(crate) fn is_pef(t: &TiffFile) -> bool {
    let make = t.text_upto(&t.ifds[0], 271, NAME_MAX).unwrap_or_default();
    let make = make.trim().to_ascii_uppercase();
    make.starts_with("PENTAX")
        || ((make.starts_with("SAMSUNG") || make.starts_with("RICOH")) && pentax::read(t).is_some())
}

/// Reads a PEF's header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let width = t.int(ifd0, 256).ok_or_else(|| corrupt("PEF width"))?;
    let height = t.int(ifd0, 257).ok_or_else(|| corrupt("PEF height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(ifd0, 258).unwrap_or(12);
    if !(8..=16).contains(&bits) {
        return Err(Error::Unsupported(format!("PEF of {bits} bits")));
    }
    let compression = t.int(ifd0, 259).unwrap_or(1);
    if t.int(ifd0, 262) == Some(34892) {
        return Err(Error::Unsupported("PEF of linear (monochrome) data".into()));
    }
    let offset = *t
        .ints_upto(ifd0, 273, 1)
        .unwrap_or_default()
        .first()
        .ok_or_else(|| corrupt("PEF data offset"))?;
    let bytes = t
        .pieces(ifd0, 279)
        .unwrap_or_default()
        .iter()
        .fold(0u64, |sum, &c| sum.saturating_add(u64::from(c)));
    let pixels = u64::from(width) * u64::from(height);
    let make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    let model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // Ricoh Imaging's Pentax bodies are Pentax's.
    let pentax = make.trim().to_ascii_uppercase().starts_with("PENTAX")
        || model.to_ascii_uppercase().starts_with("PENTAX ");
    // Samsung's GX bodies' crop is not read: RAWmakase has always shown
    // their whole sensor (our measurements).
    let mut notes = pentax::read(&t).unwrap_or_default();
    if !pentax {
        notes.crop = None;
    }
    let variant = match compression {
        65535 => Variant::PefHuffman,
        1 if bytes >= pixels * 2 => Variant::PefUnpacked,
        1 | 32773 if bits == 12 => Variant::PefPacked,
        _ => {
            return Err(Error::Unsupported(format!(
                "PEF compression {compression} of {bits} bits"
            )));
        }
    };
    if variant == Variant::PefHuffman && notes.huffman.is_none() {
        return Err(corrupt("PEF without its Huffman table"));
    }
    let mut info = crate::info::base(variant);
    info.make = make;
    info.model = model;
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    info.cfa = Some(cfa(&t));
    super::tiff_exif::read(&t, &mut info);
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    info.cam_mul = notes.cam_mul;
    info.presets = notes.presets;
    info.lens_id = notes.lens_id;
    // The note's ISO code only without an EXIF ISO speed, which Adobe DNG
    // Converter keeps where the two differ.
    if let Some(iso) = notes.iso.filter(|_| info.iso <= 0.) {
        info.iso = iso;
    }
    // The visible area first: the note's crop is in sensor coordinates.
    crate::geometry::apply(&mut info);
    if let Some(crop) = notes.crop {
        info.frame = crate::geometry::even(crop);
    }
    let size = [width, height];
    // A body whose note's crop is not where its JPEGs are.
    if let Some(frame) =
        crate::facts::camera(&info.make, &info.model, false, size).find_map(|c| c.frame)
    {
        info.frame = frame;
    }
    // Rows for files of this bit depth, or any.
    let rows = || {
        crate::facts::camera(&info.make, &info.model, false, size)
            .filter(move |c| c.bits.is_none_or(|b| b == bits))
    };
    let black = rows()
        .find_map(|c| c.black)
        .map(|b| [b; 4])
        .or(notes.black)
        .unwrap_or([0; 4]);
    if black.iter().any(|&b| b > 0xffff) {
        return Err(corrupt("PEF black level"));
    }
    // The camera table's (Adobe DNG Converter's, where it is not the file's),
    // else the white the maker note records, within the data's range.
    let full = (1 << bits) - 1;
    let white = rows()
        .find_map(|c| c.white)
        .or(notes.white.filter(|&w| w <= full))
        .unwrap_or(full);
    info.plan = Plan::Pef(PefPlan {
        offset: t.base + u64::from(offset),
        width,
        height,
        bits,
        huffman: notes.huffman,
        order: if t.little() {
            Order::Little
        } else {
            Order::Big
        },
        black,
        white,
    });
    Ok(info)
}

/// The colour filter from EXIF's CFAPattern at the sensor's corner,
/// blue first when the file has none.
fn cfa(t: &TiffFile) -> Cfa {
    let colors = super::tiff_exif::cfa_pattern(t).unwrap_or(vec![2, 1, 1, 0]);
    crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    })
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &PefPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let file = Bytes::new(bytes, Order::Little);
    // Truncated files keep what they have.
    let available = (bytes.len() as u64).saturating_sub(plan.offset);
    let src = file.slice(plan.offset, available)?;
    match info.variant {
        Variant::PefHuffman => {
            let (table, order) = plan
                .huffman
                .as_ref()
                .ok_or_else(|| corrupt("PEF without its Huffman table"))?;
            let codes = crate::codec::pentax::Codes::from_note(table, *order)?;
            crate::codec::pentax::decode(src, &codes, width, height, &mut data, cancel)?;
        }
        Variant::PefPacked => {
            let p = Packing::msb(plan.bits);
            packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
        }
        Variant::PefUnpacked => packed::words(src, plan.order, 0, &mut data, cancel)?,
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not a PEF variant",
                v.name()
            )));
        }
    }
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

    const LE: Order = Order::Little;

    /// A Huffman table of three codes: "0" for a difference of no bits, "10"
    /// of one bit and "11" of two.
    fn table() -> Vec<u8> {
        let mut t = 7u16.to_le_bytes().to_vec();
        t.extend([0; 12]);
        t.extend(shorts(LE, &[0, 0x800, 0xc00]));
        t.extend([1, 2, 2]);
        t
    }

    /// A new-style note (`PENTAX \0II`, offsets from its start) of `entries`.
    fn note(entries: Vec<E>) -> Vec<u8> {
        let mut out = b"PENTAX \0II".to_vec();
        out.extend(dir(&entries, 10, LE));
        out
    }

    /// A little-endian PEF by `make` and `model` of `width × height` coded `bps` bits with
    /// `compression`, its data `strip`, IFD0 entries `more` and a note.
    fn pef(
        [make, model]: [&str; 2],
        [width, height, bps, compression]: [u32; 4],
        strip: &[u8],
        more: Vec<E>,
        note: Vec<u8>,
    ) -> Vec<u8> {
        let at = offsets(&[strip, &note]);
        let mut ifd0 = vec![
            ascii(271, make),
            ascii(272, model),
            long(LE, 256, &[width]),
            long(LE, 257, &[height]),
            short(LE, 258, &[bps as u16]),
            short(LE, 259, &[compression as u16]),
            short(LE, 262, &[32803]),
            long(LE, 273, &[at[0]]),
            long(LE, 279, &[strip.len() as u32]),
        ];
        ifd0.extend(more);
        let exif = vec![
            E(0x927c, 7, vec![0; note.len()]),
            E(41730, 7, vec![2, 0, 2, 0, 0, 1, 1, 2]),
            short(LE, 34855, &[400]),
        ];
        let mut file = tiff(b"II*\0", LE, ifd0, exif, &[strip, &note]);
        point(&mut file, LE, 0x927c, note.len(), at[1]);
        file
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn huffman_pefs_read_their_note() {
        // "11" 2 bits "11": +3; then "10" "1": +1, and zeros after.
        let strip = [0b1111_1010, 0];
        let mut levels = vec![2u8];
        for (id, l) in [
            (0u8, [100u16, 8192, 8192, 200]),
            (1, [300, 8192, 8192, 400]),
            (5, [500, 8192, 8192, 600]),
        ] {
            levels.push(id);
            levels.extend(shorts(LE, &l));
        }
        let entries = vec![
            short(LE, 0x0014, &[13]),
            short(LE, 0x0038, &[3, 1]),
            short(LE, 0x0039, &[2, 2]),
            E(0x003f, 1, vec![7, 222]),
            short(LE, 0x0200, &[1, 2, 3, 4]),
            short(LE, 0x0201, &[10656, 8448, 8448, 25216]),
            E(0x0220, 7, table()),
            E(0x022d, 7, levels),
        ];
        let file = pef(
            ["RICOH IMAGING COMPANY, LTD.", "PENTAX K-TEST"],
            [4, 4, 14, 65535],
            &strip,
            vec![],
            note(entries),
        );
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::PefHuffman);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Pentax", "K-TEST")
        );
        assert_eq!(&s.data[..4], &[3, 1, 3, 1]);
        assert_eq!(info.lens_id, 7 * 256 + 222);
        // EXIF's 400 over the note's ISO code 13 (500), as Adobe takes it.
        assert_eq!(info.iso, 400.);
        assert_eq!(info.cam_mul, [10656., 8448., 25216., 8448.]);
        assert_eq!(info.presets.daylight, [100., 8192., 200.]);
        assert_eq!(info.presets.shade, [300., 8192., 400.]);
        assert_eq!(info.presets.d65, [500., 8192., 600.]);
        // Red, green, green, blue as red, green, blue, second green.
        assert_eq!((s.black, s.white), ([1, 2, 4, 3], 16383));
        // An odd crop corner moves on by one, keeping its end.
        assert_eq!(info.frame, [4, 2, 1, 1]);
        assert_eq!(info.cfa.as_ref().unwrap().colors, [0, 1, 3, 2]);
    }

    /// The white level the note records is the white of a camera the table
    /// has no row for, within the data's range; without it, the full range.
    #[test]
    fn the_notes_white_level_is_the_white() {
        let strip = [0b1111_1010, 0];
        for (white, want) in [(Some(16124), 16124), (Some(20000), 16383), (None, 16383)] {
            let mut entries = vec![E(0x0220, 7, table())];
            if let Some(w) = white {
                entries.push(long(LE, 0x007e, &[w]));
            }
            let file = pef(
                ["RICOH IMAGING COMPANY, LTD.", "PENTAX K-TEST"],
                [4, 4, 14, 65535],
                &strip,
                vec![],
                note(entries),
            );
            assert_eq!(decode(&file).unwrap().1.white, want, "{white:?}");
        }
    }

    #[test]
    fn packed_and_word_pefs_and_samsung_bodies() {
        // Plainly packed 12-bit rows under compression 32773.
        let strip = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let (info, s) = decode(&pef(
            ["SAMSUNG TECHWIN", "GX-TEST"],
            [2, 2, 12, 32773],
            &strip,
            vec![],
            note(vec![
                short(LE, 0x0201, &[2, 1, 1, 3]),
                short(LE, 0x0038, &[2, 2]),
                short(LE, 0x0039, &[1, 1]),
            ]),
        ))
        .unwrap();
        assert_eq!(info.variant, Variant::PefPacked);
        assert_eq!(s.data, [0x123, 0x456, 0x789, 0xabc]);
        // A Samsung body's note is read, but not its crop.
        assert_eq!(info.cam_mul, [2., 1., 3., 1.]);
        assert_eq!(info.frame[2], 0);
        // 16-bit words.
        let words = shorts(LE, &[1, 2, 3, 4]);
        let (info, s) = decode(&pef(
            ["PENTAX", "PENTAX K-TEST"],
            [2, 2, 12, 1],
            &words,
            vec![],
            note(vec![]),
        ))
        .unwrap();
        assert_eq!(info.variant, Variant::PefUnpacked);
        assert_eq!(s.data, [1, 2, 3, 4]);
    }

    #[test]
    fn monochrome_and_bad_levels_are_errors() {
        let mono = pef(
            ["PENTAX", "PENTAX K-TEST"],
            [2, 2, 12, 1],
            &[0; 8],
            vec![short(LE, 262, &[34892])],
            note(vec![]),
        );
        assert!(matches!(crate::open(&mono), Err(Error::Unsupported(_))));
        let black = note(vec![long(LE, 0x0200, &[1 << 16; 4])]);
        let file = pef(
            ["PENTAX", "PENTAX K-TEST"],
            [2, 2, 12, 1],
            &[0; 8],
            vec![],
            black,
        );
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
        let crop = note(vec![
            long(LE, 0x0038, &[u32::MAX; 2]),
            long(LE, 0x0039, &[u32::MAX; 2]),
        ]);
        let info = crate::open(&pef(
            ["PENTAX", "PENTAX K-TEST"],
            [2, 2, 12, 1],
            &[0; 8],
            vec![],
            crop,
        ))
        .unwrap();
        assert_eq!(info.frame, [u32::MAX, u32::MAX, u32::MAX - 1, u32::MAX - 1]);
        let file = pef(
            ["PENTAX", "PENTAX K-TEST"],
            [4, 4, 14, 65535],
            &[0; 8],
            vec![],
            note(vec![]),
        );
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
    }

    #[test]
    fn decoding_stops_when_cancelled_and_bad_files_do_not_panic() {
        let files = [
            pef(
                ["PENTAX", "PENTAX K-TEST"],
                [4, 4, 14, 65535],
                &[0xfa; 8],
                vec![],
                note(vec![E(0x0220, 7, table())]),
            ),
            pef(
                ["PENTAX", "PENTAX K-TEST"],
                [2, 2, 12, 32773],
                &[0x5a; 6],
                vec![],
                note(vec![]),
            ),
            pef(
                ["PENTAX", "PENTAX K-TEST"],
                [2, 2, 12, 1],
                &[7; 8],
                vec![],
                note(vec![]),
            ),
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
