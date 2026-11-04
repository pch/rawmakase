//! One bit stream of prefix codes decoded on several threads, by
//! speculation. Many codings (Nikon's, Pentax's, lossless JPEG without
//! restart intervals) are a single stream, each symbol starting where the
//! last ended, so only one thread can know where a symbol starts. Prefix
//! codes resynchronise, though: a decoder started at an arbitrary bit soon
//! reaches a bit where a symbol of the true stream starts, and from there
//! decodes what a decoder started at the beginning decodes.
//!
//! The stream is cut into chunks, each decoded from its first bit into a
//! buffer of its own, on all threads, keeping where its first symbols start.
//! Then, in order, the true stream is followed from where the chunk before
//! ended, a symbol at a time, until it starts a symbol where the chunk's
//! decoding did with the same table; the chunk's symbols from there are the
//! stream's. A coding that cycles through tables (one per component) has a
//! chunk decoded once for each table it might start with; decodings with
//! other tables than the first stop where they meet the first's (or, if
//! they don't soon, decode the whole chunk).
//!
//! Symbols must decode from the bits and the table alone, not from earlier
//! values: prediction comes after. Each symbol placed is one the true stream
//! decodes from the same bit with the same table, so the values are the
//! sequential decoder's. Where that can't be shown (no meeting point within
//! the symbols kept, an invalid code on the true stream) [`decode`] gives
//! `None`, and the caller decodes the stream in order, which also reports
//! its errors.
use super::bits::Msb;
use crate::error::{Error, Result};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// Symbol starts kept per decoding to meet the true stream at (true streams
/// meet a chunk's within a few dozen symbols).
const SYNC: usize = 4096;
/// Every this many symbols a decoding keeps where one starts, so that the
/// end of the stream inside a chunk is found by decoding at most as many.
const CHECKPOINT: usize = 1024;
/// The smallest chunk worth a thread.
const MIN_CHUNK: usize = 256 << 10;

/// A stream of prefix codes, decodable from any bit.
pub(crate) trait Stream: Sync {
    fn data(&self) -> &[u8];
    /// Symbol `j` is coded with table `j % cycle()`.
    fn cycle(&self) -> usize {
        1
    }
    /// The value of the symbol at `bits`, coded with `table`; `None` for an
    /// invalid code, after which `bits` has still moved on.
    fn next(&self, bits: &mut Msb, table: usize) -> Option<u16>;
}

/// A pump that starts at any bit of the data and knows where it is.
struct Pump<'a> {
    bits: Msb<'a>,
    base: u64,
}

impl<'a> Pump<'a> {
    fn at(data: &'a [u8], bit: u64) -> Self {
        let byte = usize::try_from(bit / 8)
            .unwrap_or(usize::MAX)
            .min(data.len());
        let mut bits = Msb::new(&data[byte..]);
        let mut skip = bit - byte as u64 * 8;
        while skip > 0 {
            let n = skip.min(32) as u32;
            bits.skip(n);
            skip -= u64::from(n);
        }
        Self {
            bits,
            base: byte as u64 * 8,
        }
    }
    fn position(&self) -> u64 {
        self.base + self.bits.consumed()
    }
}

/// One chunk decoded from its first bit, its first symbol coded with
/// table `phase`.
struct Spec {
    phase: usize,
    /// Where symbol `j` starts, for the first [`SYNC`] symbols.
    starts: Vec<u64>,
    /// Where symbol `j · CHECKPOINT` starts.
    checkpoints: Vec<u64>,
    /// Symbol `j` of this decoding is symbol `j0` of the chunk's decoding
    /// with the first table, where this one stopped: `(j, j0)`.
    merge: Option<(usize, usize)>,
    values: Vec<u16>,
    /// The symbols whose code was invalid.
    invalid: Vec<usize>,
    /// Where the symbol after the last starts.
    end: u64,
    /// Whether a symbol took no bits, after which the decoding repeats
    /// itself and its values could fill all of `out`.
    stalled: bool,
}

/// Decodes `out.len()` symbols of `s` on all threads into `out`, and gives
/// where the symbol after the last starts; `None` when the stream is too
/// short to share or the decoding can't be shown to be the sequential one
/// (`out` is then to be decoded in order).
pub(crate) fn decode<S: Stream>(s: &S, out: &mut [u16], cancel: Cancel<'_>) -> Result<Option<u64>> {
    let len = s.data().len();
    if !shares(len) {
        return Ok(None);
    }
    let chunk = len.div_ceil(rayon::current_num_threads() * 4);
    decode_in_chunks(s, out, chunk.max(MIN_CHUNK), cancel)
}

/// Whether a stream of `len` bytes is decoded on several threads.
pub(crate) fn shares(len: usize) -> bool {
    rayon::current_num_threads() > 1 && len >= 2 * MIN_CHUNK
}

fn decode_in_chunks<S: Stream>(
    s: &S,
    out: &mut [u16],
    chunk: usize,
    cancel: Cancel<'_>,
) -> Result<Option<u64>> {
    let total = s.data().len() as u64 * 8;
    let chunk = chunk.max(1) as u64 * 8;
    let chunks = total.div_ceil(chunk).max(1) as usize;
    let cycle = s.cycle().max(1);
    let n = out.len();
    let specs = (0..chunks)
        .into_par_iter()
        .map(|k| {
            let start = k as u64 * chunk;
            let stop = (start + chunk).min(total);
            let first = speculate(s, start, stop, 0, n, None, cancel)?;
            let mut all = Vec::with_capacity(cycle);
            // The first chunk starts with the first table.
            let others = if k == 0 { 1..1 } else { 1..cycle };
            for phase in others {
                let spec = speculate(s, start, stop, phase, n, Some(&first.starts), cancel)?;
                all.push(spec);
            }
            all.insert(0, first);
            Ok(all)
        })
        .collect::<Result<Vec<Vec<Spec>>>>()?;
    if specs.iter().flatten().any(|spec| spec.stalled) {
        return Ok(None);
    }
    Ok(resolve(s, &specs, chunk, out))
}

/// Decodes from `start` until a symbol ends at or past `stop` (or `cap`
/// symbols, or one takes no bits), the first coded with table `phase`. With `main`, the starts of
/// the chunk's decoding with the first table, it stops where it meets them
/// within [`SYNC`] symbols.
fn speculate<S: Stream>(
    s: &S,
    start: u64,
    stop: u64,
    phase: usize,
    cap: usize,
    mut main: Option<&[u64]>,
    cancel: Cancel<'_>,
) -> Result<Spec> {
    let cycle = s.cycle().max(1);
    let mut pump = Pump::at(s.data(), start);
    let mut spec = Spec {
        phase,
        starts: Vec::with_capacity(SYNC),
        checkpoints: Vec::new(),
        merge: None,
        values: Vec::new(),
        invalid: Vec::new(),
        end: start,
        stalled: false,
    };
    spec.values.reserve(if main.is_none() {
        ((stop - start) / 4) as usize
    } else {
        SYNC
    });
    let mut pos = start;
    let mut table = phase % cycle;
    let mut j = 0usize;
    while pos < stop && j < cap {
        if let Some(first) = main {
            if let Ok(j0) = first.binary_search(&pos)
                && j0 % cycle == table
            {
                spec.starts.push(pos);
                spec.merge = Some((j, j0));
                break;
            }
            if j == SYNC {
                main = None;
            }
        }
        if j < SYNC {
            spec.starts.push(pos);
        }
        if j.is_multiple_of(CHECKPOINT) {
            spec.checkpoints.push(pos);
            if j.is_multiple_of(64 * CHECKPOINT) && cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
        }
        let v = s.next(&mut pump.bits, table).unwrap_or_else(|| {
            spec.invalid.push(j);
            0
        });
        spec.values.push(v);
        let at = pump.position();
        if at == pos {
            spec.stalled = true;
            break;
        }
        pos = at;
        table = if table + 1 == cycle { 0 } else { table + 1 };
        j += 1;
    }
    spec.end = pos;
    Ok(spec)
}

/// Follows the true stream through the chunks' decodings, filling `out`.
fn resolve<S: Stream>(s: &S, specs: &[Vec<Spec>], chunk: u64, out: &mut [u16]) -> Option<u64> {
    let cycle = s.cycle().max(1);
    let total = s.data().len() as u64 * 8;
    let n = out.len();
    let mut done = 0usize;
    let mut pos = 0u64;
    // The true stream decoded a symbol at a time, between meeting points.
    let mut walk: Option<Pump> = None;
    let mut walked = 0usize;
    while done < n {
        let k = ((pos / chunk) as usize).min(specs.len() - 1);
        let table = done % cycle;
        let hit = (pos < total)
            .then(|| {
                specs[k].iter().find_map(|spec| {
                    let j = spec.starts.binary_search(&pos).ok()?;
                    ((spec.phase + j) % cycle == table).then_some((spec, j))
                })
            })
            .flatten();
        if let Some((spec, j)) = hit {
            let until = spec.merge.map_or(spec.values.len(), |(jm, _)| jm);
            let count = (until - j).min(n - done);
            if spec.invalid.iter().any(|&i| (j..j + count).contains(&i)) {
                return None;
            }
            out[done..done + count].copy_from_slice(&spec.values[j..j + count]);
            done += count;
            pos = match spec.merge {
                Some((jm, _)) if j + count == jm => spec.starts[jm],
                _ => start_of(s, spec, j + count),
            };
            walk = None;
            walked = 0;
            continue;
        }
        // Past the data the stream reads zeros to its end, in order; inside
        // it, the true stream meets a chunk's decoding soon or never.
        if pos < total && walked > 2 * SYNC {
            return None;
        }
        let pump = walk.get_or_insert_with(|| Pump::at(s.data(), pos));
        out[done] = s.next(&mut pump.bits, table)?;
        pos = pump.position();
        done += 1;
        walked += 1;
    }
    Some(pos)
}

/// Where symbol `j` of `spec`'s decoding starts.
fn start_of<S: Stream>(s: &S, spec: &Spec, j: usize) -> u64 {
    if j == spec.values.len() {
        return spec.end;
    }
    if let Some(&p) = spec.starts.get(j) {
        return p;
    }
    let cycle = s.cycle().max(1);
    let c = j / CHECKPOINT;
    let mut pump = Pump::at(s.data(), spec.checkpoints[c]);
    let mut table = (spec.phase + c * CHECKPOINT) % cycle;
    for _ in c * CHECKPOINT..j {
        let _ = s.next(&mut pump.bits, table);
        table = if table + 1 == cycle { 0 } else { table + 1 };
    }
    pump.position()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::huffman::Huffman;
    use std::sync::atomic::AtomicBool;

    /// Two JPEG Huffman tables used in turn, each symbol a difference
    /// length followed by that many bits.
    struct Test<'a> {
        data: &'a [u8],
        tables: [Huffman; 2],
        cycle: usize,
    }

    impl Stream for Test<'_> {
        fn data(&self) -> &[u8] {
            self.data
        }
        fn cycle(&self) -> usize {
            self.cycle
        }
        fn next(&self, bits: &mut Msb, table: usize) -> Option<u16> {
            match self.tables[table].difference(bits, false) {
                Ok(d) => Some(d as u16),
                Err(_) => {
                    bits.skip(1);
                    None
                }
            }
        }
    }

    /// With `complete`, every bit string starts with a code of each table;
    /// without, the second table has no code 1111.
    fn tables(complete: bool) -> [Huffman; 2] {
        // Complete: codes of 2 bits (0, 1, 2), 3 (3), 4 (4) and 5 (5, 9).
        let mut a = [0u8; 16];
        a[1] = 3;
        a[2] = 1;
        a[3] = 1;
        a[4] = 2;
        let a = Huffman::new(&a, &[0, 1, 2, 3, 4, 5, 9]).unwrap();
        let mut b = [0u8; 16];
        b[0] = 1;
        b[1] = 1;
        b[2] = 1 + u8::from(complete);
        let b = Huffman::new(&b, &[4, 2, 7, 3]).unwrap();
        [a, b]
    }

    fn sequential(t: &Test, n: usize) -> Option<(Vec<u16>, u64)> {
        let mut pump = Pump::at(t.data, 0);
        let out = (0..n)
            .map(|j| t.next(&mut pump.bits, j % t.cycle))
            .collect::<Option<Vec<_>>>()?;
        Some((out, pump.position()))
    }

    /// A xorshift generator, for streams the same on every run.
    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn decodes_what_the_sequential_decoder_does_or_declines() {
        for complete in [true, false] {
            let mut shared = 0;
            for seed in 1..40u64 {
                let data = noise(seed, 20_000 + seed as usize * 997);
                for cycle in [1, 2] {
                    let t = Test {
                        data: &data,
                        tables: tables(complete),
                        cycle,
                    };
                    // Fewer symbols than the data holds, about as many, and
                    // more (the stream read on in zeros).
                    for n in [10_000, 30_000, 60_000] {
                        let mut out = vec![0; n];
                        let got = decode_in_chunks(&t, &mut out, 1500, Cancel::NEVER).unwrap();
                        let want = sequential(&t, n);
                        match (got, &want) {
                            (Some(end), Some((values, at))) => {
                                assert!(
                                    out == *values && end == *at,
                                    "seed {seed} cycle {cycle} n {n}"
                                );
                                shared += 1;
                            }
                            (Some(_), None) => panic!("decoded past an invalid code: seed {seed}"),
                            (None, _) => {}
                        }
                    }
                }
            }
            // Streams of complete tables all decode in parallel; with the
            // incomplete table, those of one table still do.
            let expected = if complete { 39 * 3 * 2 } else { 39 * 3 };
            assert!(shared >= expected, "complete {complete}: {shared}");
        }
    }

    /// Symbols of no bits (values a table leaves uncovered) repeat
    /// forever: the decoding declines rather than keep a chunk's worth of
    /// `out` for each chunk.
    #[test]
    fn symbols_taking_no_bits_decline() {
        struct Empty<'a>(&'a [u8]);
        impl Stream for Empty<'_> {
            fn data(&self) -> &[u8] {
                self.0
            }
            fn next(&self, bits: &mut Msb, _: usize) -> Option<u16> {
                if bits.peek(1) == 1 {
                    return Some(0);
                }
                bits.skip(1);
                Some(1)
            }
        }
        let data = [0xffu8; 50_000];
        let mut out = vec![0; 1_000_000];
        let got = decode_in_chunks(&Empty(&data), &mut out, 4096, Cancel::NEVER);
        assert_eq!(got, Ok(None));
    }

    #[test]
    fn cancelling_stops_the_decoding() {
        let data = noise(7, 50_000);
        let t = Test {
            data: &data,
            tables: tables(true),
            cycle: 1,
        };
        let mut out = vec![0; 100_000];
        assert_eq!(
            decode_in_chunks(&t, &mut out, 4096, Cancel::new(&AtomicBool::new(true))),
            Err(Error::Cancelled)
        );
    }
}
