//! A digest of everything that makes up one photo's edit and descriptive
//! metadata, and a conditional edit that is written only while the photo still
//! has the digest it was read with.
//!
//! An extension reads photos, works out new settings and proposes them later;
//! meanwhile the user, a Sync or `rawmakase control` may have changed the
//! photo. [`Catalog::photo_fingerprint`] names the state a read saw, and
//! [`Catalog::change_edits_if`] recomputes it inside the write transaction and
//! refuses when it moved on, so a proposal never lands on an edit it wasn't
//! computed from. The digest is computed, not stored: the catalog's format is
//! unchanged.
//!
//! The encoding is canonical: every table is read in its primary key's order,
//! and every value is written with its type and length, so two states hash
//! alike only when they are the same.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, EditChange, PhotoId};
use anyhow::Result;
use sha2::{Digest, Sha256};

/// What [`Catalog::change_edits_if`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conditional {
    /// The photo still had the expected fingerprint, and the change was written.
    Written,
    /// The photo's edit or metadata changed, or the photo is gone: nothing was
    /// written.
    Moved,
}

/// The canonical encoding, fed straight into the digest.
struct Canon(Sha256);
impl Canon {
    fn tag(&mut self, tag: &str) {
        self.bytes(b'T', tag.as_bytes());
    }
    fn bytes(&mut self, kind: u8, bytes: &[u8]) {
        self.0.update([kind]);
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }
    fn text(&mut self, text: Option<&str>) {
        match text {
            Some(text) => self.bytes(b's', text.as_bytes()),
            None => self.0.update(*b"n"),
        }
    }
    fn int(&mut self, value: Option<i64>) {
        match value {
            Some(value) => self.bytes(b'i', &value.to_le_bytes()),
            None => self.0.update(*b"n"),
        }
    }
    fn real(&mut self, value: Option<f64>) {
        match value {
            Some(value) => self.bytes(b'r', &value.to_bits().to_le_bytes()),
            None => self.0.update(*b"n"),
        }
    }
    /// The number of rows a table has for the photo, so rows can't run into
    /// the next table's.
    fn rows(&mut self, count: usize) {
        self.bytes(b'c', &(count as u64).to_le_bytes());
    }
}

/// The photo's fingerprint as `db` sees it, or none when there is no such
/// photo.
pub(super) fn fingerprint(db: &impl Reads, id: PhotoId) -> Result<Option<String>> {
    row! {
        struct PhotoRow {
            folder: i64,
            filename: String,
            original_path: String,
            captured: String,
            rating: i64,
            flag: i64,
            label: String,
            copy_name: String,
            master_id: Option<i64>,
            orientation: Option<String>,
            lightroom_develop: Option<String>,
            recipe: Option<String>,
            export_options: Option<String>,
            identity: Option<String>,
            edited_at: Option<String>,
        }
    }
    let photo: Option<PhotoRow> = db.read_optional(
        sql!(
            "SELECT folder,filename,original_path,captured,rating,flag,label,copy_name,\
             master_id,orientation,lightroom_develop,recipe,export_options,identity,edited_at \
             FROM photos WHERE id=?"
        ),
        &[&id],
    )?;
    let Some(p) = photo else {
        return Ok(None);
    };
    let mut c = Canon(Sha256::new());
    c.tag("photos");
    c.int(Some(p.folder));
    c.text(Some(&p.filename));
    c.text(Some(&p.original_path));
    c.text(Some(&p.captured));
    c.int(Some(p.rating));
    c.int(Some(p.flag));
    c.text(Some(&p.label));
    c.text(Some(&p.copy_name));
    c.int(p.master_id);
    c.text(p.orientation.as_deref());
    c.text(p.lightroom_develop.as_deref());
    c.text(p.recipe.as_deref());
    c.text(p.export_options.as_deref());
    c.text(p.identity.as_deref());
    c.text(p.edited_at.as_deref());

    c.tag("local_edits");
    let local: Option<String> =
        db.read_optional(sql!("SELECT data FROM local_edits WHERE photo=?"), &[&id])?;
    c.text(local.as_deref());

    c.tag("develop_history");
    let history: Option<Vec<u8>> = db.read_optional(
        sql!("SELECT data FROM develop_history WHERE photo=?"),
        &[&id],
    )?;
    match history {
        Some(history) => c.bytes(b'b', &history),
        None => c.text(None),
    }

    row! {
        struct LightroomStep {
            position: i64,
            name: String,
            created: Option<f64>,
            text: String,
        }
    }
    c.tag("lightroom_history");
    let steps: Vec<LightroomStep> = db.read(
        sql!(
            "SELECT position,name,created,text FROM lightroom_history \
             WHERE photo=? ORDER BY position"
        ),
        &[&id],
    )?;
    c.rows(steps.len());
    for s in &steps {
        c.int(Some(s.position));
        c.text(Some(&s.name));
        c.real(s.created);
        c.text(Some(&s.text));
    }

    row! {
        struct Field {
            field: String,
            state: String,
        }
    }
    c.tag("photo_fields");
    let fields: Vec<Field> = db.read(
        sql!("SELECT field,state FROM photo_fields WHERE photo=? ORDER BY field"),
        &[&id],
    )?;
    c.rows(fields.len());
    for f in &fields {
        c.text(Some(&f.field));
        c.text(Some(&f.state));
    }

    row! {
        struct Text {
            field: String,
            lang: String,
            position: i64,
            value: String,
        }
    }
    c.tag("photo_text");
    let texts: Vec<Text> = db.read(
        sql!(
            "SELECT field,lang,position,value FROM photo_text \
             WHERE photo=? ORDER BY field,lang"
        ),
        &[&id],
    )?;
    c.rows(texts.len());
    for t in &texts {
        c.text(Some(&t.field));
        c.text(Some(&t.lang));
        c.int(Some(t.position));
        c.text(Some(&t.value));
    }

    row! {
        struct Creator {
            position: i64,
            name: String,
        }
    }
    c.tag("photo_creators");
    let creators: Vec<Creator> = db.read(
        sql!("SELECT position,name FROM photo_creators WHERE photo=? ORDER BY position"),
        &[&id],
    )?;
    c.rows(creators.len());
    for cr in &creators {
        c.int(Some(cr.position));
        c.text(Some(&cr.name));
    }

    row! {
        struct Capture {
            captured: String,
            subsec: Option<String>,
            offset: Option<String>,
        }
    }
    c.tag("photo_capture");
    let capture: Option<Capture> = db.read_optional(
        sql!("SELECT captured,subsec,\"offset\" FROM photo_capture WHERE photo=?"),
        &[&id],
    )?;
    c.rows(usize::from(capture.is_some()));
    if let Some(cap) = &capture {
        c.text(Some(&cap.captured));
        c.text(cap.subsec.as_deref());
        c.text(cap.offset.as_deref());
    }

    row! {
        struct Location {
            lat: Option<f64>,
            lon: Option<f64>,
            alt: Option<f64>,
            cleared: i64,
        }
    }
    c.tag("photo_location");
    let location: Option<Location> = db.read_optional(
        sql!("SELECT lat,lon,alt,cleared FROM photo_location WHERE photo=?"),
        &[&id],
    )?;
    c.rows(usize::from(location.is_some()));
    if let Some(l) = &location {
        c.real(l.lat);
        c.real(l.lon);
        c.real(l.alt);
        c.int(Some(l.cleared));
    }

    c.tag("photo_keywords");
    let keywords: Vec<i64> = db.read(
        sql!("SELECT keyword FROM photo_keywords WHERE photo=? ORDER BY keyword"),
        &[&id],
    )?;
    c.rows(keywords.len());
    for k in keywords {
        c.int(Some(k));
    }

    let digest = c.0.finalize();
    Ok(Some(digest.iter().map(|b| format!("{b:02x}")).collect()))
}

impl Catalog {
    /// A SHA-256, as hex, of everything that makes up photo `id`'s edit (its
    /// recipe, export options, file identity, Lightroom settings and History,
    /// its spots and masks, its RAWmakase History) and its descriptive
    /// metadata (rating, flag, label, fields, text, creators, capture time,
    /// location, keywords), with its folder and file name. None when there is
    /// no such photo. Two reads give the same value exactly when none of
    /// those changed in between.
    pub fn photo_fingerprint(&self, id: PhotoId) -> Result<Option<String>> {
        fingerprint(&self.db, id)
    }

    /// Writes each of `changes`, a photo's change with the fingerprint it
    /// was computed from, only while that photo's fingerprint is still the
    /// one given, all in one write transaction: a photo that changed since it
    /// was read is left as it is and reported [`Conditional::Moved`], the
    /// others are written. `admitted` is asked once, inside the transaction
    /// before anything is written; when it says no, nothing is written and
    /// the result is `None`. It lets the caller's own state (an extension's
    /// request still admitted) and the photos' state be checked at the same
    /// moment. With the same checks and storage as [`Catalog::change_edits`].
    pub fn change_edits_if(
        &mut self,
        changes: &[(&str, EditChange<'_, '_>)],
        admitted: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<Conditional>>> {
        // As `change_edits` does: the files and the stored edits are read
        // before the transaction takes its lock.
        let checked = changes
            .iter()
            .map(|(_, change)| self.check_edit(change))
            .collect::<Result<Vec<_>>>()?;
        let edited_at = rawmakase_model::time::now_text();
        let write = |w: &mut super::db::Write<'_>| {
            if !admitted() {
                return Ok(None);
            }
            let mut written = Vec::with_capacity(changes.len());
            for ((expected, change), checked) in changes.iter().zip(&checked) {
                let id = match change {
                    EditChange::Save(e) => e.id,
                    EditChange::Clear { id } => *id,
                };
                if fingerprint(&*w, id)?.as_deref() != Some(*expected) {
                    written.push(Conditional::Moved);
                    continue;
                }
                super::edit_rows::write_edit(w, checked, &edited_at)?;
                written.push(Conditional::Written);
            }
            Ok(Some(written))
        };
        let recipes = changes.iter().flat_map(|(_, change)| match change {
            EditChange::Save(e) => {
                let history = match e.history {
                    super::HistoryUpdate::Replace(h) => Some(h),
                    super::HistoryUpdate::Keep => None,
                };
                std::iter::once(e.recipe)
                    .chain(history.map(|h| &h.origin))
                    .chain(
                        history
                            .into_iter()
                            .flat_map(|h| h.steps.iter().map(|s| &s.recipe)),
                    )
                    .collect::<Vec<_>>()
            }
            EditChange::Clear { .. } => Vec::new(),
        });
        let written = if changes
            .iter()
            .any(|(_, c)| matches!(c, EditChange::Save(_)))
        {
            self.write_recipes(super::format_upgrade::Stores::of(recipes), write)?
        } else {
            self.db.write(write)?
        };
        if let Some(written) = &written {
            for (checked, result) in checked.iter().zip(written) {
                if *result == Conditional::Written {
                    checked.committed();
                }
            }
        }
        Ok(written)
    }
}
