//! Olympus and OM Digital Solutions ORF and ORI: a TIFF (magic `IIRO`,
//! `IIRS` or `MMOR`) whose IFD0 is the raw image. How it is coded follows
//! from its bit depth and the size of its data: Olympus's compressed coding,
//! 12-bit samples in 16-byte blocks of ten (12.8 bits a sample) or in 32-bit
//! words, 16-bit words, or (the compacts, photometric 2) two interlaced
//! fields of plainly packed 12-bit rows. The maker note is
//! `vendor::olympus`'s.
//!
//! Sources: TIFF 6, Exif 2.32 (CFAPattern), ExifTool's Olympus tag
//! documentation, and our own measurements of the codings, the fields'
//! layout and the visible areas against LibRaw's output on our files.
use super::tiff::{NAME_MAX, TiffFile};
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::olympus;
use std::sync::atomic::AtomicBool;

/// How an ORF's sensor data is coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    Compressed,
    Packed(Packing),
    /// Two fields of packed rows, the odd rows' at the next 2 KiB boundary
    /// after the even rows'.
    Fields(Packing),
    /// 16-bit words in the file's byte order, holding samples of the note's
    /// bit depth in their high bits.
    Words(Order),
}

/// Where an ORF's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct OrfPlan {
    pub offset: u64,
    pub coding: Coding,
    /// The samples' bit depth.
    pub bits: u32,
    pub width: u32,
    pub height: u32,
    pub black: [u32; 4],
    pub white: u32,
}

/// Whether the TIFF `t` is an Olympus or OM Digital Solutions RAW.
pub(crate) fn is_orf(bytes: &[u8], t: &TiffFile) -> bool {
    if matches!(bytes.get(..4), Some(b"IIRO" | b"IIRS" | b"MMOR")) {
        return true;
    }
    let make = t.text_upto(&t.ifds[0], 271, NAME_MAX).unwrap_or_default();
    let make = make.trim().to_ascii_uppercase();
    make.starts_with("OLYMPUS") || make.starts_with("OM DIGITAL")
}

/// Visible widths by stored width, where the stored rows end in columns
/// RAWmakase has never shown (our measurements, shared by every body of the
/// sensor).
const VISIBLE_WIDTHS: [(u32, u32); 6] = [
    (4080, 4056),
    (4100, 4096),
    (4040, 4014),
    (8180, 8170),
    (8200, 8170),
    (10400, 10388),
];

/// Reads an ORF's header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let width = t.int(ifd0, 256).ok_or_else(|| corrupt("ORF width"))?;
    let stored_height = t.int(ifd0, 257).ok_or_else(|| corrupt("ORF height"))?;
    // An odd number of rows is read as one more.
    let height = stored_height
        .checked_add(stored_height & 1)
        .ok_or(Error::TooLarge("image side"))?;
    crate::io::samples(width, height, 1)?;
    let bps = t.int(ifd0, 258).unwrap_or(16);
    let compression = t.int(ifd0, 259).unwrap_or(1);
    if compression != 1 {
        return Err(Error::Unsupported(format!("ORF compression {compression}")));
    }
    let offsets = t.ints_upto(ifd0, 273, 1).unwrap_or_default();
    let counts = t.pieces(ifd0, 279).unwrap_or_default();
    let offset = *offsets.first().ok_or_else(|| corrupt("ORF data offset"))?;
    let bytes = counts
        .iter()
        .fold(0u64, |sum, &c| sum.saturating_add(u64::from(c)));
    let pixels = u64::from(width) * u64::from(stored_height);
    let order = if t.little() {
        Order::Little
    } else {
        Order::Big
    };
    let (variant, coding) = if t.int(ifd0, 262) == Some(2) && bps == 12 {
        (Variant::OrfPacked, Coding::Fields(Packing::msb(12)))
    } else if bytes.checked_mul(5) == Some(pixels * 8) {
        let p = Packing {
            bits: 12,
            word: 3,
            pad: true,
            swap: true,
        };
        (Variant::OrfPacked, Coding::Packed(p))
    } else if bytes.checked_mul(2) == Some(pixels * 3) {
        let p = Packing {
            word: 4,
            ..Packing::msb(12)
        };
        (Variant::OrfPacked, Coding::Packed(p))
    } else if bps == 16 && bytes >= pixels * 2 {
        (Variant::OrfUnpacked, Coding::Words(order))
    } else if bps == 16 {
        (Variant::OrfCompressed, Coding::Compressed)
    } else {
        return Err(Error::Unsupported(format!("ORF of {bps} bits")));
    };
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = VISIBLE_WIDTHS
        .iter()
        .find(|(w, _)| *w == width)
        .map_or(width, |&(_, v)| v);
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    info.cfa = Some(cfa(&t));
    super::tiff_exif::read(&t, &mut info);
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let notes = olympus::read(&t).unwrap_or_default();
    info.cam_mul = notes.cam_mul;
    info.presets = notes.presets;
    if info.lens.is_empty()
        && let Some(lens) = &notes.lens
    {
        info.lens = lens.clone();
    }
    for &(at, len) in &notes.previews {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + at,
            len,
            pixels: 0,
        });
    }
    if let Some(crop) = notes.crop {
        info.frame = crate::geometry::even(crop);
        if let Some(aspect) = notes.aspect.and_then(|ratio| aspect(crop, ratio)) {
            info.aspect = crate::geometry::even(aspect);
        }
    }
    if let Coding::Fields(p) = coding
        && let Some(jpeg) = trailing_jpeg(t.bytes.data, offset, p, width, height)
    {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + jpeg.0,
            len: jpeg.1,
            pixels: 0,
        });
    }
    let bits = notes.bits.filter(|b| (8..=16).contains(b)).unwrap_or(12);
    // The note's black is on the 12-bit scale, or its own when that is less.
    let black = match notes.black14.filter(|_| bits == 14) {
        Some(b) => b.map(u64::from),
        None => notes
            .black
            .unwrap_or([0; 4])
            .map(|b| u64::from(b) << bits.saturating_sub(12)),
    };
    if black.iter().any(|&b| b > 0xffff) {
        return Err(corrupt("ORF black level"));
    }
    let black = black.map(|b| b as u32);
    let size = [width, height];
    // Rows for files of this bit depth, or any.
    let rows = || {
        crate::facts::camera(&info.make, &info.model, false, size)
            .filter(move |c| c.bits.is_none_or(|b| b == bits))
    };
    let white = rows().find_map(|c| c.white).unwrap_or((1 << bits) - 1);
    // Where the camera's white balance levels are on another scale than
    // Adobe DNG Converter reads them (four compacts).
    let [red, blue] = rows().find_map(|c| c.wb_scale).unwrap_or([1., 1.]);
    info.cam_mul[0] *= red as f32;
    info.cam_mul[2] *= blue as f32;
    // A camera Adobe gives no matrix for: its own, of white balanced data,
    // balanced at D65 by its D65 levels (on the as-shot levels' scale).
    if crate::facts::matrix(&info.make, &info.model).is_none()
        && let Some(m) = &notes.color_matrix
        && let Some([r, g, b]) = notes.daylight.map(|d| d.map(f64::from))
        && g > 0.
    {
        let daylight = [r / g * red, 1., b / g * blue];
        info.file_matrix = crate::color::cam_xyz_of(m, daylight);
    }
    info.plan = Plan::Orf(OrfPlan {
        offset: t.base + u64::from(offset),
        coding,
        bits,
        width,
        height,
        black,
        white,
    });
    Ok(info)
}

/// The camera's aspect-ratio crop, centred in the default crop `[left,
/// top, width, height]`: the ratio `[across, down]` as wide as the crop
/// when wider, else as tall. `None` for the sensor's own 4:3.
fn aspect([left, top, width, height]: [u32; 4], [across, down]: [u32; 2]) -> Option<[u32; 4]> {
    if across == 0 || down == 0 || across * 3 == down * 4 {
        return None;
    }
    let (w, h) = if u64::from(across) * u64::from(height) >= u64::from(down) * u64::from(width) {
        (
            width,
            (u64::from(width) * u64::from(down) / u64::from(across)) as u32,
        )
    } else {
        (
            (u64::from(height) * u64::from(across) / u64::from(down)) as u32,
            height,
        )
    };
    Some([
        left.saturating_add((width - w) / 2),
        top.saturating_add((height - h) / 2),
        w,
        h,
    ])
}

/// The JPEG some compacts store after their fields of sensor data: at the
/// next 2 KiB boundary after the odd rows' field (our measurements), to the
/// end of the file. `(offset, length)` from the TIFF header.
fn trailing_jpeg(
    tiff: &[u8],
    offset: u32,
    p: Packing,
    width: u32,
    height: u32,
) -> Option<(u64, u64)> {
    let stride = p.row_bytes(width as usize) as u64;
    let even = u64::from(height.div_ceil(2));
    let second = (even * stride).next_multiple_of(2048);
    let end = u64::from(offset) + second + u64::from(height / 2) * stride;
    let at = end.next_multiple_of(2048);
    let len = (tiff.len() as u64).checked_sub(at)?;
    crate::preview::is_jpeg(tiff, at).then_some((at, len))
}

/// The colour filter from EXIF's CFAPattern at the sensor's corner,
/// red first when the file has none.
fn cfa(t: &TiffFile) -> Cfa {
    let colors = super::tiff_exif::cfa_pattern(t).unwrap_or(vec![0, 1, 1, 2]);
    crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    })
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &OrfPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let file = Bytes::new(bytes, Order::Little);
    // Truncated files keep what they have.
    let available = (bytes.len() as u64).saturating_sub(plan.offset);
    let src = file.slice(plan.offset, available)?;
    match plan.coding {
        Coding::Compressed => {
            // 14-bit samples' codes start a byte later (our measurements).
            let header = crate::codec::olympus::HEADER + usize::from(plan.bits > 12);
            crate::codec::olympus::decode(src, header, width, height, &mut data, cancel)?;
        }
        Coding::Packed(p) => {
            packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
        }
        Coding::Fields(p) => {
            // The odd rows start at the next 2 KiB after the even rows.
            packed::fields(src, p, width, p.row_bytes(width), &mut data, cancel)?;
        }
        Coding::Words(order) => {
            packed::words(src, order, 16 - plan.bits, &mut data, cancel)?;
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
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, short, shorts, tiff};

    const LE: Order = Order::Little;

    fn point_note(file: &mut [u8], order: Order, len: usize, at: u32) {
        crate::container::test_tiff::point(file, order, 0x927c, len, at);
    }

    /// A little-endian ORF (`IIRO`) of `width × height` and `bps` bits, its
    /// data `strip`, IFD0 entries `more` and a maker note.
    fn orf(size: [u32; 2], bps: u16, strip: &[u8], more: Vec<E>, note: Option<Vec<u8>>) -> Vec<u8> {
        let mut blobs: Vec<&[u8]> = vec![strip];
        if let Some(n) = &note {
            blobs.push(n);
        }
        let at = offsets(&blobs);
        let mut ifd0 = vec![
            ascii(271, "OLYMPUS CORPORATION"),
            ascii(272, "E-TEST"),
            long(LE, 256, &[size[0]]),
            long(LE, 257, &[size[1]]),
            short(LE, 258, &[bps]),
            short(LE, 259, &[1]),
            long(LE, 273, &[at[0]]),
            long(LE, 279, &[strip.len() as u32]),
        ];
        ifd0.extend(more);
        let exif = match &note {
            Some(n) => vec![E(0x927c, 7, vec![0; n.len()])],
            None => vec![ascii(0x9003, "2026:10:17 12:00:00")],
        };
        let mut file = tiff(b"IIRO", LE, ifd0, exif, &blobs);
        if let Some(n) = &note {
            point_note(&mut file, LE, n.len(), at[1]);
        }
        file
    }

    /// A new-style note: `OLYMPUS\0II\3\0`, its main directory pointing at
    /// an ImageProcessing directory of `processing`, offsets from the
    /// note's start.
    fn note(processing: Vec<E>) -> Vec<u8> {
        let mut out = b"OLYMPUS\0II\x03\0".to_vec();
        let main_len = dir(&[E(0x2040, 13, vec![0; 4])], 12, LE).len() as u32;
        let main = [E(0x2040, 13, (12 + main_len).to_le_bytes().to_vec())];
        out.extend(dir(&main, 12, LE));
        out.extend(dir(&processing, 12 + main_len, LE));
        out
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn compressed_orfs_read_the_note_s_processing_facts() {
        // 14-bit samples' codes start a byte later.
        let strip = [&[0][..], &crate::codec::olympus::tests::row_of_four()].concat();
        let processing = vec![
            short(LE, 0x0100, &[512, 384, 256, 260]),
            short(LE, 0x010a, &[452, 528, 256, 256]),
            short(LE, 0x010c, &[508, 500, 256, 256]),
            short(LE, 0x010d, &[530, 414, 256, 256]),
            short(LE, 0x0600, &[255, 254, 253, 252]),
            short(LE, 0x0611, &[14, 0]),
            short(LE, 0x0612, &[1, 0]),
            short(LE, 0x0613, &[0, 0]),
            long(LE, 0x0614, &[3]),
            long(LE, 0x0615, &[2]),
            E(0x1112, 1, vec![3, 1]),
        ];
        let file = orf([4, 3], 16, &strip, vec![], Some(note(processing)));
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::OrfCompressed);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Olympus", "E-TEST")
        );
        // An odd number of rows is read as one more.
        assert_eq!([info.raw_width, info.raw_height], [4, 4]);
        // Red and blue over the mean of the two greens' levels.
        assert_eq!(info.cam_mul, [512., 258., 384., 258.]);
        assert_eq!(info.presets.fine_weather, [452., 256., 528.]);
        assert_eq!(info.presets.d65, [508., 256., 500.]);
        assert_eq!(info.presets.shade, [530., 256., 414.]);
        // The crop moved to an even column, keeping its end; 16:9 centred
        // in the crop, then likewise.
        assert_eq!(info.frame, [2, 0, 2, 2]);
        assert_eq!(info.aspect, [2, 0, 2, 1]);
        // 14 bits: the note's black (red, green, green, blue) on the 12-bit
        // scale, times four.
        assert_eq!(s.black, [1020, 1016, 1008, 1012]);
        assert_eq!(s.white, 16383);
        assert_eq!(s.area, [0, 0, 4, 4]);
        // (The later rows decode what follows the data, the note.)
        assert_eq!(s.data[..4], [21, 65527, 97, 65525]);
    }

    #[test]
    fn a_camera_without_a_matrix_row_takes_the_note_s_own() {
        // Camera to sRGB of white balanced data in 1/256 (rows of 256, signed
        // values), and the D65 levels 512 and 384 over 256: daylight 2, 1, 1.5.
        let m: [i16; 9] = [300, -44, 0, -20, 286, -10, 0, -56, 312];
        let processing = vec![
            short(LE, 0x010c, &[512, 384, 256, 256]),
            short(LE, 0x0200, &m.map(|v| v as u16)),
        ];
        let strip = crate::codec::olympus::tests::row_of_four();
        let (info, _) = decode(&orf([4, 4], 16, &strip, vec![], Some(note(processing)))).unwrap();
        // The matrix and the multipliers come back from the XYZ matrix.
        for i in 0..3 {
            for j in 0..3 {
                let want = f32::from(m[i * 3 + j]) / 256.;
                assert!(
                    (info.rgb_cam[i][j] - want).abs() < 1e-5,
                    "{:?}",
                    info.rgb_cam
                );
            }
        }
        for (got, want) in info.pre_mul.iter().zip([2., 1., 1.5, 1.]) {
            assert!((got - want).abs() < 1e-5, "{:?}", info.pre_mul);
        }
        // D65's white (sRGB's white point) reaches the camera as the
        // reciprocals of its multipliers.
        let white = [0.95047, 1., 1.08883];
        for (row, want) in info.cam_xyz.iter().zip([0.5, 1., 1. / 1.5]) {
            let got: f32 = row.iter().zip(white).map(|(a, b)| a * b).sum();
            assert!((got - want).abs() < 1e-4, "{:?}", info.cam_xyz);
        }
        // No D65 levels: no matrix.
        let processing = vec![short(LE, 0x0200, &m.map(|v| v as u16))];
        let (info, _) = decode(&orf([4, 4], 16, &strip, vec![], Some(note(processing)))).unwrap();
        assert_eq!(info.cam_xyz, [[0.; 3]; 3]);
        assert_eq!(info.pre_mul, [1.; 4]);
    }

    #[test]
    fn fourteen_bit_samples_take_their_own_black() {
        let strip = [&[0][..], &crate::codec::olympus::tests::row_of_four()].concat();
        let processing = vec![
            short(LE, 0x0600, &[255, 254, 253, 252]),
            short(LE, 0x0606, &[1019, 1018, 1017, 1016]),
            short(LE, 0x0611, &[14, 0]),
        ];
        let file = orf([4, 3], 16, &strip, vec![], Some(note(processing)));
        let (_, s) = decode(&file).unwrap();
        // Red, green, green, blue as red, green, blue, second green.
        assert_eq!(s.black, [1019, 1018, 1016, 1017]);
    }

    #[test]
    fn twelve_bit_compressed_rows_start_after_seven_bytes() {
        let strip = crate::codec::olympus::tests::row_of_four();
        let (info, s) = decode(&orf([4, 4], 16, &strip, vec![], None)).unwrap();
        assert_eq!(info.variant, Variant::OrfCompressed);
        assert_eq!(&s.data[..4], &[21, 65527, 97, 65525]);
        assert_eq!((s.black, s.white), ([0; 4], 4095));
    }

    #[test]
    fn extreme_green_levels_neither_wrap_nor_panic() {
        let strip = crate::codec::olympus::tests::row_of_four();
        let levels = vec![long(LE, 0x0100, &[512, 768, u32::MAX, u32::MAX])];
        let file = orf([4, 3], 16, &strip, vec![], Some(note(levels)));
        let info = crate::open(&file).unwrap();
        assert_eq!(info.cam_mul[1], u32::MAX as f32);
        assert_eq!([info.cam_mul[0], info.cam_mul[2]], [512., 768.]);
    }

    #[test]
    fn packed_codings_follow_the_data_size() {
        // 16-byte blocks of ten 12-bit samples (12.8 bits a sample): 3-byte
        // little-endian words read from the top, pairs swapped, a pad byte.
        let mut block = Vec::new();
        for pair in 0..5u32 {
            let (a, b) = (0x100 + pair, 0x200 + pair);
            block.extend(&(a << 12 | b).to_le_bytes()[..3]);
        }
        block.push(0);
        let file = orf([10, 2], 16, &[block.clone(), block].concat(), vec![], None);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::OrfPacked);
        assert_eq!(&s.data[..4], &[0x200, 0x100, 0x201, 0x101]);
        // 12-bit samples in 32-bit little-endian words, 1.5 bytes a sample.
        let words: Vec<u8> = [0x1234_5678u32, 0x9abc_def0, 0x1357_9bdf]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let file = orf([8, 2], 12, &[words.clone(), words].concat(), vec![], None);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::OrfPacked);
        assert_eq!(
            &s.data[..8],
            &[0x123, 0x456, 0x789, 0xabc, 0xdef, 0x013, 0x579, 0xbdf]
        );
    }

    #[test]
    fn compacts_store_two_fields_of_rows() {
        // 4 × 3 (read as 4 rows): rows 0 and 2, then from 2 KiB on rows 1
        // and 3, two 12-bit samples in three bytes.
        let row = |v: u32| -> Vec<u8> { (v << 12 | v).to_be_bytes()[1..].repeat(2) };
        let mut strip = [row(1), row(3)].concat();
        strip.resize(2048, 0);
        strip.extend([row(2), row(4)].concat());
        let file = orf([4, 3], 12, &strip, vec![short(LE, 262, &[2])], None);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::OrfPacked);
        assert_eq!(s.data, [1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4]);
    }

    #[test]
    fn old_notes_give_bit_depth_black_and_balance() {
        // Big-endian (MMOR) 16-bit words of 10-bit samples in their high
        // bits; an old-style note, its offsets from the TIFF header.
        let be = Order::Big;
        let words = shorts(be, &[100 << 6, 200 << 6, 300 << 6, 400 << 6]);
        let note_at = 8 + words.len() as u32;
        let main = [
            E(0x1012, 3, shorts(be, &[32, 33, 34, 35])),
            E(0x1017, 3, shorts(be, &[396, 64])),
            E(0x1018, 3, shorts(be, &[368, 64])),
            E(0x102c, 3, shorts(be, &[10, 0])),
        ];
        let mut note = b"OLYMP\0\x01\0".to_vec();
        note.extend(dir(&main, note_at + 8, be));
        let at = offsets(&[&words, &note]);
        assert_eq!(at[1], note_at);
        let ifd0 = vec![
            ascii(271, "OLYMPUS OPTICAL CO.,LTD"),
            ascii(272, "E-20,E-20N,E-20P"),
            long(be, 256, &[2]),
            long(be, 257, &[2]),
            short(be, 258, &[16]),
            long(be, 273, &[at[0]]),
            long(be, 279, &[8]),
        ];
        let exif = vec![E(0x927c, 7, vec![0; note.len()])];
        let mut file = tiff(b"MMOR", be, ifd0, exif, &[&words, &note]);
        point_note(&mut file, be, note.len(), note_at);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::OrfUnpacked);
        assert_eq!(info.normalized_model, "E-20");
        assert_eq!(s.data, [100, 200, 300, 400]);
        assert_eq!((s.black, s.white), ([32, 33, 35, 34], 1023));
        assert_eq!(info.cam_mul, [396., 256., 368., 256.]);
    }

    #[test]
    fn extreme_note_values_neither_wrap_nor_panic() {
        let strip = crate::codec::olympus::tests::row_of_four();
        let crop = |v: u32| {
            vec![
                long(LE, 0x0612, &[v]),
                long(LE, 0x0613, &[v]),
                long(LE, 0x0614, &[v]),
                long(LE, 0x0615, &[v]),
                E(0x1112, 1, vec![2, 1]),
            ]
        };
        let file = orf([4, 3], 16, &strip, vec![], Some(note(crop(u32::MAX))));
        let info = crate::open(&file).unwrap();
        assert_eq!(info.frame, [u32::MAX, u32::MAX, u32::MAX - 1, u32::MAX - 1]);
        // A black level that does not fit 16 bits on the samples' scale.
        let black = vec![long(LE, 0x0600, &[0x4000; 4]), short(LE, 0x0611, &[14, 0])];
        let file = orf([4, 3], 16, &strip, vec![], Some(note(black)));
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
        // The most rows a height says, one more when odd.
        let file = orf([4, u32::MAX], 16, &strip, vec![], None);
        assert!(crate::open(&file).is_err());
    }

    #[test]
    fn decoding_stops_when_cancelled() {
        let strip = crate::codec::olympus::tests::row_of_four();
        let files = [
            orf([4, 4], 16, &strip, vec![], None),
            orf([10, 2], 16, &[0x5a; 32], vec![], None),
            orf([4, 3], 12, &[0xa5; 2100], vec![short(LE, 262, &[2])], None),
            orf([4, 2], 16, &[7; 16], vec![], None),
        ];
        for file in files {
            let info = crate::open(&file).unwrap();
            let cancelled = AtomicBool::new(true);
            assert_eq!(
                crate::decode(&info, &file, &cancelled).unwrap_err(),
                Error::Cancelled,
                "{:?}",
                info.variant
            );
        }
    }

    #[test]
    fn truncated_and_garbage_files_are_errors_not_panics() {
        let strip = crate::codec::olympus::tests::row_of_four();
        let processing = vec![
            short(LE, 0x0100, &[512, 384, 256, 256]),
            short(LE, 0x0611, &[14, 0]),
            E(0x1112, 1, vec![9, 1]),
        ];
        let files = [
            orf([4, 1], 16, &strip, vec![], Some(note(processing))),
            orf([10, 2], 16, &[0x5a; 32], vec![], None),
            orf([4, 3], 12, &[0xa5; 2100], vec![short(LE, 262, &[2])], None),
            orf([4, 2], 16, &[7; 16], vec![], None),
        ];
        for file in files {
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
