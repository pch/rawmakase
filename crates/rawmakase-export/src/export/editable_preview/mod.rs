//! Writing editable previews (docs/editable-previews.md): a reduced copy of a
//! photo, [`LONG_EDGE`] pixels on its long edge, built while its original is
//! online so that Develop can edit the photo while the original is offline.
//! [`write()`] makes it from the photo's full-size decode, as an export is made;
//! [`rawmakase_decode::editable_preview::open`] opens it as the photo again.
//!
//! The file is a DNG linear raw of the image with highlights recovered, in
//! lossless JPEG tiles (`dng`, `ljpeg`), which RAWmakase's DNG reader opens
//! unchanged and another DNG reader can open too. The original's facts that
//! its standard tags carry only in part or only approximately are in
//! RAWmakase's own versioned record in DNGPrivateData (`record`).
//!
//! The copy holds no identity of the original it was built from: that is the
//! catalog's row's (`catalog::editable_previews`), and the name the catalog gives
//! the file ties the two.
mod dng;
mod ljpeg;
mod record;
#[cfg(test)]
mod tests;

use crate::camera_data::CameraImage;
use anyhow::{Context, Result, ensure};
use rawmakase_cancel::Cancel;
use rawmakase_model::camera_profiles::CameraProfile;
use std::path::Path;

/// The long edge editable previews are built at, as Lightroom Classic's Smart
/// Previews are.
pub const LONG_EDGE: u32 = 2560;

/// Writes the editable preview of `original`, a photo's full-size decode, to
/// `path`, with the photo's `default_look` when it has one (a fitted Camera
/// Look, for a camera without color data).
///
/// Highlights are recovered first unless they are already, as
/// `rawmakase_decode::decode` gives Develop the image, then it is reduced to
/// [`LONG_EDGE`] (never enlarged). Auto CA is measured on `original`, as every
/// render of the photo measures it, and kept. The file is written beside `path`
/// under a staging name and renamed into place only once complete, replacing a
/// file there.
pub fn write(
    path: &Path,
    original: &CameraImage,
    default_look: Option<&CameraProfile>,
    cancel: Cancel<'_>,
) -> Result<()> {
    write_at(path, original, default_look, LONG_EDGE, cancel)
}

/// [`write()`], reduced to `long_edge` pixels.
fn write_at(
    path: &Path,
    original: &CameraImage,
    default_look: Option<&CameraProfile>,
    long_edge: u32,
    cancel: Cancel<'_>,
) -> Result<()> {
    let bytes = build(original, default_look, long_edge, cancel)?;
    cancel.check("Editable preview cancelled")?;
    crate::storage::write_atomic(path, crate::storage::Replace::Overwrite, |file| {
        use std::io::Write as _;
        file.write_all(&bytes)?;
        Ok(())
    })
    .with_context(|| format!("Could not write the editable preview {}", path.display()))
}

/// The bytes of the editable preview [`write_at`] writes.
fn build(
    original: &CameraImage,
    default_look: Option<&CameraProfile>,
    long_edge: u32,
    cancel: Cancel<'_>,
) -> Result<Vec<u8>> {
    ensure!(
        !original.fast,
        "An editable preview is built from a full-size decode"
    );
    ensure!(
        original.width > 0
            && original.height > 0
            && original.pixels.len() == original.width as usize * original.height as usize,
        "An editable preview's original has no pixels"
    );
    ensure!(
        original.hdr().is_none(),
        "An editable preview can't keep an HDR file's light"
    );
    // Auto CA's measurement and the reduction below take no cancel: Auto CA's is
    // the measurement every render of the photo shares, and both are a fraction
    // of the decode before them. A cancel is honoured between stages and within
    // highlight recovery and the tiles' encoding.
    cancel.check("Editable preview cancelled")?;
    rawmakase_model::lens::auto_ca::prime(original);
    let recovered = crate::develop::quality::recovered(original, cancel)?;
    cancel.check("Editable preview cancelled")?;
    let reduced = crate::develop::preview(&recovered, long_edge);
    cancel.check("Editable preview cancelled")?;
    let record = record::of(
        &original.metadata,
        [reduced.width, reduced.height],
        default_look,
    )?;
    let private = record::to_bytes(&record)?;
    dng::file(&reduced, &original.metadata, &private, cancel)
}
