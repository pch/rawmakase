//! Nikon's small RAW (NEF "RAW S"): luma and chroma, two pixels' luma and
//! one chroma pair in six bytes, converted to RGB through a tone curve, as
//! RAWmakase has always read it.
//!
//! Each pair of pixels stores Y₀, Y₁, Cb and Cr, 12 bits each, least
//! significant bit first; the second pixel's chroma is the mean (rounded
//! down) of its pair's and the next pair's. RGB comes from ITU-R BT.601's
//! YCbCr coefficients over luma ÷ 2549 and chroma ÷ 1536 (centred on 2048),
//! scaled by 3072 into a 3072-step tone curve whose top is 16287; from a luma
//! of 2047 the chroma is ignored.
//!
//! Sources: ITU-R BT.601 (the YCbCr coefficients); our own measurements: the
//! layout, the scales, the highlight rule and the curve, recovered from
//! LibRaw's output on the dataset's small RAWs and on synthetic ones (the
//! curve step by step). The curve is a compat-observed fact: LibRaw's output,
//! shipped so small RAWs keep the values saved edits were made on; the file
//! does not carry it. The rounding of LibRaw's arithmetic is not known: about
//! one value in ten thousand falls one to four curve steps from LibRaw's.
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// The tone curve's steps, one hexadecimal digit each: the curve at index
/// i + 1 less the curve at i, from 0 at index 0 (compat-observed: LibRaw's
/// output on synthetic small RAWs, measured step by step).
const STEPS: &str = concat!(
    "0000000000000000000000000000000000000000000000000000000000001001",
    "0100101010010101010110101011011011011011101110111101111101111111",
    "1111011111111211111111111211111121111211121112111211211211211212",
    "1121212112121212121212122121212212122121221221221222122122212221",
    "2222122222122222122222221222222222222222212222222232222222222222",
    "2223222222223222222322222322223222232223222232232223222322322322",
    "3222322323223223223232232322323223232322323232323232232323232323",
    "2323323232323232332323232332323323233232332323323323233233233233",
    "2332332332332332333233233323323332333233323332333233323332333323",
    "3332333323333233333233332333333233333323333332333333323333333332",
    "3333333333233333333333333233333333333333333333333333333333333333",
    "3333333333333333333334333333333333334333333333343333333343333333",
    "4333333433333433333433333433334333343333433343334333433343334333",
    "4333433433343343343343343343343343343343433433434334343343434334",
    "3434334343434343434343434343434343434344343434434343443434434434",
    "3443443443443443443444344344434443444344443444434444344444434444",
    "4434444444444344444444444444444444444444444454444444444544444454",
    "4444544445444454445444544544454454454454544544545454454545454545",
    "4545454545455454545545545455455455545545554555455554555554555555",
    "5455555555555555555555555555555655555556555565555655565556556556",
    "5565565565655656565656565656565665656656566566656656665666566665",
    "6666665666666666666666666666666666676666667666676667666766766766",
    "7667676766767676767677676767767767767767776777677776777777677777",
    "7777777777777777778777777877778777877787787787787877878787878787",
    "8787887887878878887887888788888788888887888888888888888889888888",
    "8988889888898898889889898898898989898989898989989899899899899899",
    "989999899999899999999989999999999a9999999999a99999a999a999a999a9",
    "9a99a99a9a99a9a9a99a9a9a9a9aa9a9a9aa9a9aa9aa9aaa9aa9aaa9aaa9aaaa",
    "a9aaaaaa9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabaaaaaabaaaaabaaaabaaaba",
    "aabaaabaabaabaabaababaabaabababaabababababababababababababbababa",
    "bbababbababbabbabbabbabbabbabbabbbabbabbbabbbabbbabbbabbbabbbbab",
    "bbabbbbabbbbabbbbbabbbbabbbbbabbbbbabbbbbbabbbbbabbbbbabbbbbbabb",
    "bbbabbbbbabbbbbbabbbbabbbbbabbbbabbbbbabbbabbbbabbbbabbbabbbabbb",
    "abbbabbabbbabbabbabbabbabbabbabbababbababbabababbabababababababa",
    "babaabababaababaabaabaabaabaabaaabaaabaaaabaaaabaaaaaaabaaaaaaaa",
    "aaaaaaaaaaaaaaaa9aaaaaa9aaaaa9aaa9aaa9aa9aa9aa9a9aa9a9a9a9a9a9a9",
    "a99a9a99a99a99a999a999a99999a99999999999999999999999899999899989",
    "9899989899898989989889898988988988988898888988888888888888888888",
    "8887888878887887887887878787878787877877877877787777877777777777",
    "7777777767777767776776776767767676767667676676676667666667666666",
    "6666666666665666656665666566565656656565565655655655655565555565",
    "5555555555555554555554555455545545455454545454544545445445444544",
    "4454444444444543444444444443444434443443443434434343434343434334",
    "3343343334333433333433333333333333333333233333323332333233233233",
    "2323323232323232323223232232232223222322223222223222222222222222",
    "2222222222212222221222122221221222122121221221212122121212121211",
    "2121211212112112112112111211211112111121111121111111211111111111",
    "1111111111111111111110111111110111110111110111011110111011101101",
);

/// The tone curve from index 0 to 3072, and its top beyond.
fn curve() -> Vec<u16> {
    let mut curve = Vec::with_capacity(STEPS.len() + 1);
    let mut v = 0u16;
    curve.push(v);
    for c in STEPS.bytes() {
        v += (c as char).to_digit(16).unwrap_or(0) as u16;
        curve.push(v);
    }
    curve
}

/// Decodes `width × height` pixels (width even) into red, green, blue and a
/// zero fourth channel.
pub(crate) fn decode(
    data: &[u8],
    width: usize,
    height: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u16>> {
    if !width.is_multiple_of(2) {
        return Err(corrupt("small RAW of an odd width"));
    }
    let samples = crate::io::samples(width as u32, height as u32, 4)?;
    let curve = curve();
    let top = *curve.last().unwrap_or(&0);
    let tone = |v: f32| -> u16 {
        // Indices below 0 are 0; the curve stops at its top.
        let i = (v * 3072.).max(0.).floor();
        if i >= curve.len() as f32 {
            top
        } else {
            curve[i as usize]
        }
    };
    let mut out = vec![0u16; samples];
    let pairs = width / 2;
    for (y, row) in out.chunks_exact_mut(width * 4).enumerate() {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let src = data.get(y * pairs * 6..).unwrap_or(&[]);
        let pair = |k: usize| -> [u32; 4] {
            let mut b = [0u8; 8];
            if let Some(s) = src.get(k * 6..k * 6 + 6) {
                b[..6].copy_from_slice(s);
            }
            let x = u64::from_le_bytes(b);
            [0, 12, 24, 36].map(|s| (x >> s & 0xfff) as u32)
        };
        for k in 0..pairs {
            let [y0, y1, cb, cr] = pair(k);
            let [_, _, cb_next, cr_next] = pair((k + 1).min(pairs - 1));
            let pixels = [(y0, cb, cr), (y1, (cb + cb_next) / 2, (cr + cr_next) / 2)];
            for (i, (luma, cb, cr)) in pixels.into_iter().enumerate() {
                let rgb = rgb(luma, cb, cr, &tone);
                row[(2 * k + i) * 4..(2 * k + i) * 4 + 3].copy_from_slice(&rgb);
            }
        }
    }
    Ok(out)
}

/// One pixel's red, green and blue.
fn rgb(luma: u32, cb: u32, cr: u32, tone: &impl Fn(f32) -> u16) -> [u16; 3] {
    let y = luma as f32 / 2549.;
    if luma >= 2047 {
        let v = tone(y);
        return [v; 3];
    }
    let cb = (cb as f32 - 2048.) / 1536.;
    let cr = (cr as f32 - 2048.) / 1536.;
    [
        tone(1.402f32.mul_add(cr, y)),
        tone((-0.71414f32).mul_add(cr, (-0.34414f32).mul_add(cb, y))),
        tone(1.772f32.mul_add(cb, y)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(quads: &[[u32; 4]]) -> Vec<u8> {
        quads
            .iter()
            .flat_map(|q| {
                let x = u64::from(q[0])
                    | u64::from(q[1]) << 12
                    | u64::from(q[2]) << 24
                    | u64::from(q[3]) << 36;
                x.to_le_bytes()[..6].to_vec()
            })
            .collect()
    }

    #[test]
    fn decodes_as_measured() {
        // Values LibRaw reported for these pairs (synthetic files): neutral
        // luma 1000 is 3214; a highlight ignores its chroma; red, green and
        // blue apart; the second pixel's chroma is the mean with the next
        // pair's.
        let quads = [
            [1000, 1000, 2048, 2048],
            [2047, 2047, 2048 - 50, 2048 + 50],
            [560, 560, 2018, 2073],
            [448, 448, 2040, 2056],
        ];
        let out = decode(&pack(&quads), 8, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(out[..4], [3214, 3214, 3214, 0]);
        assert_eq!(out[8..11], [14186; 3]);
        assert_eq!(out[16..19], [1477, 1218, 958]);
        assert_eq!(out[24..27], [941, 865, 803]);
        assert_eq!(curve().len(), 3073);
        assert_eq!((curve()[60], curve()[61], curve()[3072]), (0, 1, 16287));
    }

    #[test]
    fn short_and_odd_inputs_do_not_panic() {
        for len in 0..30 {
            let data: Vec<u8> = (0..len).map(|i| (i * 97) as u8).collect();
            let _ = decode(&data, 4, 2, &AtomicBool::new(false));
        }
        assert!(decode(&[0; 12], 3, 1, &AtomicBool::new(false)).is_err());
    }
}
