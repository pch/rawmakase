//! Files whose metadata records declare gigantic lengths: `open` reads only
//! what its readers use, so it allocates far less than the file's size
//! however many records point at the same megabytes.
//!
//! The allocations are counted by a global allocator that forwards to the
//! system's, so the whole check is one test: no other test thread
//! allocates while it measures.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

fn grew(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
    TOTAL.fetch_add(size, Relaxed);
}

// SAFETY: every call forwards to the system allocator unchanged; the
// counters only observe the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            grew(size);
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The bytes `open` allocates in all, and at most at once, on `file`.
fn allocations(file: &[u8]) -> (usize, usize) {
    // Once first: tables parsed on first use are not the file's doing.
    let _ = rawmakase_raw::open(file);
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    TOTAL.store(0, Relaxed);
    let result = rawmakase_raw::open(file);
    let (total, peak) = (TOTAL.load(Relaxed), PEAK.load(Relaxed) - base);
    drop(result);
    (total, peak)
}

/// The shared record: four megabytes, its first bytes `prefix`.
const BLOB: u32 = 4 << 20;

fn blob(prefix: &[u8]) -> Vec<u8> {
    let mut b = prefix.to_vec();
    b.resize(BLOB as usize, 0);
    b
}

const BYTE: u16 = 1;
const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const UNDEFINED: u16 = 7;

/// An entry's value: its bytes, a directory's offset, or a place in the
/// data after the directories (the TIFF's tail).
#[derive(Clone)]
enum Value {
    Bytes(Vec<u8>),
    Ifd(usize),
    Tail(u32),
}

/// Tag, type, count, value.
type Entry = (u16, u16, u32, Value);

fn long(tag: u16, v: u32) -> Entry {
    (tag, LONG, 1, Value::Bytes(v.to_le_bytes().to_vec()))
}

fn short(tag: u16, v: u16) -> Entry {
    (tag, SHORT, 1, Value::Bytes(v.to_le_bytes().to_vec()))
}

/// An entry whose `count` values of `kind` are the tail's from `at` on.
fn record(tag: u16, kind: u16, at: u32, count: u32) -> Entry {
    (tag, kind, count, Value::Tail(at))
}

/// A little-endian TIFF: directory 0 (the chain's only one), the others
/// standalone after it, their long values, then `tail`; and where the tail
/// starts.
fn tiff(dirs: &[Vec<Entry>], tail: &[u8]) -> (Vec<u8>, u32) {
    let mut at = 8u32;
    let dir_at: Vec<u32> = dirs
        .iter()
        .map(|d| {
            let here = at;
            at += 2 + 12 * d.len() as u32 + 4;
            here
        })
        .collect();
    let long_values: u32 = dirs
        .iter()
        .flatten()
        .filter_map(|e| match &e.3 {
            Value::Bytes(b) if b.len() > 4 => Some(b.len() as u32),
            _ => None,
        })
        .sum();
    let mut values_at = at;
    let tail_at = (at + long_values).next_multiple_of(4);
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    let mut values = Vec::new();
    for d in dirs {
        let mut d = d.clone();
        d.sort_by_key(|e| e.0);
        out.extend((d.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in d {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            match value {
                Value::Bytes(b) if b.len() <= 4 => {
                    out.extend(b.iter().copied().chain([0; 4]).take(4));
                }
                Value::Bytes(b) => {
                    out.extend(values_at.to_le_bytes());
                    values_at += b.len() as u32;
                    values.extend(b);
                }
                Value::Ifd(i) => out.extend(dir_at[i].to_le_bytes()),
                Value::Tail(off) => out.extend((tail_at + off).to_le_bytes()),
            }
        }
        out.extend(0u32.to_le_bytes());
    }
    out.extend(values);
    out.resize(tail_at as usize, 0);
    out.extend(tail);
    (out, tail_at)
}

/// The EXIF directory's exposure, lens and CFA records, all the blob.
fn exif() -> Vec<Entry> {
    vec![
        record(33434, BYTE, 0, BLOB),
        record(33437, BYTE, 0, BLOB),
        record(37377, BYTE, 0, BLOB),
        record(37378, BYTE, 0, BLOB),
        record(37386, BYTE, 0, BLOB),
        record(41730, UNDEFINED, 0, BLOB),
        record(42036, ASCII, 0, BLOB),
    ]
}

/// A 16 × 16 image of 16-bit zeros after the blob, in one strip.
const IMAGE: u32 = 16 * 16 * 2;

fn image(size: u32) -> Vec<Entry> {
    vec![
        long(254, 0),
        long(256, size),
        long(257, size),
        short(258, 16),
        short(259, 1),
        short(262, 32803),
        short(277, 1),
        record(273, LONG, BLOB, 1),
        long(278, size),
        long(279, IMAGE),
    ]
}

/// A TIFF-based raw: IFD0 `entries` and the image's, its Make and Model
/// the blob's `make` (then NULs), an EXIF directory of gigantic records.
fn tiff_raw(make: &[u8], entries: &[Entry]) -> Vec<u8> {
    let mut ifd0 = image(16);
    ifd0.extend([
        record(271, ASCII, 0, BLOB),
        record(272, ASCII, 0, BLOB),
        (34665, LONG, 1, Value::Ifd(1)),
    ]);
    for e in entries {
        ifd0.retain(|x| x.0 != e.0);
        ifd0.push(e.clone());
    }
    let mut tail = blob(make);
    tail.resize((BLOB + IMAGE) as usize, 0);
    tiff(&[ifd0, exif()], &tail).0
}

fn dng() -> Vec<u8> {
    // Past the Make's NULs, zeros: a red-only pattern of plane 0.
    let at = 64;
    let n = BLOB - at;
    let mut entries = vec![
        (50706, BYTE, 4, Value::Bytes(vec![1, 4, 0, 0])),
        record(33421, SHORT, at, n / 2),
        record(33422, BYTE, at, n),
        record(50708, ASCII, at, n),
        record(50710, BYTE, at, n),
        record(50712, SHORT, at, n / 2),
        record(50740, BYTE, at, n),
        record(51008, UNDEFINED, at, n),
        record(51009, UNDEFINED, at, n),
    ];
    for tag in [
        50713, 50714, 50715, 50716, 50719, 50720, 50721, 50722, 50727, 50728, 50829, 51125,
    ] {
        entries.push(record(tag, BYTE, at, n));
    }
    tiff_raw(b"Hostile\0", &entries)
}

fn nef() -> Vec<u8> {
    tiff_raw(b"NIKON\0", &[record(33422, BYTE, 64, BLOB - 64)])
}

fn arw() -> Vec<u8> {
    let mut entries = vec![record(33422, BYTE, 64, BLOB - 64)];
    for tag in [0x7010, 0x7310, 0x7313, 0x74c7, 0x74c8] {
        entries.push(record(tag, BYTE, 64, BLOB - 64));
    }
    tiff_raw(b"SONY\0", &entries)
}

fn orf() -> Vec<u8> {
    // Its strips' lengths, one per byte of the blob.
    tiff_raw(b"OLYMPUS\0", &[record(279, BYTE, 64, BLOB - 64)])
}

/// A RAF whose JPEG's maker note and raw directory's black levels and
/// illuminants are the blob.
fn raf() -> Vec<u8> {
    let raw_dir = vec![
        long(0xf001, 16),
        long(0xf002, 16),
        long(0xf003, 16),
        // RawOffset, from the raw block: the image's, after the blob.
        record(0xf007, LONG, BLOB, 1),
        long(0xf008, IMAGE),
        record(0xf00a, BYTE, 0, BLOB),
        record(0xf00c, BYTE, 0, BLOB),
    ];
    let mut tail = blob(b"");
    tail.resize((BLOB + IMAGE) as usize, 0);
    let (raw, tail_at) = tiff(&[vec![(0xf000, LONG, 1, Value::Ifd(1))], raw_dir], &tail);
    let records: Vec<u8> = {
        let mut d = 1u32.to_be_bytes().to_vec();
        d.extend(0x110u16.to_be_bytes());
        d.extend(4u16.to_be_bytes());
        d.extend([0, 0, 0, 0]);
        d
    };
    // The JPEG: SOI, APP1 "Exif" whose TIFF's EXIF maker note is the blob
    // in the raw block, EOI.
    let jpeg = |note_at: u32| {
        let exif = tiff(
            &[
                vec![(34665, LONG, 1, Value::Ifd(1))],
                vec![(
                    0x927c,
                    UNDEFINED,
                    BLOB,
                    Value::Bytes(note_at.to_le_bytes().to_vec()),
                )],
            ],
            &[],
        )
        .0;
        let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
        j.extend((8 + exif.len() as u16).to_be_bytes());
        j.extend(b"Exif\0\0");
        j.extend(exif);
        j.extend([0xff, 0xd9]);
        j
    };
    let jpeg_len = jpeg(0).len() as u32;
    let raw_at = 160 + jpeg_len + records.len() as u32;
    let exif_base = 160 + 2 + 2 + 2 + 6;
    let jpeg = jpeg(raw_at + tail_at - exif_base);

    let mut out = b"FUJIFILMCCD-RAW 0201FF000000Hostile".to_vec();
    out.resize(160, 0);
    let pointers = [
        160,
        jpeg_len,
        160 + jpeg_len,
        records.len() as u32,
        raw_at,
        raw.len() as u32,
    ];
    for (i, v) in pointers.into_iter().enumerate() {
        out[84 + 4 * i..88 + 4 * i].copy_from_slice(&v.to_be_bytes());
    }
    out.extend(jpeg);
    out.extend(records);
    out.extend(raw);
    out
}

#[test]
fn gigantic_records_cost_open_less_than_the_file() {
    for (name, file) in [
        ("DNG", dng()),
        ("NEF", nef()),
        ("ARW", arw()),
        ("ORF", orf()),
        ("RAF", raf()),
    ] {
        // Each opens: every reader of the records ran.
        assert!(rawmakase_raw::open(&file).is_ok(), "{name} does not open");
        let (total, peak) = allocations(&file);
        eprintln!("{name}: {total} bytes allocated, {peak} at most at once");
        // No reader copies a record it does not use: far less than one
        // copy of the file in all.
        assert!(
            total < file.len() / 4,
            "{name}: open allocated {total} bytes for a file of {}",
            file.len()
        );
        assert!(peak <= total);
    }
}
