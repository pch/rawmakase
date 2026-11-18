//! Merge to HDR ([docs/hdr-merge.md](../../../docs/hdr-merge.md)): a bracket
//! of RAWs merged into one scene-referred linear floating-point DNG that opens
//! as an ordinary raw, here and in Lightroom and Camera Raw.
//!
//! So far it holds the DNG writer ([`encode`]): a demosaiced linear raw image of
//! half-precision samples with the camera's colour, levels, EXIF and maker
//! note.
//!
//! It never depends on the catalog, export, the app, the GUI or inference
//! (`scripts/deps-crates.txt`).
#![forbid(unsafe_code)]

mod dng;

pub use dng::{Calibration, DngFacts, LinearImage, MAX_WHITE, MakerNote, TILE, encode};

/// Why a merge or its DNG failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The work was cancelled.
    Cancelled,
    /// The DNG would pass the 4 GiB that TIFF's 32-bit offsets reach.
    TooLarge,
    /// The input breaks the writer's contract.
    Invalid(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Merge to HDR was cancelled"),
            Self::TooLarge => f.write_str("The merged DNG would be larger than 4 GB"),
            Self::Invalid(why) => write!(f, "Invalid merged DNG: {why}"),
        }
    }
}

impl std::error::Error for Error {}
