//! Samsung's SRW maker notes: a plain directory whose offsets count from
//! the note's start. Several of its vectors are stored offset by the
//! EncryptionKey's eleven numbers: white balance as shot, the presets and
//! the black levels hold value plus key, the levels' RawData value minus
//! key.
//!
//! Sources: ExifTool's Samsung tag documentation (EncryptionKey 0xA020,
//! WB_RGGBLevelsUncorrected 0xA021, WB_RGGBLevelsIlluminator2 0xA024,
//! WB_RGGBLevelsBlack 0xA028); our own measurements of the keys' use (the
//! element each value is offset by) and of which values RAWmakase has
//! always read.
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::Presets;

const KEY: u16 = 0xa020;
const WB_AS_SHOT: u16 = 0xa021;
const WB_D65: u16 = 0xa024;
const BLACK: u16 = 0xa028;
const RAW_DATA: u16 = 0xa048;

/// What a Samsung maker note says.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    /// As-shot multipliers: red, green, blue, second green.
    pub cam_mul: [f32; 4],
    pub presets: Presets,
    /// Black of red, green, blue and the second green.
    pub black: Option<[i64; 4]>,
    /// The white level.
    pub white: Option<i64>,
}

/// The note the EXIF directory's MakerNote holds.
pub(crate) fn read(t: &TiffFile) -> Option<Notes> {
    let entry = t.exif.as_ref()?.entries.get(&0x927c)?;
    let view = TiffFile::view(t.bytes.data, t.offset_of(entry)?, t.bytes.order)?;
    let ifd = view.ifd_at(0)?;
    Some(notes(&view, &ifd))
}

fn notes(v: &TiffFile, ifd: &Ifd) -> Notes {
    let key = v
        .ints(ifd, KEY, 11)
        .filter(|k| k.len() == 11)
        .unwrap_or(vec![0; 11]);
    // A vector's elements less (`sign` −1) or plus (+1) the key, starting
    // at key element `from`; 32-bit wrapping, as the file stores them. Only
    // the first `n` are read.
    let keyed = |tag, sign: i64, from: usize, n: usize| {
        v.ints_upto(ifd, tag, n).map(|l| {
            l.iter()
                .enumerate()
                .map(|(i, &x)| {
                    i64::from((i64::from(x) + sign * i64::from(key[(i + from) % 11])) as i32)
                })
                .collect::<Vec<_>>()
        })
    };
    let mut out = Notes::default();
    // Red, green, green, blue.
    if let Some(l) = keyed(WB_AS_SHOT, -1, 0, 4).filter(|l| l.len() >= 4) {
        out.cam_mul = [l[0], l[1], l[3], l[2]].map(|x| x as f32);
    }
    if let Some(l) = keyed(WB_D65, -1, 1, 4).filter(|l| l.len() >= 4) {
        // Levels on a scale of 4096, or of 256 where red and blue are below
        // a quarter of green (our measurements).
        let green = if l[0].max(l[3]) < l[1] / 4 { 256 } else { l[1] };
        out.presets.d65 = [l[0] as f32, green as f32, l[3] as f32];
    }
    out.black = keyed(BLACK, -1, 0, 4)
        .filter(|l| l.len() >= 4)
        .map(|l| [l[0], l[1], l[3], l[2]]);
    out.white = keyed(RAW_DATA, 1, 0, 5).and_then(|l| l.get(4).copied());
    out
}
