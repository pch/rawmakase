//! Editable previews: a reduced copy of a photo, about 2560 pixels on the long edge,
//! built while its original is online so that Develop can edit the photo while the
//! original is offline. An editable preview is not a cache: once the original is
//! offline it cannot be rebuilt, so **every format version ever written stays
//! readable**. This module owns the file format; which photos have one and where
//! the files live is the catalog's (one file per master photo, named
//! `<photo id>-<identity key>.rmep`, the key being the original's
//! [`Identity::key`]).
//!
//! # Layout, version 1
//!
//! Little-endian throughout.
//!
//! | Bytes | Content |
//! | --- | --- |
//! | 8 | Magic, `RMKEDPV\0` |
//! | 4 | Format version, `u32`: 1 |
//! | … | Sections, to the end of the file |
//!
//! A section is a 4-byte ASCII tag, a `u64` payload length, the payload and the
//! payload's 64-bit FNV-1a hash ([`crate::storage::fnv1a`]). Version 1 writes
//! `HEAD`, `FACT`, `EXIF`, `LOOK` when there is one, and `PIXL`, in that order,
//! each once; a reader refuses a file without `HEAD`, `FACT`, `EXIF` and `PIXL`,
//! and skips a section it doesn't know (after checking its hash), so a later
//! release can add one without a new version, as `LOOK` was. Nothing follows the
//! last section.
//!
//! - `HEAD`, a record (`fields`: `u16` id, `u32` length, value): the codec (1), the
//!   pixel curve (2), the preview's width and height (3), the original's width and
//!   height (4), and the original's [`Identity`]: its size in bytes (5), its
//!   modification time in nanoseconds, `u128` (6), and its prefix hash (7). All are
//!   required.
//! - `FACT`, a record: the original's [`Metadata`] field by field, with defaults
//!   for fields missing (`facts`).
//! - `EXIF`, a record: the camera's EXIF ([`photo::camera_exif`](crate::photo::camera_exif)),
//!   kept so that exporting from an editable preview can be added later; empty when the
//!   original has none.
//! - `LOOK`, optional, a record: the photo's default Camera Look (fitted), for a
//!   photo without colour data of its own, whose default look is fitted to the
//!   original's embedded JPEG and so can't be fitted while the original is
//!   offline. Field 1 is the fitted profile as an XMP sidecar carries it
//!   (`rawmakase:FittedProfile`, [`fitted::carried`]: versioned, about 100 KB),
//!   checked when read as a sidecar's is ([`fitted::from_carried`]). Files
//!   written before it have none; releases before it skip it.
//! - `PIXL`: the pixels in the codec's encoding: camera-space values, highlights
//!   recovered, on curve 1 (`curve`), which keeps 12 stops above white, as a
//!   lossless JPEG XL codestream of three 16-bit channels (`codec`).
//!
//!   | Codec | Samples | Precision |
//!   | --- | --- | --- |
//!   | 1 | Curve 1's 16-bit codes | Lossless: within 0.013% of each value |
//!   | 2 | Curve 1's 12-bit codes, samples 0 to 4095 in the same 16-bit codestream (4096 and above are damage); code `q` reads as 16-bit code `16 q + 8`, the middle of the 16 codes it stands for; dithered when built | [`Precision::Smaller`]: a step of 0.41%, about 40% smaller files |
//!
//!   Codec 2 changes only the samples: a reader that knows it widens them
//!   (`curve::widen`) and develops them as codec 1's. The dither is the writer's
//!   (`curve::dithered`) and nothing a reader needs.
//!
//! A file of a later version, or naming a codec, curve or fact code a later
//! release introduced, or holding a Camera Look this release can't read, is
//! refused with an error that says so. Structs are never
//! serialised whole: they change between releases, and the file must not.
mod codec;
mod curve;
#[cfg(test)]
mod depths;
mod facts;
mod fields;
#[cfg(test)]
mod measure;
#[cfg(test)]
mod pace;
#[cfg(test)]
mod tests;

use crate::storage::Identity;
use anyhow::{Context, Result, bail, ensure};
use fields::{Record, Writer};
use rawmakase_cancel::Cancel;
use rawmakase_interop::exif::CameraExif;
use rawmakase_model::camera_data::{CameraImage, Metadata};
use rawmakase_model::camera_profiles::{CameraProfile, fitted};
use rayon::prelude::*;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"RMKEDPV\0";
/// The format version this release writes.
pub const VERSION: u32 = 1;
/// The long edge Develop's editable previews are built at, as Lightroom Classic's.
pub const LONG_EDGE: u32 = 2560;

const HEAD: [u8; 4] = *b"HEAD";
const FACT: [u8; 4] = *b"FACT";
const EXIF: [u8; 4] = *b"EXIF";
const PIXL: [u8; 4] = *b"PIXL";
const LOOK: [u8; 4] = *b"LOOK";

// HEAD record ids.
const CODEC: u16 = 1;
const CURVE: u16 = 2;
const PREVIEW_SIZE: u16 = 3;
const ORIGINAL_SIZE: u16 = 4;
const IDENTITY: [u16; 3] = [5, 6, 7];
// LOOK record ids.
const LOOK_PROFILE: u16 = 1;

/// The log curve with headroom of `curve`.
const CURVE_LOG: u8 = 1;
/// The largest preview written or read, per side and in pixels: well beyond the
/// long edge built, yet small enough that a damaged header can't ask for a huge
/// allocation. The writer and the reader share them ([`check_sizes`]), so nothing
/// is written that can't be read. A later release may raise them, never lower them.
const MAX_SIDE: u32 = 1 << 14;
const MAX_PIXELS: u64 = 1 << 26;
/// The largest original recorded, per side and in pixels.
const MAX_ORIGINAL_SIDE: u32 = 1 << 18;
const MAX_ORIGINAL_PIXELS: u64 = 1 << 34;
/// The largest `HEAD`, `FACT` or `EXIF` section read.
const MAX_RECORD: u64 = 64 << 20;

/// How finely an editable preview keeps its pixels: the choice [`build`] takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Precision {
    /// Every 16-bit code, losslessly ([`Codec::LosslessJxl16`]).
    #[default]
    Lossless,
    /// 12-bit codes, dithered ([`Codec::DitheredJxl12`]): about 40% smaller files.
    Smaller,
}

/// How an editable preview's pixels are coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Lossless JPEG XL, three 16-bit channels on the log curve (code 1).
    LosslessJxl16,
    /// The log curve's 12-bit codes, dithered when built, kept losslessly as JPEG
    /// XL samples 0 to 4095 in three 16-bit channels (code 2).
    DitheredJxl12,
}
impl Codec {
    /// The choice this codec keeps the pixels with.
    pub fn precision(self) -> Precision {
        match self {
            Codec::LosslessJxl16 => Precision::Lossless,
            Codec::DitheredJxl12 => Precision::Smaller,
        }
    }
    fn of_precision(precision: Precision) -> Self {
        match precision {
            Precision::Lossless => Codec::LosslessJxl16,
            Precision::Smaller => Codec::DitheredJxl12,
        }
    }
    fn code(self) -> u8 {
        match self {
            Codec::LosslessJxl16 => 1,
            Codec::DitheredJxl12 => 2,
        }
    }
    fn of(code: u8) -> Result<Self> {
        match code {
            1 => Ok(Codec::LosslessJxl16),
            2 => Ok(Codec::DitheredJxl12),
            _ => bail!(
                "This editable preview's pixels are coded in a way (codec {code}) only a later release of RAWmakase reads"
            ),
        }
    }
}

/// What the catalog needs from an editable preview file, read without decoding its
/// pixels ([`read_header`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub codec: Codec,
    /// The original's identity when the editable preview was built.
    pub identity: Identity,
    /// The original's full decoded size, width and height.
    pub original: [u32; 2],
    /// The editable preview's own size, width and height.
    pub preview: [u32; 2],
}

/// An editable preview: the reduced camera image, standing for its original.
pub struct EditablePreview {
    /// The format version of the file it was read from; [`VERSION`] for one built.
    version: u32,
    codec: Codec,
    identity: Identity,
    original: [u32; 2],
    exif: Option<CameraExif>,
    /// The photo's default Camera Look (fitted), when it has one.
    camera_look: Option<CameraProfile>,
    /// Reduced (`fast`), on the curve's codes, with its recovered image set to
    /// itself; its metadata is the original's.
    image: CameraImage,
}

impl EditablePreview {
    /// The camera image to develop: `fast`, a reduced decode, its pixels already
    /// recovered, and its metadata the original's (its width and height the
    /// original's full size). The imported lens profiles that fit are not part of
    /// it: whoever opens the photo matches them, as `photo::open` does.
    pub fn image(&self) -> &CameraImage {
        &self.image
    }
    pub fn into_image(self) -> CameraImage {
        self.image
    }
    /// The original's identity when this was built.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    /// The camera's EXIF of the original, if it had one.
    pub fn exif(&self) -> Option<&CameraExif> {
        self.exif.as_ref()
    }
    /// The photo's default Camera Look (fitted), stored when it was built from an
    /// original without colour data of its own. Its fit needs the original's
    /// embedded JPEG, so offline it can come only from here.
    pub fn camera_look(&self) -> Option<&CameraProfile> {
        self.camera_look.as_ref()
    }
    /// Keeps `profile` as the photo's default Camera Look (fitted), to be written
    /// with it. Refused, keeping none, unless it reads back as [`read`] reads it:
    /// a fitted look for the photo's camera, whole.
    pub fn set_camera_look(&mut self, profile: &CameraProfile) -> Result<()> {
        self.camera_look = None;
        let back = fitted::from_carried(&fitted::carried(profile), &self.image.metadata)?;
        self.camera_look = Some(back);
        Ok(())
    }
    pub fn header(&self) -> Header {
        Header {
            version: self.version,
            codec: self.codec,
            identity: self.identity.clone(),
            original: self.original,
            preview: [self.image.width, self.image.height],
        }
    }
}

/// The editable preview of `original`, the full-size decode of the file `identity`
/// names, at `long_edge` pixels on its long edge (never enlarged), with the camera's
/// `exif`, its pixels kept at `precision`.
///
/// Highlights are recovered first, unless they already are, and the image is
/// reduced by averaging the area each pixel covers. Auto CA is measured on
/// `original` (not on the reduced copy, as every render measures it) and kept. An
/// HDR file's light is not kept in version 1, so such a photo is refused, as is a
/// reduced decode.
pub fn build(
    original: &CameraImage,
    identity: &Identity,
    exif: Option<CameraExif>,
    long_edge: u32,
    precision: Precision,
    cancel: Cancel<'_>,
) -> Result<EditablePreview> {
    ensure!(
        original.hdr().is_none() && original.metadata.hdr_source.is_none(),
        "An editable preview can't keep an HDR file's light yet"
    );
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
    ensure!(long_edge >= 2, "An editable preview needs a size");
    rawmakase_model::lens::auto_ca::prime(original);
    let recovered = crate::develop::quality::recovered(original, cancel)?;
    ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
    let reduced = crate::develop::preview(&recovered, long_edge);
    ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
    // The JPEG XL encoder codes 2 × 2 pixels and more.
    ensure!(
        reduced.width >= 2 && reduced.height >= 2,
        "A photo this narrow can't have an editable preview"
    );
    let codec = Codec::of_precision(precision);
    let codes = match codec {
        Codec::LosslessJxl16 => encode_pixels(&reduced.pixels),
        Codec::DitheredJxl12 => reduced
            .pixels
            .par_iter()
            .enumerate()
            .map(|(i, p)| std::array::from_fn(|c| curve::widen(curve::dithered(p[c], i, c))))
            .collect(),
    };
    let lateral_ca = Arc::new(std::sync::OnceLock::new());
    if let Some(measured) = original.metadata.lateral_ca.get() {
        let _ = lateral_ca.set(measured.clone());
    }
    let metadata = Metadata {
        lens_profiles: Default::default(),
        lateral_ca,
        ..original.metadata.clone()
    };
    check_sizes(
        [reduced.width, reduced.height],
        [original.width, original.height],
    )?;
    Ok(EditablePreview {
        version: VERSION,
        codec,
        identity: identity.clone(),
        original: [original.width, original.height],
        exif,
        camera_look: None,
        image: image_of(
            reduced.width,
            reduced.height,
            &codes,
            metadata,
            facts::Scale {
                factor: original.scale_factor,
                clipped: original.scale_clipped,
            },
        ),
    })
}

fn encode_pixels(pixels: &[[f32; 3]]) -> Vec<[u16; 3]> {
    pixels.par_iter().map(|p| p.map(curve::encode)).collect()
}

/// The camera image of `codes`, as [`EditablePreview::image`] describes it.
fn image_of(
    width: u32,
    height: u32,
    codes: &[[u16; 3]],
    metadata: Metadata,
    scale: facts::Scale,
) -> CameraImage {
    let table = curve::table();
    let pixels = codes
        .par_iter()
        .map(|c| c.map(|c| table[c as usize]))
        .collect();
    let image = CameraImage {
        width,
        height,
        recovered: Default::default(),
        pixels,
        metadata,
        fast: true,
        scale_factor: scale.factor,
        scale_clipped: scale.clipped,
        hdr: Default::default(),
    };
    // Its pixels are recovered already: a second recovery of a reduced copy would
    // not be the original's.
    let _ = image.recovered.set(Arc::new(image.clone()));
    image
}

/// Writes `preview` to `path`: a temporary file in the same folder, synced, then
/// renamed to `path` only if nothing has that name. An existing file is never
/// replaced; that is an error of kind [`std::io::ErrorKind::AlreadyExists`]. A
/// crash may leave the temporary file (`.rmep-*.tmp`) behind for the folder's
/// owner to remove.
///
/// On a volume with neither an exclusive rename nor hard links (exFAT on macOS)
/// the file is copied into a name created only if free
/// ([`storage::persist_noclobber`](crate::storage::persist_noclobber)). A crash
/// during that copy can leave a partial file under the name: [`read`] refuses it,
/// and whoever rebuilds the editable preview removes it first, or this write fails
/// with `AlreadyExists`.
///
/// The folder is synced after the rename, so an error can follow a file already
/// complete at `path`: after a failed write, [`read`] tells whether the file is
/// usable before anything removes it.
pub fn write(path: &Path, preview: &EditablePreview) -> Result<()> {
    let image = &preview.image;
    check_sizes([image.width, image.height], preview.original)?;
    let codes = encode_pixels(&image.pixels);
    let pixels = match preview.codec {
        Codec::LosslessJxl16 => codec::encode(image.width, image.height, &codes)?,
        Codec::DitheredJxl12 => {
            // Built or read at 12 bits, every code is one `widen` made.
            let narrow: Vec<[u16; 3]> = codes.iter().map(|p| p.map(|c| c >> 4)).collect();
            ensure!(
                narrow
                    .iter()
                    .zip(&codes)
                    .all(|(n, c)| n.map(curve::widen) == *c),
                "A Smaller editable preview's pixels aren't 12-bit codes"
            );
            codec::encode(image.width, image.height, &narrow)?
        }
    };
    let mut head = Writer::default();
    head.u8(CODEC, preview.codec.code());
    head.u8(CURVE, CURVE_LOG);
    head.u32s(PREVIEW_SIZE, &[image.width, image.height]);
    head.u32s(ORIGINAL_SIZE, &preview.original);
    facts::write_identity(&mut head, IDENTITY, &preview.identity);
    let fact = facts::write_metadata(
        &image.metadata,
        facts::Scale {
            factor: image.scale_factor,
            clipped: image.scale_clipped,
        },
    );
    let exif = facts::write_exif(preview.exif.as_ref());
    let look = preview.camera_look.as_ref().map(|p| {
        let mut look = Writer::default();
        look.str(LOOK_PROFILE, &fitted::carried(p));
        look
    });

    let dir = crate::storage::parent_dir(path);
    let mut builder = tempfile::Builder::new();
    builder.prefix(".rmep-").suffix(".tmp");
    // Readable as any file the user makes (the umask applies), not only by its owner
    // as a temporary file is.
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
    let mut file = builder.tempfile_in(dir)?;
    {
        let mut out = std::io::BufWriter::new(file.as_file_mut());
        out.write_all(MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        let sections = [
            Some((HEAD, head.into_bytes())),
            Some((FACT, fact.into_bytes())),
            Some((EXIF, exif.into_bytes())),
            look.map(|look| (LOOK, look.into_bytes())),
            Some((PIXL, pixels)),
        ];
        for (tag, payload) in sections.into_iter().flatten() {
            out.write_all(&tag)?;
            out.write_all(&(payload.len() as u64).to_le_bytes())?;
            out.write_all(&payload)?;
            out.write_all(&hash(&payload).to_le_bytes())?;
        }
        out.flush()?;
    }
    file.as_file().sync_all()?;
    crate::storage::persist(file, path, crate::storage::Replace::NoClobber)
}

/// Refuses a preview of `preview` pixels standing for an `original` of that size
/// unless both are within what [`read`] opens.
fn check_sizes(preview: [u32; 2], original: [u32; 2]) -> Result<()> {
    let within = |[w, h]: [u32; 2], side: u32, pixels: u64| {
        w > 0 && h > 0 && w <= side && h <= side && u64::from(w) * u64::from(h) <= pixels
    };
    ensure!(
        within(preview, MAX_SIDE, MAX_PIXELS)
            && within(original, MAX_ORIGINAL_SIDE, MAX_ORIGINAL_PIXELS),
        "An editable preview of {}×{} pixels for an original of {}×{} is larger than editable previews may be",
        preview[0],
        preview[1],
        original[0],
        original[1]
    );
    Ok(())
}

fn hash(bytes: &[u8]) -> u64 {
    crate::storage::fnv1a(crate::storage::FNV_OFFSET, bytes)
}

/// The magic and version at the start of `file`; an error for anything but a
/// editable preview this release reads.
fn read_version(file: &mut impl Read) -> Result<u32> {
    let mut start = [0; 12];
    file.read_exact(&mut start)
        .context("Not an editable preview: the file is too short")?;
    ensure!(&start[..8] == MAGIC, "Not an editable preview");
    let version = u32::from_le_bytes([start[8], start[9], start[10], start[11]]);
    ensure!(version != 0, "Not an editable preview: version 0");
    ensure!(
        version <= VERSION,
        "This editable preview is format version {version}, newer than this release of RAWmakase reads (version {VERSION}): update RAWmakase to open it"
    );
    Ok(version)
}

/// A section's tag and payload length, read at the reader's position.
fn read_section_start(file: &mut impl Read) -> Result<([u8; 4], u64)> {
    let mut start = [0; 12];
    file.read_exact(&mut start)
        .context("An editable preview is cut short inside a section")?;
    let tag = [start[0], start[1], start[2], start[3]];
    let len = u64::from_le_bytes(start[4..].try_into().expect("8 bytes"));
    Ok((tag, len))
}

/// The HEAD record's facts.
struct Head {
    codec: Codec,
    preview: [u32; 2],
    original: [u32; 2],
    identity: Identity,
}

fn read_head(payload: &[u8]) -> Result<Head> {
    let r = Record::parse(payload)?;
    let codec = Codec::of(
        r.u8(CODEC)?
            .context("An editable preview without its codec")?,
    )?;
    let curve = r
        .u8(CURVE)?
        .context("An editable preview without its curve")?;
    ensure!(
        curve == CURVE_LOG,
        "This editable preview's pixels are on a curve ({curve}) only a later release of RAWmakase reads"
    );
    let preview = r
        .u32_array::<2>(PREVIEW_SIZE)?
        .context("An editable preview without its size")?;
    let original = r
        .u32_array::<2>(ORIGINAL_SIZE)?
        .context("An editable preview without its original's size")?;
    check_sizes(preview, original)?;
    Ok(Head {
        codec,
        preview,
        original,
        identity: facts::read_identity(&r, IDENTITY)?,
    })
}

/// The format version, the original's identity and size, and the preview's size of
/// the editable preview at `path`, without decoding its pixels. The file's structure is
/// checked to its end, so a file cut short is refused, but only the HEAD section's
/// hash is: [`read`] checks the rest.
pub fn read_header(path: &Path) -> Result<Header> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let size = file.get_ref().metadata()?.len();
    let version = read_version(&mut file)?;
    let (tag, len) = read_section_start(&mut file)?;
    ensure!(
        tag == HEAD,
        "An editable preview must start with its HEAD section"
    );
    ensure!(
        len <= MAX_RECORD && 12 + 12 + len + 8 <= size,
        "An editable preview is cut short inside its HEAD section"
    );
    let mut payload = vec![0; len as usize];
    file.read_exact(&mut payload)?;
    let mut stored = [0; 8];
    file.read_exact(&mut stored)?;
    ensure!(
        u64::from_le_bytes(stored) == hash(&payload),
        "An editable preview's HEAD section is damaged"
    );
    let head = read_head(&payload)?;
    // The other sections' lengths, to the end of the file.
    let mut at = 12 + 12 + len + 8;
    let mut seen = vec![HEAD];
    while at < size {
        let (tag, len) = read_section_start(&mut file)?;
        let end = at
            .checked_add(12 + 8)
            .and_then(|v| v.checked_add(len))
            .filter(|end| *end <= size)
            .context("An editable preview is cut short")?;
        seen.push(tag);
        file.seek(SeekFrom::Start(end))?;
        at = end;
    }
    ensure!(
        [FACT, EXIF, PIXL].iter().all(|t| seen.contains(t)),
        "An editable preview is cut short"
    );
    Ok(Header {
        version,
        codec: head.codec,
        identity: head.identity,
        original: head.original,
        preview: head.preview,
    })
}

/// The editable preview at `path`, its pixels decoded. Any damage, a file cut short, or
/// a version, codec or fact only a later release knows is an error.
pub fn read(path: &Path, cancel: Cancel<'_>) -> Result<EditablePreview> {
    let bytes = std::fs::read(path)?;
    ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
    let mut rest: &[u8] = &bytes;
    let version = read_version(&mut rest)?;
    let mut head = None;
    let mut fact = None;
    let mut exif = None;
    let mut pixl = None;
    let mut look = None;
    while !rest.is_empty() {
        let (tag, len) = read_section_start(&mut rest)?;
        let len = usize::try_from(len)
            .ok()
            .filter(|len| len.checked_add(8).is_some_and(|n| n <= rest.len()))
            .context("An editable preview is cut short")?;
        let (payload, after) = rest.split_at(len);
        let (stored, after) = after.split_at(8);
        ensure!(
            u64::from_le_bytes(stored.try_into().expect("8 bytes")) == hash(payload),
            "An editable preview's {} section is damaged",
            String::from_utf8_lossy(&tag)
        );
        rest = after;
        ensure!(
            tag == HEAD || head.is_some(),
            "An editable preview must start with its HEAD section"
        );
        let slot = match tag {
            HEAD => &mut head,
            FACT => &mut fact,
            EXIF => &mut exif,
            PIXL => &mut pixl,
            LOOK => &mut look,
            _ => continue,
        };
        ensure!(
            slot.replace(payload).is_none(),
            "An editable preview has two {} sections",
            String::from_utf8_lossy(&tag)
        );
    }
    let (Some(head), Some(fact), Some(exif), Some(pixl)) = (head, fact, exif, pixl) else {
        bail!("An editable preview is cut short");
    };
    let head = read_head(head)?;
    let (metadata, scale) = facts::read_metadata(&Record::parse(fact)?, head.original)?;
    let exif = facts::read_exif(&Record::parse(exif)?)?;
    let camera_look = look.map(|look| read_look(look, &metadata)).transpose()?;
    ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
    let [width, height] = head.preview;
    let codes = match head.codec {
        Codec::LosslessJxl16 => codec::decode(pixl, width, height)?,
        Codec::DitheredJxl12 => {
            let narrow = codec::decode(pixl, width, height)?;
            ensure!(
                narrow.iter().flatten().all(|q| *q < 4096),
                "An editable preview's 12-bit pixels are damaged"
            );
            narrow.iter().map(|p| p.map(curve::widen)).collect()
        }
    };
    ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
    Ok(EditablePreview {
        version,
        codec: head.codec,
        identity: head.identity,
        original: head.original,
        exif,
        camera_look,
        image: image_of(width, height, &codes, metadata, scale),
    })
}

/// The Camera Look a `LOOK` payload holds, for the photo `m` describes.
fn read_look(payload: &[u8], m: &Metadata) -> Result<CameraProfile> {
    let r = Record::parse(payload)?;
    let text = r
        .str(LOOK_PROFILE)?
        .context("An editable preview's Camera Look section holds no profile")?;
    fitted::from_carried(&text, m).map_err(|e| {
        if look_from_a_later_release(&text, &e) {
            e.context("This editable preview's Camera Look (fitted) was written by a later release of RAWmakase: update RAWmakase to open it")
        } else {
            e.context("This editable preview's Camera Look (fitted) can't be read")
        }
    })
}

/// Whether [`fitted::from_carried`] refused `text` as a later release would
/// write it: a carried version above 1, or a profile field this release doesn't
/// know (profiles refuse unknown fields).
fn look_from_a_later_release(text: &str, refused: &anyhow::Error) -> bool {
    let later_version = text
        .strip_prefix('v')
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(version, _)| version.parse::<u32>().ok())
        .is_some_and(|version| version > 1);
    later_version
        || refused
            .chain()
            .filter_map(|e| e.downcast_ref::<serde_json::Error>())
            .any(|e| e.to_string().starts_with("unknown field"))
}
