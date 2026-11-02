//! Sony's maker notes and private directories: the encrypted SR2SubIFD
//! (white balance as shot and its presets, black and white levels), the
//! raw directory's own Sony tags (levels, white balance, crops), the maker
//! note's image size, and the DSLR-A100's Minolta MRW blocks.
//!
//! Sources: ExifTool's Sony and Minolta tag documentation (tag names and
//! meanings of SR2Private, SR2SubIFD, the raw directory's 0x7000–0x74c8 tags,
//! the maker note's FullImageSize, MRW's PRD and WBG blocks); the SR2SubIFD
//! encryption's keystream (a linear congruential generator seeded from the
//! 32-bit SR2SubIFDKey, then a lagged XOR recurrence) is written from the
//! implementer's prior general knowledge of the format, no decoder's source
//! consulted: ExifTool's documentation only says the directory is encrypted
//! and that SR2SubIFDKey is its key, and no permitted public description of
//! the algorithm was found. Validated by our own measurements: on every ARW
//! of our test dataset (470 files) the decrypted SR2SubIFD is a well-formed
//! directory, its levels and white balance equal the plain copies newer
//! bodies also write (239 files), and the decoded files match LibRaw's
//! output. Also our own measurements: which value RAWmakase has always read
//! where several exist.
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::{RawInfo, UNSET};
use rawmakase_tiff::kind;

/// IFD0's DNGPrivateData, which a Sony ARW points to its SR2Private
/// directory with.
const DNG_PRIVATE_DATA: u16 = 0xc634;
/// SR2Private's offset, length and key of the SR2SubIFD.
const SR2_OFFSET: u16 = 0x7200;
const SR2_LENGTH: u16 = 0x7201;
const SR2_KEY: u16 = 0x7221;
/// The largest SR2SubIFD decrypted (the largest seen is 62 kB).
const SR2_MAX: u32 = 1 << 20;

/// The keystream words for `key`: 127 words seeded from it, each later word
/// the XOR of the words 127 and 63 before it.
struct Keystream {
    words: [u32; 128],
    at: usize,
}

impl Keystream {
    fn new(key: u32) -> Self {
        let mut words = [0u32; 128];
        let mut k = key;
        for w in &mut words[..4] {
            k = k.wrapping_mul(48_828_125).wrapping_add(1);
            *w = k;
        }
        words[3] = words[3] << 1 | (words[0] ^ words[2]) >> 31;
        for i in 4..127 {
            words[i] = (words[i - 4] ^ words[i - 2]) << 1 | (words[i - 3] ^ words[i - 1]) >> 31;
        }
        Self { words, at: 127 }
    }
    fn next(&mut self) -> u32 {
        let i = self.at;
        self.words[i & 127] = self.words[(i + 1) & 127] ^ self.words[(i + 65) & 127];
        self.at += 1;
        self.words[i & 127]
    }
}

/// Decrypts (or encrypts) `block` in place: every whole 4-byte word XOR the
/// next keystream word, most significant byte first; a shorter tail is
/// stored as it is.
pub(crate) fn decrypt(block: &mut [u8], key: u32) {
    let mut stream = Keystream::new(key);
    for word in block.as_chunks_mut::<4>().0 {
        for (b, k) in word.iter_mut().zip(stream.next().to_be_bytes()) {
            *b ^= k;
        }
    }
}

/// A decrypted SR2SubIFD: a TIFF directory whose value offsets are file
/// offsets into the block itself.
pub(crate) struct Sr2 {
    start: u64,
    plain: Vec<u8>,
    little: bool,
}

impl Sr2 {
    /// The file's SR2SubIFD, decrypted; `None` when it has none or it is not
    /// inside the file.
    pub fn read(t: &TiffFile) -> Option<Self> {
        let private = t.bytes_of(&t.ifds[0], DNG_PRIVATE_DATA)?;
        let at = u64::from(t.u32(private.get(..4)?));
        let ifd = t.ifd_at(at)?;
        let start = t.int(&ifd, SR2_OFFSET)?;
        let len = t.int(&ifd, SR2_LENGTH)?.min(SR2_MAX);
        let key = t.bytes_of(&ifd, SR2_KEY).filter(|k| k.len() == 4)?;
        let mut plain = t
            .bytes
            .slice(u64::from(start), u64::from(len))
            .ok()?
            .to_vec();
        decrypt(&mut plain, t.u32(key));
        Some(Self {
            start: u64::from(start),
            plain,
            little: t.little(),
        })
    }
    fn u16(&self, b: &[u8]) -> u16 {
        if self.little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        }
    }
    fn u32(&self, b: &[u8]) -> u32 {
        if self.little {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    }
    /// The type and value bytes of `tag`.
    fn entry(&self, tag: u16) -> Option<(u16, &[u8])> {
        let n = usize::from(self.u16(self.plain.get(..2)?));
        let entry = (0..n)
            .map_while(|i| self.plain.get(2 + 12 * i..14 + 12 * i))
            .find(|e| self.u16(e) == tag)?;
        let kind = self.u16(&entry[2..]);
        let size = rawmakase_tiff::Entry {
            kind,
            count: 0,
            value: [0; 4],
        }
        .type_size()?;
        let len = usize::try_from(self.u32(&entry[4..]))
            .ok()?
            .checked_mul(size)?;
        let value = if len <= 4 {
            &entry[8..8 + len]
        } else {
            let at = u64::from(self.u32(&entry[8..])).checked_sub(self.start)?;
            let at = usize::try_from(at).ok()?;
            self.plain.get(at..at.checked_add(len)?)?
        };
        Some((kind, value))
    }
    /// The values of a SHORT or SSHORT tag, as stored.
    pub fn shorts(&self, tag: u16) -> Option<Vec<u16>> {
        let (k, v) = self.entry(tag)?;
        matches!(k, kind::SHORT | kind::SSHORT)
            .then(|| v.as_chunks::<2>().0.iter().map(|c| self.u16(c)).collect())
    }
}

/// Whether the file has Sony's private directory (SR2Private).
pub(crate) fn has_private(t: &TiffFile) -> bool {
    t.ifds[0].has(DNG_PRIVATE_DATA)
}

/// The black and white levels the file records.
#[derive(Clone, Debug, Default)]
pub(crate) struct Levels {
    /// Red, green, blue, second green.
    pub black: Option<[u32; 4]>,
    pub white: Option<u32>,
    pub linear_white: [u32; 4],
}

/// The raw directory's Sony tags.
const BLACK_LEVEL: u16 = 0x7310;
const WB_RGGB: u16 = 0x7313;
const CROP_TOP_LEFT: u16 = 0x74c7;
const CROP_SIZE: u16 = 0x74c8;
/// DNG's DefaultCropOrigin and DefaultCropSize, in the same directory.
const DEFAULT_CROP_ORIGIN: u16 = 0xc61f;
const DEFAULT_CROP_SIZE: u16 = 0xc620;
const WHITE_LEVEL: u16 = 50717;
/// The SR2SubIFD's older levels: black, and as-shot white balance green,
/// red, blue, green.
const BLACK_LEVEL_OLD: u16 = 0x7300;
const WB_GRBG: u16 = 0x7303;
const SR2_WHITE: u16 = 0x787f;
/// White balance presets (red, green, blue): daylight, shade, and the
/// daylight fluorescent lamp, D65's.
const PRESET_DAYLIGHT: u16 = 0x7820;
const PRESET_SHADE: u16 = 0x7825;
const PRESET_D65: u16 = 0x7827;
/// The oldest bodies' daylight preset.
const PRESET_DAYLIGHT_OLD: u16 = 0x7480;
/// The maker note's FullImageSize: height, width.
const FULL_IMAGE_SIZE: u16 = 0xb02b;

/// Reads the facts Sony keeps in its private directories into `info`: white
/// balance and presets, the default crop; returns the levels.
pub(crate) fn read(t: &TiffFile, raw: &Ifd, info: &mut RawInfo) -> Levels {
    // A style's name is short whatever length the file gives the record.
    info.camera_look = maker_note(t).and_then(|mn| creative_style(&t.raw_upto(&mn, 0xb020, 64)?));
    let sr2 = Sr2::read(t);
    // One value more than any record read, so that a longer one still
    // fails its length check.
    let raw_shorts = |tag| t.ints_upto(raw, tag, 5);
    let sr2_shorts = |tag| {
        sr2.as_ref()
            .and_then(|s| s.shorts(tag))
            .map(|v| v.into_iter().map(u32::from).collect::<Vec<_>>())
    };
    // Red, green, green, blue; the oldest by filter position, rows first
    // (as Adobe DNG Converter's BlackLevel has the DSC-R1's).
    let black = raw_shorts(BLACK_LEVEL)
        .or_else(|| sr2_shorts(BLACK_LEVEL))
        .filter(|b| b.len() == 4)
        .map(|b| [b[0], b[1], b[3], b[2]])
        .or_else(|| {
            let b = sr2_shorts(BLACK_LEVEL_OLD).filter(|b| b.len() == 4)?;
            Some(by_position(&b, info.cfa.as_ref()))
        });
    let white = t.int(raw, WHITE_LEVEL);
    let linear_white = sr2_shorts(SR2_WHITE)
        .filter(|w| w.len() == 3)
        .map(|w| [w[0], w[1], w[2], w[1]])
        .unwrap_or([0; 4]);
    if let Some(wb) = raw_shorts(WB_RGGB)
        .or_else(|| sr2_shorts(WB_RGGB))
        .filter(|w| w.len() == 4)
    {
        info.cam_mul = [wb[0], wb[1], wb[3], wb[2]].map(|v| v as f32);
    } else if let Some(wb) = sr2_shorts(WB_GRBG).filter(|w| w.len() == 4) {
        info.cam_mul = [wb[1], wb[0], wb[2], wb[3]].map(|v| v as f32);
    }
    let preset = |tag| {
        sr2_shorts(tag)
            .filter(|p| p.len() == 3)
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
            .unwrap_or_default()
    };
    info.presets.daylight = preset(PRESET_DAYLIGHT);
    if info.presets.daylight == [0.; 3] {
        info.presets.daylight = preset(PRESET_DAYLIGHT_OLD);
    }
    info.presets.shade = preset(PRESET_SHADE);
    info.presets.d65 = preset(PRESET_D65);
    // Sony names no lens (an adapted or manual one) as dashes.
    if info.lens.trim() == "----" {
        info.lens.clear();
    }
    info.frame = match (raw_shorts(CROP_TOP_LEFT), raw_shorts(CROP_SIZE)) {
        (Some(at), Some(size)) if at.len() == 2 && size.len() == 2 => {
            // The a7R III's and a7R IV's own tag starts at column 0, 8 and 32
            // columns left of what their JPEGs and Camera Raw show. Along a side
            // the crop does not cut, the default crop places it.
            let origin = raw_shorts(DEFAULT_CROP_ORIGIN).filter(|v| v.len() == 2);
            let whole = raw_shorts(DEFAULT_CROP_SIZE).filter(|v| v.len() == 2);
            let place = |axis: usize| match (&origin, &whole) {
                (Some(origin), Some(whole)) if whole[axis] == size[axis] => origin[axis],
                _ => at[axis],
            };
            [place(0), place(1), size[0], size[1]]
        }
        _ => match maker_note(t).and_then(|mn| t.ints(&mn, FULL_IMAGE_SIZE, 2)) {
            Some(s) if s.len() == 2 && s[0] > 0 && s[1] > 0 => [UNSET, UNSET, s[1], s[0]],
            _ => [UNSET, UNSET, 0, 0],
        },
    };
    Levels {
        black,
        white,
        linear_white,
    }
}

/// Levels of the 2 × 2 filter positions (rows first) per channel (red,
/// green, blue, second green); in that order without a 2 × 2 filter of the
/// four channels.
fn by_position(levels: &[u32], cfa: Option<&crate::info::Cfa>) -> [u32; 4] {
    let mut out = [0; 4];
    let mut seen = [false; 4];
    if let Some(cfa) = cfa.filter(|c| c.rows == 2 && c.cols == 2) {
        for (i, &v) in levels.iter().take(4).enumerate() {
            let c = usize::from(cfa.color(i / 2, i % 2)).min(3);
            out[c] = v;
            seen[c] = true;
        }
    }
    if seen.iter().all(|&s| s) {
        out
    } else {
        std::array::from_fn(|i| levels.get(i).copied().unwrap_or(0))
    }
}

/// The DSLR-A100's Minolta MRW blocks, which its SR2Private points to: the
/// sensor's size (PRD), white balance as shot (WBG) and presets (RIF).
pub(crate) struct Mrw {
    pub width: u32,
    pub height: u32,
    pub cam_mul: [f32; 4],
    pub presets: crate::info::Presets,
}

impl Mrw {
    pub fn read(t: &TiffFile) -> Option<Self> {
        let private = t.bytes_of(&t.ifds[0], DNG_PRIVATE_DATA)?;
        let at = u64::from(t.u32(private.get(..4)?));
        let head = t.bytes.slice(at, 8).ok()?;
        // "\0MRM" in Motorola's byte order, "\0MRI" in Intel's.
        let little = match &head[..4] {
            b"\0MRI" => true,
            b"\0MRM" => false,
            _ => return None,
        };
        let u32_at = |b: &[u8]| {
            let b = [b[0], b[1], b[2], b[3]];
            if little {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let u16_at = |b: &[u8], i: usize| {
            b.get(i..i + 2).map(|b| {
                if little {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
        };
        let len = u64::from(u32_at(&head[4..]));
        let body = t.bytes.slice(at + 8, len.min(4096)).ok()?;
        let mut mrw = Self {
            width: 0,
            height: 0,
            cam_mul: [0.; 4],
            presets: Default::default(),
        };
        let mut rest = body;
        while rest.len() >= 8 {
            let size = usize::try_from(u32_at(&rest[4..])).ok()?;
            let block = rest.get(8..8usize.checked_add(size)?)?;
            match &rest[..4] {
                b"\0PRD" => {
                    mrw.height = u32::from(u16_at(block, 8)?);
                    mrw.width = u32::from(u16_at(block, 10)?);
                }
                b"\0WBG" => {
                    // Four scale bytes, then red, green, green, blue.
                    let l: Vec<f32> = (0..4)
                        .map(|i| u16_at(block, 4 + 2 * i).map(f32::from))
                        .collect::<Option<_>>()?;
                    mrw.cam_mul = [l[0], l[1], l[3], l[2]];
                }
                b"\0RIF" => {
                    // Presets' red and blue over a green of 256, where
                    // RAWmakase has always read them (our measurements).
                    let preset = |i: usize| {
                        Some([
                            f32::from(u16_at(block, i)?),
                            256.,
                            f32::from(u16_at(block, i + 2)?),
                        ])
                    };
                    mrw.presets.daylight = preset(0x0c).unwrap_or_default();
                    mrw.presets.shade = preset(0x20).unwrap_or_default();
                    mrw.presets.d65 = preset(0x28).unwrap_or_default();
                }
                _ => {}
            }
            rest = &rest[8 + size..];
        }
        (mrw.width > 0 && mrw.height > 0).then_some(mrw)
    }
}

/// CreativeStyle (0xB020), by the name the camera gives it, its short names in full.
fn creative_style(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes.split(|b| *b == 0).next()?);
    let name = match text.trim() {
        "" | "None" | "n/a" => return None,
        "ST" => "Standard",
        "PT" => "Portrait",
        "NT" => "Neutral",
        "VV" => "Vivid",
        "VV2" => "Vivid 2",
        "BW" => "B&W",
        other => other,
    };
    Some(name.to_owned())
}

/// The maker note's directory: Sony's notes start with "SONY DSC " or "SONY
/// CAM " and three zero bytes before it; its offsets are from the TIFF
/// header.
fn maker_note(t: &TiffFile) -> Option<Ifd> {
    let exif = t.exif.as_ref()?;
    let e = exif.entries.get(&0x927c)?;
    let at = u64::from(t.u32(&e.value));
    let head = t.bytes.slice(at, 12).ok()?;
    let at = if head.starts_with(b"SONY") {
        at + 12
    } else {
        at
    };
    t.ifd_at(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_creative_style_is_named_in_full() {
        assert_eq!(creative_style(b"Portrait\0\0").as_deref(), Some("Portrait"));
        assert_eq!(creative_style(b"PT\0").as_deref(), Some("Portrait"));
        assert_eq!(creative_style(b"\0\0"), None);
    }

    #[test]
    fn older_black_levels_are_by_filter_position() {
        // The DSC-R1's filter starts green, red: its first level is green's.
        let cfa = crate::info::second_green(crate::info::Cfa {
            rows: 2,
            cols: 2,
            colors: vec![1, 0, 2, 1],
        });
        let levels = [514, 511, 512, 513];
        assert_eq!(by_position(&levels, Some(&cfa)), [511, 514, 512, 513]);
        // Without a filter: in the order stored.
        assert_eq!(by_position(&levels, None), levels);
    }

    #[test]
    fn the_keystream_follows_its_recurrence() {
        // Independently: the first four words from the generator, then words
        // 127 on are the XOR of the words 127 and 63 before them.
        let key = 0x4433_2211u32;
        let mut w = vec![];
        let mut k = key;
        for _ in 0..4 {
            k = k.wrapping_mul(48828125).wrapping_add(1);
            w.push(k);
        }
        w[3] = w[3] << 1 | (w[0] ^ w[2]) >> 31;
        for i in 4..127 {
            let v = (w[i - 4] ^ w[i - 2]) << 1 | (w[i - 3] ^ w[i - 1]) >> 31;
            w.push(v);
        }
        // Then each word is the XOR of the words 127 and 63 before it.
        for n in 127..527 {
            let v = w[n - 127] ^ w[n - 63];
            w.push(v);
        }
        let mut s = Keystream::new(key);
        for (n, &want) in w.iter().enumerate().skip(127) {
            assert_eq!(s.next(), want, "word {n}");
        }
    }

    #[test]
    fn decryption_is_its_own_inverse_and_keeps_the_tail() {
        let plain: Vec<u8> = (0..23u8).collect();
        let mut block = plain.clone();
        decrypt(&mut block, 7);
        assert_ne!(block[..20], plain[..20]);
        assert_eq!(block[20..], plain[20..]);
        decrypt(&mut block, 7);
        assert_eq!(block, plain);
    }
}
