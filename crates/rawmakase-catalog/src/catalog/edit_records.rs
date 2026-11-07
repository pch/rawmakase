//! Reading the edits that `crate::edits` resolves: one photo's, or many photos'
//! in one transaction.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::edits::{EditRecord, PhotoRecord};
use anyhow::Result;

impl Catalog {
    /// Photo `id`'s edit as stored.
    pub fn edit_record(&self, id: PhotoId) -> Result<EditRecord> {
        edit_record(&self.db, id)
    }
    /// The records of `ids`, in order, read in one transaction: one consistent
    /// state of the catalog however many photos there are.
    pub fn photo_records(&self, ids: &[PhotoId]) -> Result<Vec<PhotoRecord>> {
        row! {
            struct Rated {
                rating: i32,
                label: String,
                captured: String,
            }
        }
        self.db.snapshot(|snapshot| {
            ids.iter()
                .map(|&id| {
                    let rated: Rated = snapshot.read_one(
                        sql!("SELECT rating,label,captured FROM photos WHERE id=?"),
                        &[&id],
                    )?;
                    Ok(PhotoRecord {
                        edit: edit_record(snapshot, id)?,
                        descriptive: super::descriptive::read(snapshot, id)?,
                        keywords: super::descriptive::keywords(snapshot, id)?,
                        rating: rated.rating,
                        label: rated.label,
                        captured: rated.captured,
                    })
                })
                .collect()
        })
    }
}

/// Photo `id`'s edit as stored.
fn edit_record(db: &impl Reads, id: PhotoId) -> Result<EditRecord> {
    row! {
        struct Columns {
            recipe: Option<String>,
            export: Option<String>,
            identity: Option<String>,
            lightroom: Option<String>,
        }
    }
    let columns: Columns = db.read_one(
        sql!("SELECT recipe,export_options,identity,lightroom_develop FROM photos WHERE id=?"),
        &[&id],
    )?;
    Ok(EditRecord {
        recipe: columns.recipe,
        export: columns.export,
        identity: columns.identity,
        local: super::edits::local_text(db, id)?,
        lightroom: columns.lightroom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::edits::{Origin, resolve};
    use crate::{
        export_settings::ExportOptions, model::recipe::Recipe, raw_defaults::DevelopDefaults,
    };
    use std::path::Path;

    /// Each photo's catalog id and file.
    type Photos = Vec<(PhotoId, std::path::PathBuf)>;
    /// A catalog of three copies of the synthetic chart DNG.
    fn catalog() -> Result<(tempfile::TempDir, Catalog, Photos)> {
        let d = tempfile::tempdir()?;
        let photos = d.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng", "c.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        let mut c = Catalog::create(&d.path().join("resolve.rawmakase"))?;
        c.add_folder(&photos)?;
        let photos = c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
        Ok((d, c, photos))
    }
    /// The chart's metadata as the decoder reports it, enough to resolve its edits;
    /// the catalog crate does not decode photos.
    fn chart_metadata() -> crate::camera_data::Metadata {
        crate::camera_data::Metadata {
            make: "RAWmakase".into(),
            model: "Synthetic D65".into(),
            width: 64,
            height: 64,
            wb: [2., 1., 1.5],
            daylight_wb: [2., 1., 1.5],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        }
    }
    fn set_lightroom(c: &Catalog, id: PhotoId, text: &str) -> Result<()> {
        c.db_for_tests().execute(
            "UPDATE photos SET lightroom_develop=? WHERE id=?",
            rusqlite::params![text, id.0],
        )?;
        Ok(())
    }

    #[test]
    fn empty_lightroom_text_is_no_edit() -> Result<()> {
        let (_d, c, photos) = catalog()?;
        let id = photos[0].0;
        assert!(!c.has_edit(id)?);
        set_lightroom(&c, id, "")?;
        assert!(!c.has_edit(id)?);
        set_lightroom(&c, id, "s = { Exposure2012 = 0.5 }")?;
        assert!(c.has_edit(id)?);
        Ok(())
    }

    #[test]
    fn a_saved_edit_comes_first_then_lightroom_then_the_defaults() -> Result<()> {
        let (_d, mut c, photos) = catalog()?;
        let metadata = chart_metadata();
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let defaults = crate::raw_defaults::brighter_defaults();
        let resolve_photo = |c: &Catalog, (id, path): &(PhotoId, std::path::PathBuf)| {
            resolve(&c.edit_record(*id)?, path, &metadata, &profiles, &defaults)
        };
        // Saved, masks included: they are stored apart from the recipe.
        let mut saved = Recipe::with_profiles(&metadata, &profiles);
        saved.exposure = 0.4;
        saved.masks.push(crate::model::masks::MaskGroup {
            components: vec![crate::model::masks::MaskComponent::new(
                crate::model::masks::MaskShape::Radial {
                    center: [0.5, 0.5],
                    radii: [0.2, 0.1],
                    angle: 0.,
                    feather: 0.5,
                },
            )],
            adjust: crate::model::masks::LocalAdjust {
                shadows: 0.5,
                ..Default::default()
            },
            ..Default::default()
        });
        let export = ExportOptions {
            quality: 80,
            max_edge: 0,
            ..Default::default()
        };
        let (a, b, unedited) = (&photos[0], &photos[1], &photos[2]);
        c.save_edit(
            a.0,
            &a.1,
            &crate::storage::Identity::read(&a.1)?,
            &saved,
            &export,
            super::super::HistoryUpdate::Keep,
        )?;
        // A Lightroom edit under it changes nothing.
        set_lightroom(&c, a.0, "s = { Exposure2012 = 0.25 }")?;
        let resolved = resolve_photo(&c, a)?;
        assert_eq!(resolved.origin, Origin::Saved);
        assert_eq!(resolved.recipe, saved);
        assert_eq!(resolved.export.quality, 80);
        assert_eq!(resolved.recipe, c.load_edit(a.0, &a.1)?.unwrap().recipe);

        let text = "s = { Exposure2012 = 0.25 }";
        set_lightroom(&c, b.0, text)?;
        let resolved = resolve_photo(&c, b)?;
        assert_eq!(resolved.origin, Origin::Lightroom);
        // From Adobe Default, as Lightroom stores it, whatever the raw defaults.
        assert_eq!(
            resolved.recipe,
            crate::lr_develop::convert_develop(text, &metadata, &profiles, None)?.0
        );
        assert_eq!(resolved.recipe.exposure, 0.25);

        let resolved = resolve_photo(&c, unedited)?;
        assert_eq!(resolved.origin, Origin::Defaults);
        assert_eq!(
            resolved.recipe,
            defaults.resolve(&metadata, &profiles).recipe
        );
        assert_eq!(resolved.recipe.exposure, 0.7);
        // Empty Lightroom settings are none.
        set_lightroom(&c, unedited.0, "")?;
        assert_eq!(resolve_photo(&c, unedited)?.origin, Origin::Defaults);
        Ok(())
    }

    /// A source chosen once ([`crate::edits::Start`]) and resolved later gives what
    /// [`resolve`] gives; the sources only previews and Develop use keep their own
    /// rules.
    #[test]
    fn a_captured_start_resolves_as_resolve_does() -> Result<()> {
        use crate::edits::Start;
        use std::sync::Arc;
        let (_d, mut c, photos) = catalog()?;
        let metadata = chart_metadata();
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let defaults = Arc::new(crate::raw_defaults::brighter_defaults());
        let (a, b, unedited) = (&photos[0], &photos[1], &photos[2]);
        let mut saved = Recipe::with_profiles(&metadata, &profiles);
        saved.exposure = 0.4;
        c.save_edit(
            a.0,
            &a.1,
            &crate::storage::Identity::read(&a.1)?,
            &saved,
            &ExportOptions::default(),
            super::super::HistoryUpdate::Keep,
        )?;
        set_lightroom(&c, b.0, "s = { Exposure2012 = 0.25 }")?;
        set_lightroom(&c, unedited.0, "")?;
        for (photo, origin) in [
            (a, Origin::Saved),
            (b, Origin::Lightroom),
            (unedited, Origin::Defaults),
        ] {
            let record = c.edit_record(photo.0)?;
            let start = Start::capture(&record, &photo.1, defaults.clone())?;
            let later = start.resolve(&metadata, &profiles)?;
            let now = resolve(&record, &photo.1, &metadata, &profiles, &defaults)?;
            assert_eq!(
                (
                    later.origin,
                    &later.recipe,
                    later.export.quality,
                    &later.warnings
                ),
                (origin, &now.recipe, now.export.quality, &now.warnings)
            );
        }
        // An unreadable Lightroom edit is an error either way; only `resolve` says
        // whose it is.
        set_lightroom(&c, b.0, "not Lightroom")?;
        let record = c.edit_record(b.0)?;
        let later = Start::capture(&record, &b.1, defaults.clone())?
            .resolve(&metadata, &profiles)
            .unwrap_err();
        let now = resolve(&record, &b.1, &metadata, &profiles, &defaults).unwrap_err();
        assert!(!format!("{later:#}").contains("Its Lightroom edit can't be read"));
        assert!(format!("{now:#}").starts_with("Its Lightroom edit can't be read"));
        // A preview's recipe is read as it is, not validated, and Adobe Default
        // ignores the defaults.
        let mut out_of_bounds = saved.clone();
        out_of_bounds.exposure = 99.;
        assert!(out_of_bounds.validate().is_err());
        let json = serde_json::to_string(&out_of_bounds)?;
        assert_eq!(
            Start::RecipeJson(json)
                .resolve(&metadata, &profiles)?
                .recipe,
            out_of_bounds
        );
        let adobe = Start::AdobeDefault.resolve(&metadata, &profiles)?;
        assert_eq!(
            (adobe.origin, adobe.recipe),
            (
                Origin::Defaults,
                Recipe::with_profiles(&metadata, &profiles)
            )
        );
        Ok(())
    }

    /// Issue #870: a file whose modification time alone changed, as a cloud
    /// mount or `touch` can leave it, keeps its edit and takes saves; one whose
    /// bytes changed stays protected.
    #[test]
    fn a_new_modification_time_alone_keeps_the_edit_and_its_saves() -> Result<()> {
        use crate::storage::Identity;
        let (_d, mut c, photos) = catalog()?;
        let (id, path) = &photos[0];
        let opened = Identity::read(path)?;
        let mut edit = Recipe {
            exposure: 0.1,
            ..Default::default()
        };
        let save = |c: &mut Catalog, source: &Identity, edit: &Recipe| {
            c.save_edit(
                *id,
                path,
                source,
                edit,
                &ExportOptions::default(),
                super::super::HistoryUpdate::Keep,
            )
        };
        save(&mut c, &opened, &edit)?;

        let file = std::fs::File::options().write(true).open(path)?;
        let modified = file.metadata()?.modified()?;
        file.set_modified(modified + std::time::Duration::from_secs(2))?;
        drop(file);
        let touched = Identity::read(path)?;
        assert_ne!(touched, opened);
        assert_eq!(c.load_edit(*id, path)?.unwrap().recipe, edit);
        // The editor still holds the identity read when the photo was opened.
        edit.exposure = 0.2;
        save(&mut c, &opened, &edit)?;
        assert_eq!(c.load_edit(*id, path)?.unwrap().recipe, edit);
        // The saved edit now names the file as it is.
        let stored: Identity = serde_json::from_str(&c.edit_record(*id)?.identity.expect("saved"))?;
        assert_eq!(stored, touched);

        // Same size, other bytes: another photo, never given this edit.
        let mut bytes = std::fs::read(path)?;
        bytes[16] ^= 0xff;
        std::fs::write(path, bytes)?;
        let error = format!("{:#}", c.load_edit(*id, path).unwrap_err());
        assert!(error.contains("protected"), "{error}");
        assert!(save(&mut c, &opened, &edit).is_err());
        assert!(save(&mut c, &Identity::read(path)?, &edit).is_err());
        Ok(())
    }

    #[test]
    fn an_edit_that_cant_be_used_is_an_error_never_the_defaults() -> Result<()> {
        let (_d, mut c, photos) = catalog()?;
        let metadata = chart_metadata();
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let defaults = DevelopDefaults::default();
        let resolve_photo = |c: &Catalog, (id, path): &(PhotoId, std::path::PathBuf)| {
            c.edit_record(*id)
                .and_then(|record| resolve(&record, path, &metadata, &profiles, &defaults))
                .map(|r| r.origin)
                .map_err(|e| format!("{e:#}"))
        };
        let (changed, unreadable, lightroom) = (&photos[0], &photos[1], &photos[2]);
        let edit = Recipe::with_profiles(&metadata, &profiles);
        c.save_edit(
            changed.0,
            &changed.1,
            &crate::storage::Identity::read(&changed.1)?,
            &edit,
            &ExportOptions::default(),
            super::super::HistoryUpdate::Keep,
        )?;
        // The file it was saved for was replaced: the edit is protected.
        let mut bytes = std::fs::read(&changed.1)?;
        bytes.extend_from_slice(b"changed");
        std::fs::write(&changed.1, bytes)?;
        let error = resolve_photo(&c, changed).unwrap_err();
        assert!(error.contains("protected"), "{error}");

        c.save_edit(
            unreadable.0,
            &unreadable.1,
            &crate::storage::Identity::read(&unreadable.1)?,
            &edit,
            &ExportOptions::default(),
            super::super::HistoryUpdate::Keep,
        )?;
        c.db_for_tests()
            .execute("UPDATE photos SET recipe='{' WHERE id=?", [unreadable.0.0])?;
        assert!(resolve_photo(&c, unreadable).is_err());

        // Settings cut off mid-value.
        set_lightroom(&c, lightroom.0, "s = { Exposure2012 = ")?;
        let error = resolve_photo(&c, lightroom).unwrap_err();
        assert!(error.contains("Lightroom edit can't be read"), "{error}");
        Ok(())
    }
}
