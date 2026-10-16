//! Bounded reading of a WebP file's RIFF chunks: their kinds and where their
//! bodies are, and a chunk's body on request.
use std::io::{Read, Seek, SeekFrom};

/// Largest chunk body read into memory.
const MAX_BODY: u64 = 16 << 20;
/// Most chunks listed.
const MAX_CHUNKS: usize = 4096;

/// A chunk: its kind and its body's offset and length in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub kind: [u8; 4],
    pub start: u64,
    pub size: u64,
}

/// The chunks of a WebP file, in order; `None` when it isn't one.
pub fn webp_chunks<R: Read + Seek>(r: &mut R) -> Option<Vec<Chunk>> {
    let mut head = [0u8; 12];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WEBP" {
        return None;
    }
    let end = 8 + u32::from_le_bytes(head[4..8].try_into().ok()?) as u64;
    let mut chunks = Vec::new();
    let mut at = 12u64;
    while at + 8 <= end && chunks.len() < MAX_CHUNKS {
        r.seek(SeekFrom::Start(at)).ok()?;
        let mut header = [0u8; 8];
        if r.read_exact(&mut header).is_err() {
            break;
        }
        let size = u32::from_le_bytes(header[4..8].try_into().ok()?) as u64;
        if at + 8 + size > end {
            break;
        }
        chunks.push(Chunk {
            kind: header[0..4].try_into().ok()?,
            start: at + 8,
            size,
        });
        // Bodies are padded to an even length.
        at += 8 + size + (size & 1);
    }
    Some(chunks)
}

/// A chunk's body, when not larger than 16 MiB.
pub fn chunk_data<R: Read + Seek>(r: &mut R, c: &Chunk) -> Option<Vec<u8>> {
    if c.size > MAX_BODY {
        return None;
    }
    r.seek(SeekFrom::Start(c.start)).ok()?;
    let mut body = vec![0u8; c.size as usize];
    r.read_exact(&mut body).ok()?;
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> std::fs::File {
        std::fs::File::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/data/formats")
                .join(name),
        )
        .unwrap()
    }
    #[test]
    fn a_webps_chunks_include_its_exif_and_xmp() {
        let mut f = fixture("metadata.webp");
        let chunks = webp_chunks(&mut f).unwrap();
        let kinds: Vec<_> = chunks.iter().map(|c| c.kind).collect();
        assert_eq!(kinds[0], *b"VP8X");
        assert!(
            kinds.contains(b"EXIF") && kinds.contains(b"XMP "),
            "{kinds:?}"
        );
        let xmp = chunks.iter().find(|c| &c.kind == b"XMP ").unwrap();
        assert!(
            String::from_utf8_lossy(&chunk_data(&mut f, xmp).unwrap()).contains("Fixture title")
        );
    }
    #[test]
    fn a_file_that_is_not_a_webp_has_no_chunks() {
        let mut r = std::io::Cursor::new(b"RIFF\x04\0\0\0WAVE".to_vec());
        assert!(webp_chunks(&mut r).is_none());
    }
}
