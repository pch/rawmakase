//! What adding a folder leaves out, read from the photos' files in the
//! background and kept in the catalog: capture times, so the photos sort as
//! they were taken, and photo info (camera, lens, exposure, size), with
//! where the photo falls in its camera's sequences, for Auto-Stack. Photos
//! imported from Lightroom already have the first two; Auto-Stack reads
//! their sequences when it needs them. Checksums are read beside them (see
//! `checksums`).
use super::CatalogSession;
use super::background::{Reader, can_read};
use crate::catalog::{PhotoId, SavedSequence};
use crate::exif::Sequence;
use crate::metadata::PhotoInfo;

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The readers under way, and what they have tried.
#[derive(Default)]
pub(super) struct Backfill {
    capture: Option<Reader<CaptureRead>>,
    /// Photos the capture-time backfill tried since the last online check.
    capture_tried: HashSet<PhotoId>,
    info: Option<Reader<Option<InfoRead>>>,
    /// Photo info was asked for while it was being read.
    info_again: bool,
    pub(super) checksums: super::checksums::Checksums,
    /// Reading the files of batches whose bursts are to be stacked.
    pub(super) bursts: Option<super::import_stacks::Reading>,
    /// Asked to read them while reading: once done, read again.
    pub(super) bursts_again: bool,
}

/// What one poll of a backfill saved.
#[derive(Debug, Default)]
pub(crate) struct Saved<T> {
    /// What was saved in the catalog and is now in the session's lists.
    pub saved: T,
    /// Saving failed; what was read is read again after the next online check.
    pub error: Option<anyhow::Error>,
    /// The reader finished, and photos it did not cover may want reading:
    /// start it again.
    pub start_again: bool,
}

impl CatalogSession {
    /// Reads the capture times still missing, of the photos `available` says
    /// are online and not tried since [`Self::retry_capture_times`].
    pub(crate) fn start_capture_times(&mut self, available: impl Fn(&Path) -> bool) {
        if self.backfill.capture.is_some() {
            return;
        }
        let tried = &self.backfill.capture_tried;
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| {
                p.captured.is_empty()
                    && p.master.is_none()
                    && !tried.contains(&p.id)
                    && has_capture_time(&p.path)
                    && available(&p.path)
            })
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.backfill
                .capture_tried
                .extend(todo.iter().map(|(id, _)| *id));
            self.backfill.capture = Some(Reader::start(todo, self.wake.clone(), read_capture));
        }
    }
    /// Drops the readers under way, whose results could otherwise land on
    /// photos given the ids of photos removed meanwhile. The next starts read
    /// what is still missing.
    /// Stops the background reads before a change that may give removed
    /// photos' ids to new ones; the next starts read what is still missing.
    pub(crate) fn stop_background_reads(&mut self) {
        self.stop_backfill();
    }
    pub(super) fn stop_backfill(&mut self) {
        let checksums = std::mem::take(&mut self.backfill.checksums).stopped();
        self.backfill = Backfill {
            checksums,
            ..Backfill::default()
        };
    }
    /// Lets the next [`Self::start_capture_times`] try every photo again, as
    /// once more photos are online.
    pub(crate) fn retry_capture_times(&mut self) {
        self.backfill.capture_tried.clear();
    }
    /// Whether capture times are being read.
    #[cfg(test)]
    pub(crate) fn reading_capture_times(&self) -> bool {
        self.backfill.capture.is_some()
    }
    /// Saves the capture times read so far, a virtual copy taking its
    /// master's. Returns the times saved; the photos keep their order.
    pub(crate) fn poll_capture_times(&mut self) -> Saved<Vec<(PhotoId, String)>> {
        let Some(reader) = &self.backfill.capture else {
            return Saved::default();
        };
        let (read, done) = reader.poll();
        if done {
            self.backfill.capture = None;
        }
        let dated: Vec<(PhotoId, String)> = read
            .into_iter()
            .filter_map(|(id, read)| match read {
                CaptureRead::Dated(time) => Some((id, time)),
                _ => None,
            })
            .collect();
        let mut out = Saved {
            start_again: done,
            ..Default::default()
        };
        if !dated.is_empty() {
            match self.catalog.fill_capture_times(&dated) {
                Ok(()) => {
                    self.apply_capture_times(&dated);
                    out.saved = dated;
                }
                Err(e) => out.error = Some(e),
            }
        }
        out
    }
    fn apply_capture_times(&mut self, times: &[(PhotoId, String)]) {
        let times: HashMap<PhotoId, &String> = times.iter().map(|(id, t)| (*id, t)).collect();
        for photo in &mut self.photos {
            if photo.captured.is_empty()
                && let Some(time) = times
                    .get(&photo.id)
                    .or_else(|| photo.master.and_then(|m| times.get(&m)))
            {
                photo.captured = (*time).clone();
            }
        }
    }
    /// Reads the info of the photos that have none or are to be read again
    /// (see [`crate::catalog::Catalog::photos_to_read_info`]), of those
    /// `available` says are online; asked while reading, it reads again once
    /// done.
    pub(crate) fn start_photo_info(&mut self, available: impl Fn(&Path) -> bool) {
        if self.backfill.info.is_some() {
            self.backfill.info_again = true;
            return;
        }
        let Ok(missing) = self.catalog.photos_to_read_info() else {
            return;
        };
        let missing: HashSet<PhotoId> = missing.into_iter().collect();
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| missing.contains(&p.id) && available(&p.path))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.backfill.info = Some(Reader::start(
                todo,
                self.wake.clone(),
                read_info_and_sequence,
            ));
        }
    }
    /// Whether photo info is being read.
    pub(crate) fn reading_photo_info(&self) -> bool {
        self.backfill.info.is_some()
    }
    /// Saves the photo info read so far. Returns how many photos' info was
    /// saved; a file that could not be read is left for the next online check.
    pub(crate) fn poll_photo_info(&mut self) -> Saved<usize> {
        let Some(reader) = &self.backfill.info else {
            return Saved::default();
        };
        let (read, done) = reader.poll();
        let mut out = Saved::default();
        if done {
            self.backfill.info = None;
            out.start_again = std::mem::take(&mut self.backfill.info_again);
        }
        let mut sequences = Vec::new();
        let infos: Vec<_> = read
            .into_iter()
            .filter_map(|(id, read)| {
                let InfoRead { info, sequence } = read?;
                sequences.extend(sequence.map(|sequence| (id, sequence)));
                Some((id, info))
            })
            .collect();
        if !infos.is_empty() {
            let saved = self
                .catalog
                .fill_photo_info(&infos)
                .and_then(|()| self.catalog.save_sequences(&sequences));
            match saved {
                Ok(()) => out.saved = infos.len(),
                Err(e) => out.error = Some(e),
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
enum CaptureRead {
    Dated(String),
    /// The file has no date; it keeps sorting before dated photos.
    Undated,
    /// The file could not be opened, e.g. it is offline; tried again later.
    Unreadable,
}

/// Whether a capture time can be read from this kind of file at all, so
/// files that never yield one are not opened on every launch: JPEG, TIFF,
/// PNG, WebP, AVIF, HEIF and JPEG XL, and every RAW (the EXIF reader's, and the
/// CR3, CRW, MRW and X3F files RAWmakase's own decoders date).
fn has_capture_time(path: &Path) -> bool {
    use crate::storage::RasterFormat;
    match RasterFormat::of(path) {
        Some(
            RasterFormat::Jpeg
            | RasterFormat::Tiff
            | RasterFormat::Png
            | RasterFormat::Webp
            | RasterFormat::Avif
            | RasterFormat::Heif
            | RasterFormat::Jxl,
        ) => true,
        None => crate::storage::is_raw(path),
    }
}

fn read_capture(path: &Path) -> CaptureRead {
    if !can_read(path) {
        return CaptureRead::Unreadable;
    }
    match capture_time(path) {
        Some(time) => CaptureRead::Dated(time),
        None => CaptureRead::Undated,
    }
}

/// When the photo in the file at `path` was taken, in Lightroom's form: from
/// its EXIF, else from the containers whose dates the EXIF reader can't read
/// (the RAW decoders tell CR3, CRW, MRW and X3F files by their first bytes).
pub(crate) fn capture_time(path: &Path) -> Option<String> {
    crate::exif::capture_time(path).or_else(|| crate::raw::capture_time(path))
}

/// What the photo-info backfill reads of a file.
struct InfoRead {
    info: Option<PhotoInfo>,
    /// `None` when it could not be read now; read when Auto-Stack wants it.
    sequence: Option<SavedSequence>,
}

/// The file's info and sequence, read in one pass over new photos; `None`
/// when the file cannot be read now.
fn read_info_and_sequence(path: &Path) -> Option<InfoRead> {
    let info = read_info(path)?;
    Some(InfoRead {
        info,
        sequence: read_sequence(path, crate::exif::sequence),
    })
}

/// What the file at `path` says of its sequence, as `read` reads it, with
/// the file it was read from; `None` when the file can't be read now (its
/// contents included: a file without permission keeps its stamp once
/// allowed) or changed while it was read, so nothing is kept of it.
pub(crate) fn read_sequence(
    path: &Path,
    read: fn(&Path) -> Option<Sequence>,
) -> Option<SavedSequence> {
    let stamp = crate::storage::Stamp::read(path).ok()?;
    if !can_read(path) {
        return None;
    }
    let sequence = read(path);
    (crate::storage::Stamp::read(path).ok()? == stamp).then_some(SavedSequence { stamp, sequence })
}

/// The file's info: `None` when it cannot be read now, `Some(None)` when it
/// has none.
pub(crate) fn read_info(path: &Path) -> Option<Option<PhotoInfo>> {
    if !can_read(path) {
        return None;
    }
    Some(if crate::storage::is_raw(path) {
        // An X3F's, which RAWmakase doesn't decode, from its header alone.
        // One with none may not have been read whole (its preview); its
        // header is read again later, which costs little.
        if let Some(undecoded) = crate::raw::undecoded(path) {
            return (undecoded.info != PhotoInfo::default()).then_some(Some(undecoded.info));
        }
        // A RAW that cannot be opened now (still copying, a network error) is
        // tried again later.
        Some(PhotoInfo::from_metadata(
            &crate::photo::open(path).ok()?.metadata,
        ))
    } else {
        let mut info = crate::exif::photo_info(path).unwrap_or_default();
        // A rendered file's size as shown, after its orientation, from its
        // header alone. A header that cannot be read yet (a file still being copied) is
        // tried again later.
        info.dimensions = Some(crate::raster::dimensions(path)?);
        (info != PhotoInfo::default()).then_some(info)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An X3F, whose sensor data RAWmakase doesn't decode, has the info of
    /// its preview's EXIF, else of its property list (SD9, SD10), read
    /// without decoding; one that can't be read is tried again (#671).
    #[test]
    fn an_x3f_has_the_info_of_its_preview_exif_or_property_list() -> anyhow::Result<()> {
        use crate::exif::{CameraExif, Field, tag};
        use crate::export::exif::{Container, camera_raw, x3f_with_properties};
        let dir = tempfile::tempdir()?;
        let quattro = dir.path().join("SDIM4388.X3F");
        let camera = CameraExif {
            main: vec![
                Field::ascii(tag::MAKE, "SIGMA"),
                Field::ascii(tag::MODEL, "SIGMA dp2 Quattro"),
            ],
            exif: vec![
                Field::rational(tag::EXPOSURE_TIME, 1, 250),
                Field::rational(tag::F_NUMBER, 28, 10),
                Field::short(tag::ISO, 200),
                Field::rational(tag::FOCAL_LENGTH, 30, 1),
                Field::ascii(tag::LENS_MODEL, "30mm F2.8"),
            ],
            gps: Vec::new(),
        };
        std::fs::write(&quattro, camera_raw(Container::X3f, camera))?;
        let info = read_info(&quattro).flatten().expect("the preview's info");
        assert_eq!(info.camera.as_deref(), Some("SIGMA dp2 Quattro"));
        assert_eq!(info.lens.as_deref(), Some("30mm F2.8"));
        assert_eq!(info.exposure_text().as_deref(), Some("1/250 sec at f/2.8"));
        assert_eq!(info.iso_text().as_deref(), Some("ISO 200"));
        assert_eq!(info.focal_text().as_deref(), Some("30 mm"));
        let sd9 = dir.path().join("RAW_SIGMA_SD9.X3F");
        std::fs::write(
            &sd9,
            x3f_with_properties(&[
                ("ISO", "100"),
                ("CAMMANUF", "SIGMA"),
                ("CAMMODEL", "SIGMA SD9"),
                ("EXPTIME", "41408"),
                ("LENSMODEL", "16"),
                ("APERTURE", "5.18736"),
                ("FLENGTH", "53"),
            ]),
        )?;
        let info = read_info(&sd9).flatten().expect("the property list's info");
        assert_eq!(info.camera.as_deref(), Some("SIGMA SD9"));
        assert_eq!(info.lens, None);
        assert_eq!(info.exposure_text().as_deref(), Some("1/24 sec at f/5.2"));
        assert_eq!(info.iso_text().as_deref(), Some("ISO 100"));
        assert_eq!(info.focal_text().as_deref(), Some("53 mm"));
        // An X3F that says nothing (or whose preview couldn't be read) is
        // tried again later, not kept as a photo without info.
        let blank = dir.path().join("blank.X3F");
        std::fs::write(&blank, x3f_with_properties(&[]))?;
        assert_eq!(read_info(&blank), None);
        // Gone, it is tried again later.
        std::fs::remove_file(&sd9)?;
        assert_eq!(read_info(&sd9), None);
        Ok(())
    }
    #[test]
    fn capture_times_are_read_from_rendered_files_and_raws() {
        for name in [
            "A.jpg", "A.JPG", "A.jpeg", "A.JPEG", "A.tif", "A.TIF", "A.tiff", "A.png", "A.PNG",
            "A.NEF", "A.dng", "A.ARW", "A.raf", "A.webp", "A.avif", "A.heic", "A.HIF", "A.jxl",
            "A.CR3", "A.crw", "A.X3F", "A.mrw",
        ] {
            assert!(has_capture_time(Path::new(name)), "{name}");
        }
        for name in ["A.gif", "A.xmp", "A", "._A.NEF"] {
            assert!(!has_capture_time(Path::new(name)), "{name}");
        }
    }
}
