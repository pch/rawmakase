//! Editable previews (docs/editable-previews.md): a reduced copy of a photo,
//! built while its original is online so that Develop can edit the photo while
//! the original is offline. This module opens a copy as the photo again
//! ([`open`]) and owns the format of RAWmakase's record in it ([`record`]).
//! Writing a copy is export's (`rawmakase_export::export::editable_preview`),
//! which keeps the encoder out of decoding.
//!
//! The file is a DNG linear raw of the image with highlights recovered,
//! which RAWmakase's DNG reader opens unchanged and another DNG reader can open
//! too. The original's facts that its standard tags carry only in part or only
//! approximately are in RAWmakase's own versioned record in DNGPrivateData.
//!
//! Opening a copy needs three things beyond the DNG reader, all in [`open`]:
//! the record's facts in place of the tags'; the image marked as a reduced
//! decode of a photo of the original's size ([`CameraImage::photo_scale`]); and
//! its highlights marked as recovered already
//! (`develop::quality::recovered`), since recovering a reduced copy again
//! would not give the original's.
//!
//! A copy that can't be opened as this release opens it (a record of a
//! version it doesn't know, no record, a file cut short or damaged) is an
//! error, never a panic; whoever opens it treats the copy as out of date.
pub mod record;
#[cfg(test)]
mod tests;

use crate::camera_data::{CameraImage, Decode, Demosaic};
use anyhow::{Context, Result, ensure};
use rawmakase_cancel::Cancel;
use rawmakase_model::camera_profiles::CameraProfile;
use record::Record;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

/// DNGPrivateData, the tag the record is kept in.
pub const PRIVATE_DATA: u16 = 50740;

/// An editable preview, opened: the image Develop edits the photo from.
pub struct EditablePreview {
    /// The copy's pixels, a reduced decode ([`CameraImage::fast`]) of a photo of
    /// the original's size, with the original's metadata and its highlights
    /// recovered already.
    pub image: CameraImage,
    /// The photo's default Camera Look (fitted), for a camera without colour
    /// data: its fit needs the original, so offline it comes only from here.
    pub default_look: Option<CameraProfile>,
}

/// The editable preview at `path`, opened as its photo: see [`EditablePreview`].
/// Its DNG is read by the DNG reader as any DNG is; then the record's facts
/// replace the tags', and the imported lens profiles that fit are matched, as
/// opening the original would match them.
pub fn open(path: &Path, cancel: Cancel<'_>) -> Result<EditablePreview> {
    let record = read_record(path)?;
    let mut metadata = record.metadata()?;
    let default_look = record.default_look(&metadata)?;
    let decoded = crate::photo::open(path)
        .and_then(|raw| raw.develop(Decode::Full(Demosaic::default()), cancel))
        .context("An editable preview's pixels can't be read")?;
    ensure!(
        (decoded.width, decoded.height) == (record.copy_width, record.copy_height),
        "An editable preview reads as {}×{} pixels, not the {}×{} it was written with",
        decoded.width,
        decoded.height,
        record.copy_width,
        record.copy_height
    );
    metadata.lens_profiles = crate::photo::lens_profiles(&metadata);
    let mut image = CameraImage {
        width: decoded.width,
        height: decoded.height,
        recovered: Default::default(),
        pixels: decoded.pixels,
        metadata,
        // A reduced decode of the photo, whose metadata keeps the photo's size.
        fast: true,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    // Its pixels are recovered already: a second recovery of a reduced copy
    // would not be the original's. Sharing the pixels lets the two be one buffer.
    image.pixels.share();
    let _ = image.recovered.set(Arc::new(image.clone()));
    Ok(EditablePreview {
        image,
        default_look,
    })
}

/// The record in the DNGPrivateData of the DNG at `path`, read on its own, at
/// most [`record::MAX_RECORD`] bytes of it.
fn read_record(path: &Path) -> Result<Record> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let entry = {
        let mut tiff = crate::tiff::Tiff::open(&mut file, 0)
            .context("Not an editable preview: not a TIFF file")?;
        // Little-endian, as export writes it: a big-endian file is not
        // RAWmakase's.
        ensure!(tiff.little, "Not an editable preview: a big-endian TIFF");
        let first = tiff.first;
        tiff.ifd(first)
            .context("An editable preview's directory can't be read")?
            .remove(&PRIVATE_DATA)
            .context("Not an editable preview: its DNG holds no RAWmakase record")?
    };
    ensure!(
        matches!(
            entry.kind,
            crate::tiff::kind::BYTE | crate::tiff::kind::UNDEFINED
        ),
        "An editable preview's record is damaged"
    );
    let len = u64::from(entry.count);
    ensure!(
        len <= record::MAX_RECORD,
        "An editable preview's record is larger than one may be"
    );
    let mut bytes = vec![0; len as usize];
    if len <= 4 {
        bytes.copy_from_slice(&entry.value[..len as usize]);
    } else {
        let at = u64::from(u32::from_le_bytes(entry.value));
        ensure!(
            at.checked_add(len).is_some_and(|end| end <= size),
            "An editable preview is cut short"
        );
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut bytes)?;
    }
    Record::parse(&bytes)
}
