//! An AI Remove patch's pixels as a stored raster: linear camera RGB, three
//! channels of little-endian half-precision floats, rows top to bottom
//! ([`crate::model::retouch::RetouchPatch`]). Half precision keeps about three
//! significant digits over the whole range camera values take, which an 8-bit
//! rendition's round trip never reaches.
use super::bitmaps::Bitmap;
use super::mask_assets::{Kind, validate_kind};
use anyhow::{Result, ensure};
use half::f16;

/// `samples` (`width * height * 3`, RGB interleaved) as a patch raster.
/// Samples beyond f16's range are clamped to it; one that isn't a finite
/// number is refused.
pub fn encode(width: u32, height: u32, samples: &[f32]) -> Result<Bitmap> {
    ensure!(
        samples.len() == width as usize * height as usize * 3,
        "A patch's samples don't fit its size"
    );
    ensure!(
        samples.iter().all(|v| v.is_finite()),
        "A patch holds a sample that isn't a finite number"
    );
    let data = samples
        .iter()
        .flat_map(|v| f16::from_f32(v.clamp(-65504., 65504.)).to_le_bytes())
        .collect();
    let bitmap = Bitmap {
        width,
        height,
        channels: 3,
        depth: 2,
        data,
    };
    validate_kind(&bitmap, Kind::Patch)?;
    Ok(bitmap)
}

/// A patch raster's pixels, as RGB.
pub fn decode(bitmap: &Bitmap) -> Result<Vec<[f32; 3]>> {
    validate_kind(bitmap, Kind::Patch)?;
    let sample = |s: &[u8]| f16::from_le_bytes([s[0], s[1]]).to_f32();
    Ok(bitmap
        .data
        .chunks_exact(6)
        .map(|p| [sample(&p[0..2]), sample(&p[2..4]), sample(&p[4..6])])
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_patch_round_trips_at_half_precision() {
        let samples = [0.0, 0.18, 1.0, 4.5, 1e-4, 0.5, 100.0, 70000.0, -0.01];
        let bitmap = super::encode(3, 1, &samples).unwrap();
        assert_eq!(
            (bitmap.channels, bitmap.depth, bitmap.data.len()),
            (3, 2, 18)
        );
        let back = super::decode(&bitmap).unwrap();
        for (a, b) in samples.iter().zip(back.iter().flatten()) {
            let expected = a.clamp(-65504., 65504.);
            assert!(
                (expected - b).abs() <= expected.abs() * 1e-3 + 1e-6,
                "{a} {b}"
            );
        }
        assert!(super::encode(1, 1, &[0.0, f32::NAN, 0.0]).is_err());
        assert!(super::encode(2, 1, &[0.0; 3]).is_err());
    }
}
