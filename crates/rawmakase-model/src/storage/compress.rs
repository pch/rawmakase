//! Fast zlib compression helpers shared across persistence crates.

use flate2::{Compression, bufread::ZlibDecoder, write::ZlibEncoder};
use std::io::{self, Read, Write};

/// Compresses `input` with [`Compression::fast`] and a zlib wrapper.
pub fn zlib_compress_fast(input: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(input)?;
    encoder.finish()
}

/// Compresses `input` using uncompressed zlib stored blocks.
pub fn zlib_compress_stored(input: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::none());
    encoder.write_all(input)?;
    encoder.finish()
}

/// Conservative upper bound on the byte length of [`zlib_compress_stored`] for
/// `raw_bytes` bytes of input (zlib 2-byte header, 4-byte Adler-32 trailer, and
/// 5 bytes per stored DEFLATE block across streaming encoder flushes plus finish).
pub const fn zlib_max_stored_len(raw_bytes: u64) -> u64 {
    let blocks = raw_bytes.div_ceil(16_384).saturating_add(1);
    raw_bytes
        .saturating_add(blocks.saturating_mul(5))
        .saturating_add(6)
}

/// Decompresses `compressed` into `out`, rejecting size mismatches, corrupt
/// checksums and trailing bytes.
pub fn zlib_decompress_exact(compressed: &[u8], out: &mut [u8]) -> io::Result<()> {
    let mut decoder = ZlibDecoder::new(compressed);
    decoder.read_exact(out)?;
    // One extra read forces flate2 to consume the Adler-32 trailer and confirm EOF.
    let mut extra = [0u8; 1];
    if decoder.read(&mut extra)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Compressed stream exceeds expected size",
        ));
    }
    if decoder.total_in() != compressed.len() as u64 || !decoder.into_inner().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Trailing bytes after compressed stream",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_and_non_empty() -> io::Result<()> {
        for input in [&[][..], b"hello rawmakase compression".as_slice()] {
            let compressed = zlib_compress_fast(input)?;
            let mut restored = vec![0u8; input.len()];
            zlib_decompress_exact(&compressed, &mut restored)?;
            assert_eq!(restored, input);
        }
        Ok(())
    }

    #[test]
    fn rejects_truncation_corruption_and_size_mismatch() -> io::Result<()> {
        let input = vec![42u8; 1024];
        let compressed = zlib_compress_fast(&input)?;

        let mut truncated = vec![0u8; input.len()];
        assert!(
            zlib_decompress_exact(&compressed[..compressed.len() - 1], &mut truncated).is_err()
        );

        let mut flipped = compressed.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xff;
        assert!(zlib_decompress_exact(&flipped, &mut vec![0u8; input.len()]).is_err());

        let mut too_big = vec![0u8; input.len() + 1];
        assert!(zlib_decompress_exact(&compressed, &mut too_big).is_err());

        let mut too_small = vec![0u8; input.len() - 1];
        assert!(zlib_decompress_exact(&compressed, &mut too_small).is_err());

        let mut with_trailing = compressed;
        with_trailing.push(0);
        assert!(zlib_decompress_exact(&with_trailing, &mut vec![0u8; input.len()]).is_err());
        Ok(())
    }

    #[test]
    fn stored_fallback_respects_bound_and_roundtrips() -> io::Result<()> {
        let mut pseudo_random = vec![0u8; 131_072];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for chunk in pseudo_random.as_chunks_mut::<8>().0 {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes());
        }
        for input in [
            &[][..],
            b"short stored block".as_slice(),
            pseudo_random.as_slice(),
        ] {
            let stored = zlib_compress_stored(input)?;
            assert!(
                (stored.len() as u64) <= zlib_max_stored_len(input.len() as u64),
                "stored length {} exceeded bound {} for input length {}",
                stored.len(),
                zlib_max_stored_len(input.len() as u64),
                input.len()
            );
            let mut restored = vec![0u8; input.len()];
            zlib_decompress_exact(&stored, &mut restored)?;
            assert_eq!(restored, input);
        }
        Ok(())
    }
}
