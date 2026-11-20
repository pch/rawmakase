use super::*;
use anyhow::Context as _;

/// A catalog of three photos and their ids, lowest first.
fn catalog() -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.ARW", "b.ARW", "c.ARW"] {
        std::fs::write(folder.join(name), name)?;
    }
    let mut catalog = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    catalog.add_folder(&folder)?;
    // By id, the order members are listed in: `photos` lists them by
    // name or capture time, which differ between file systems.
    let mut ids: Vec<PhotoId> = catalog.photos()?.iter().map(|p| p.id).collect();
    ids.sort();
    Ok((dir, catalog, ids))
}

fn row(c: &Catalog, id: CollectionId) -> Result<Collection> {
    c.collections()?
        .into_iter()
        .find(|c| c.id == id)
        .context("collection")
}

fn members(c: &Catalog, id: CollectionId) -> Result<Vec<(PhotoId, Option<String>)>> {
    Ok(c.collection_members(id)?
        .into_iter()
        .map(|(p, position)| (p, position.map(|p| p.as_str().to_string())))
        .collect())
}

/// `CollectionKind::from_lightroom` as releases before #998 have it
/// (a2a9123dc): what an older release takes each row for.
mod old_release {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Kind {
        Set,
        Collection,
        Smart,
        System,
    }
    pub(super) fn from_lightroom(creation_id: &str, name: &str) -> Kind {
        match creation_id {
            "com.adobe.ag.library.group" => Kind::Set,
            "com.adobe.ag.library.smart_collection" => Kind::Smart,
            "com.adobe.ag.library.collection" if name == "quick collection" => Kind::System,
            id if id.ends_with(".unsaved") => Kind::System,
            _ => Kind::Collection,
        }
    }
}

/// A collection row as an older release lists it: id, name, parent, kind.
type OldRow = (i64, String, Option<i64>, old_release::Kind);

/// Every collection row as an older release lists it, with the queries it
/// reads them with.
fn as_older_release_reads(c: &Catalog) -> Result<Vec<OldRow>> {
    let db = c.db_for_tests();
    let mut statement =
        db.prepare("SELECT id, name, parent, kind FROM collections ORDER BY name")?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, name, parent, kind)| {
            let kind = old_release::from_lightroom(&kind, &name);
            (id, name, parent, kind)
        })
        .collect())
}

#[test]
fn collections_and_sets_are_created_renamed_and_moved_by_lightroom_name_rules() -> Result<()> {
    let (_dir, mut c, _) = catalog()?;
    let trips = c.create_collection(CollectionKind::Set, "Trips", None)?;
    let japan = c.create_collection(CollectionKind::Collection, "  Japan ", Some(trips))?;
    assert_eq!(row(&c, japan)?.name, "Japan");
    assert_eq!(row(&c, japan)?.parent, Some(trips));
    assert_eq!(row(&c, japan)?.kind, CollectionKind::Collection);
    assert_eq!(row(&c, trips)?.kind, CollectionKind::Set);

    // Unique among one parent's children, exactly as typed (Lightroom's
    // "That name is already in use."); the same name elsewhere is fine.
    let taken = c.create_collection(CollectionKind::Collection, "Japan", Some(trips));
    assert_eq!(
        taken.unwrap_err().to_string(),
        "That name is already in use."
    );
    let top = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    c.create_collection(CollectionKind::Collection, "japan", Some(trips))?;
    assert!(
        c.create_collection(CollectionKind::Set, "Trips", None)
            .is_err()
    );
    assert!(
        c.create_collection(CollectionKind::Collection, "   ", None)
            .is_err()
    );
    assert!(
        c.create_collection(CollectionKind::Smart, "Five", None)
            .is_err()
    );
    // Only a set holds collections.
    assert!(
        c.create_collection(CollectionKind::Collection, "In", Some(top))
            .is_err()
    );
    assert!(
        c.create_collection(CollectionKind::Collection, "In", Some(CollectionId(999)))
            .is_err()
    );

    // Moving where a sibling has the name is refused.
    assert!(c.move_collection(japan, None).is_err());
    assert!(c.rename_collection(top, "Trips").is_err());
    c.rename_collection(top, " Japan ")?;
    c.rename_collection(top, "Kyoto")?;
    assert_eq!(row(&c, top)?.name, "Kyoto");
    c.move_collection(top, Some(trips))?;
    assert_eq!(row(&c, top)?.parent, Some(trips));
    c.move_collection(top, None)?;
    assert_eq!(row(&c, top)?.parent, None);
    Ok(())
}

#[test]
fn a_set_cannot_move_into_itself_or_a_set_inside_it() -> Result<()> {
    let (_dir, mut c, _) = catalog()?;
    let outer = c.create_collection(CollectionKind::Set, "Outer", None)?;
    let middle = c.create_collection(CollectionKind::Set, "Middle", Some(outer))?;
    let inner = c.create_collection(CollectionKind::Set, "Inner", Some(middle))?;
    for parent in [outer, middle, inner] {
        assert!(c.move_collection(outer, Some(parent)).is_err());
    }
    assert!(c.move_collection(middle, Some(inner)).is_err());
    assert_eq!(row(&c, outer)?.parent, None);
    assert_eq!(row(&c, middle)?.parent, Some(outer));
    c.move_collection(inner, None)?;
    c.move_collection(outer, Some(inner))?;
    assert_eq!(row(&c, outer)?.parent, Some(inner));
    Ok(())
}

#[test]
fn no_row_gets_the_quick_collections_name_by_an_older_releases_rule() -> Result<()> {
    let (dir, mut c, ids) = catalog()?;
    let quick = c.quick_collection()?;
    let set = c.create_collection(CollectionKind::Set, "Set", None)?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", Some(set))?;
    for name in ["quick collection", " quick collection "] {
        assert!(
            c.create_collection(CollectionKind::Collection, name, None)
                .is_err()
        );
        assert!(
            c.create_collection(CollectionKind::Collection, name, Some(set))
                .is_err()
        );
        assert!(
            c.create_collection(CollectionKind::Set, name, None)
                .is_err()
        );
        assert!(c.rename_collection(japan, name).is_err());
    }
    // Other spellings are ordinary names, to older releases too.
    let named = c.create_collection(CollectionKind::Collection, "Quick Collection", None)?;
    // The Quick Collection stays as it is.
    assert!(c.rename_collection(quick, "Mine").is_err());
    assert!(c.move_collection(quick, Some(set)).is_err());
    assert!(c.remove_collections(&[quick]).is_err());
    assert!(c.set_target_collection(Some(set)).is_err());
    c.change_collection(quick, &ids[..1], &[])?;
    // Export as Catalog named like it adds a collection under another name.
    let export = c.export_catalog(
        &ExportScope::Photos(ids),
        &dir.path().join("out/quick collection.rawmakase"),
    )?;
    assert_eq!(export.collection, "quick collection (export)");

    let read = as_older_release_reads(&c)?;
    let system: Vec<_> = read
        .iter()
        .filter(|r| r.3 == old_release::Kind::System)
        .collect();
    assert_eq!(system.len(), 1);
    assert_eq!((system[0].0, system[0].2), (quick.0, None));
    let kind = |id: CollectionId| read.iter().find(|r| r.0 == id.0).map(|r| r.3);
    assert_eq!(kind(japan), Some(old_release::Kind::Collection));
    assert_eq!(kind(named), Some(old_release::Kind::Collection));
    assert_eq!(kind(set), Some(old_release::Kind::Set));
    let exported = Catalog::open(dir.path().join("out/quick collection.rawmakase"))?;
    let read = as_older_release_reads(&exported)?;
    assert_eq!(
        read.iter()
            .filter(|r| r.3 == old_release::Kind::System)
            .count(),
        1
    );
    Ok(())
}

#[test]
fn an_older_release_reads_a_catalog_written_by_this_one() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let set = c.create_collection(CollectionKind::Set, "Trips", None)?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", Some(set))?;
    c.change_collection(japan, &ids, &[])?;
    c.set_collection_options(
        japan,
        &CollectionOptions {
            sort: "positionInCollection".into(),
            direction: "ascending".into(),
        },
    )?;
    c.set_target_collection(Some(japan))?;
    let quick = c.quick_collection()?;
    c.change_collection(quick, &ids[..1], &[])?;
    // The queries releases before #998 make.
    let db = c.db_for_tests();
    let found: i64 = db.query_row(
        "SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL",
        ["quick collection", "com.adobe.ag.library.collection"],
        |r| r.get(0),
    )?;
    assert_eq!(found, quick.0);
    let rows: i64 = db.query_row(
        "SELECT count(*) FROM (SELECT collection, photo FROM collection_photos)",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(rows, 4);
    let read = as_older_release_reads(&c)?;
    assert_eq!(read.len(), 3);
    assert!(read.contains(&(
        japan.0,
        "Japan".into(),
        Some(set.0),
        old_release::Kind::Collection
    )));
    Ok(())
}

#[test]
fn photos_go_after_the_last_and_come_back_where_they_were() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    let change = c.change_collection(japan, &ids[..2], &[])?;
    assert_eq!(
        change.added,
        [
            (ids[0], Some(Position::new("1"))),
            (ids[1], Some(Position::new("2")))
        ]
    );
    // Already in it: nothing changes.
    let again = c.change_collection(japan, &ids[..1], &[])?;
    assert_eq!(again, MembershipChange::default());
    // Lightroom's `---N` positions are read by number.
    c.db_for_tests().execute(
        "UPDATE collection_photos SET position='---7' WHERE photo=?",
        [ids[1].0],
    )?;
    c.change_collection(japan, &ids[2..], &[])?;
    assert_eq!(members(&c, japan)?[2], (ids[2], Some("8".into())));

    let removed = c.change_collection(japan, &[], &[ids[1], ids[1]])?;
    assert_eq!(removed.removed, [(ids[1], Some(Position::new("---7")))]);
    assert!(removed.added.is_empty());
    // Undo puts it back at its position.
    c.put_back_members(japan, &removed.removed, &[], &[])?;
    assert_eq!(members(&c, japan)?[1], (ids[1], Some("---7".into())));
    // A photo without a position (as Lightroom's B leaves it) comes back
    // without one.
    c.db_for_tests().execute(
        "UPDATE collection_photos SET position=NULL WHERE photo=?",
        [ids[0].0],
    )?;
    let removed = c.change_collection(japan, &[], &[ids[0]])?;
    assert_eq!(removed.removed, [(ids[0], None)]);
    c.put_back_members(japan, &removed.removed, &[], &[])?;
    assert_eq!(members(&c, japan)?[0], (ids[0], None));
    // Taken out and put back in one change, a photo stays in.
    let both = c.put_back_members(japan, &[(ids[0], Some(Position::new("5")))], &[ids[0]], &[])?;
    assert_eq!(both.removed, [(ids[0], None)]);
    assert_eq!(members(&c, japan)?[0], (ids[0], Some("5".into())));
    Ok(())
}

#[test]
fn positions_order_by_number_lightrooms_too() {
    assert_eq!(Position::new("---3").number(), Some(3.0));
    assert_eq!(Position::new("12").number(), Some(12.0));
    assert_eq!(Position::new("2.5").number(), Some(2.5));
    assert_eq!(Position::new("first").number(), None);
    assert_eq!(Position::new("inf").number(), None);
}

#[test]
fn only_plain_collections_take_photos() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let set = c.create_collection(CollectionKind::Set, "Set", None)?;
    assert!(c.change_collection(set, &ids, &[]).is_err());
    let plain = c.create_collection(CollectionKind::Collection, "Plain", None)?;
    let takes = |c: &Catalog, id| {
        c.collections()
            .unwrap()
            .iter()
            .any(|x| x.id == id && x.takes_photos)
    };
    assert!(takes(&c, plain) && !takes(&c, set));
    for kind in [
        "com.adobe.ag.library.smart_collection",
        "com.adobe.ag.print",
        "com.adobe.ag.slideshow.unsaved",
        "com.example.unknown",
    ] {
        c.db_for_tests().execute(
            "INSERT INTO collections(id, name, parent, kind) VALUES (900, 'Other', NULL, ?)",
            [kind],
        )?;
        let other = CollectionId(900);
        assert!(!takes(&c, other), "{kind}");
        assert!(c.change_collection(other, &ids, &[]).is_err(), "{kind}");
        assert!(c.change_collection(other, &[], &ids).is_err(), "{kind}");
        assert!(c.set_target_collection(Some(other)).is_err(), "{kind}");
        c.db_for_tests()
            .execute("DELETE FROM collections WHERE id=900", [])?;
    }
    assert!(c.change_collection(CollectionId(999), &ids, &[]).is_err());
    assert_eq!(c.collection_photos()?.len(), 0);
    Ok(())
}

#[test]
fn deleting_a_set_takes_everything_in_it_and_undo_restores_it_all() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let trips = c.create_collection(CollectionKind::Set, "Trips", None)?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", Some(trips))?;
    let asia = c.create_collection(CollectionKind::Set, "Asia", Some(trips))?;
    let kyoto = c.create_collection(CollectionKind::Collection, "Kyoto", Some(asia))?;
    let other = c.create_collection(CollectionKind::Collection, "Other", None)?;
    c.change_collection(japan, &ids[..2], &[])?;
    c.put_back_members(kyoto, &[(ids[2], Some(Position::new("---4")))], &[], &[])?;
    c.change_collection(other, &ids, &[])?;
    let options = CollectionOptions {
        sort: "fileName".into(),
        direction: "descending".into(),
    };
    c.set_collection_options(kyoto, &options)?;
    assert!(c.set_collection_options(asia, &options).is_err());
    let before = (
        c.collections()?.len(),
        c.collection_photos()?,
        c.collection_options()?,
    );

    // A collection chosen with the set it is in goes once, with it.
    let removed = c.remove_collections(&[japan, trips])?;
    assert_eq!(
        removed.ids().collect::<Vec<_>>(),
        [trips, japan, asia, kyoto]
    );
    assert_eq!(c.collections()?.len(), 1);
    assert!(c.collection_options()?.is_empty());
    assert_eq!(c.collection_photos()?.keys().collect::<Vec<_>>(), [&other]);
    // The photos stay in the catalog.
    assert_eq!(c.photos()?.len(), 3);

    c.restore_collections(&removed)?;
    let after = (
        c.collections()?.len(),
        c.collection_photos()?,
        c.collection_options()?,
    );
    assert_eq!(after, before);
    assert_eq!(row(&c, kyoto)?.parent, Some(asia));
    assert_eq!(members(&c, kyoto)?, [(ids[2], Some("---4".into()))]);
    // Restoring twice is refused, changing nothing.
    assert!(c.restore_collections(&removed).is_err());
    assert_eq!(c.collections()?.len(), 5);
    Ok(())
}

#[test]
fn restoring_is_refused_where_a_name_was_taken_since() -> Result<()> {
    let (_dir, mut c, _) = catalog()?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    let removed = c.remove_collections(&[japan])?;
    c.create_collection(CollectionKind::Collection, "Japan", None)?;
    assert!(c.restore_collections(&removed).is_err());
    assert_eq!(c.collections()?.len(), 1);
    Ok(())
}

#[test]
fn ids_are_never_given_again_while_undo_could_restore_them() -> Result<()> {
    let (_dir, mut c, _) = catalog()?;
    // Imported from Lightroom with its own ids.
    c.db_for_tests().execute(
        "INSERT INTO collections(id, name, parent, kind)
         VALUES (500, 'Imported', NULL, 'com.adobe.ag.library.collection')",
        [],
    )?;
    let first = c.create_collection(CollectionKind::Collection, "First", None)?;
    assert_eq!(first, CollectionId(501));
    let removed = c.remove_collections(&[first, CollectionId(500)])?;
    let next = c.create_collection(CollectionKind::Collection, "Next", None)?;
    assert_eq!(next, CollectionId(502));
    let quick = c.quick_collection()?;
    assert_eq!(quick, CollectionId(503));
    c.restore_collections(&removed)?;
    assert_eq!(row(&c, first)?.name, "First");
    assert_eq!(row(&c, CollectionId(500))?.name, "Imported");
    // Reopened, the catalog still knows.
    let location = c.location().clone();
    c.remove_collections(&[next])?;
    drop(c);
    let mut c = Catalog::open(location)?;
    assert_eq!(
        c.create_collection(CollectionKind::Collection, "Later", None)?,
        CollectionId(504)
    );
    Ok(())
}

#[test]
fn a_photo_forgotten_from_removed_collections_is_not_restored() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let japan = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    let copy = c.create_virtual_copy(ids[0])?;
    c.change_collection(japan, &[ids[1], copy], &[])?;
    let removed = c.remove_collections(&[japan])?;
    // A photo gone from the catalog is left out.
    c.remove_virtual_copy(copy)?;
    let mut gone = removed;
    c.restore_collections(&gone)?;
    assert_eq!(members(&c, japan)?, [(ids[1], Some("1".into()))]);
    c.remove_collections(&[japan])?;
    // Its id given to another photo: Undo's log forgets it first.
    let reused = c.create_virtual_copy(ids[2])?;
    assert_eq!(reused, copy, "SQLite gives the highest id out again");
    gone.forget_photo(copy);
    assert!(gone.members().all(|(_, p)| p != copy));
    c.restore_collections(&gone)?;
    assert_eq!(members(&c, japan)?, [(ids[1], Some("1".into()))]);
    Ok(())
}

#[test]
fn the_target_collection_is_kept_in_the_catalog_and_falls_back_to_quick() -> Result<()> {
    let (_dir, mut c, _) = catalog()?;
    assert_eq!(c.target_collection()?, None);
    assert!(!c.has_target_collection()?);
    let japan = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    c.set_target_collection(Some(japan))?;
    assert_eq!(c.target_collection()?, Some(japan));
    let quick = c.quick_collection()?;
    c.set_target_collection(Some(quick))?;
    assert_eq!(c.target_collection()?, None);
    assert!(c.has_target_collection()?);
    c.set_target_collection(Some(japan))?;
    // Deleted, it reads as the Quick Collection; restored, it is the target again.
    let removed = c.remove_collections(&[japan])?;
    assert_eq!(c.target_collection()?, None);
    c.restore_collections(&removed)?;
    assert_eq!(c.target_collection()?, Some(japan));
    c.set_target_collection(None)?;
    assert_eq!(c.target_collection()?, None);
    Ok(())
}

#[test]
fn export_as_catalog_keeps_the_options_of_the_collections_it_keeps() -> Result<()> {
    let (dir, mut c, ids) = catalog()?;
    let kept = c.create_collection(CollectionKind::Collection, "Kept", None)?;
    let empty = c.create_collection(CollectionKind::Collection, "Empty", None)?;
    c.change_collection(kept, &ids[..1], &[])?;
    let options = CollectionOptions {
        sort: "positionInCollection".into(),
        direction: "ascending".into(),
    };
    c.set_collection_options(kept, &options)?;
    c.set_collection_options(empty, &options)?;
    // Named like a collection there already, it gets a name of its own.
    let destination = dir.path().join("out/Kept.rawmakase");
    let export = c.export_catalog(&ExportScope::Photos(ids), &destination)?;
    assert_eq!(export.collection, "Kept (export)");
    let out = Catalog::open(&destination)?;
    assert_eq!(
        out.collection_options()?.into_iter().collect::<Vec<_>>(),
        [(kept, options)]
    );
    assert!(out.collections()?.iter().all(|c| c.id != empty));
    // A collection added after the export doesn't take a pruned one's id.
    let mut out = out;
    let added = out.create_collection(CollectionKind::Collection, "New", None)?;
    assert!(added.0 > empty.0);
    Ok(())
}

/// Lightroom Classic 15.6 (#998, pass 4): a copy made from a folder joins
/// only the Quick Collection of its source's collections; one made while a
/// collection its source is in is shown joins that one and the Quick
/// Collection.
#[test]
fn a_new_virtual_copy_joins_the_quick_collection_and_the_collection_shown() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let quick = c.quick_collection()?;
    let s1 = c.create_collection(CollectionKind::Collection, "S1", None)?;
    let s2 = c.create_collection(CollectionKind::Collection, "S2", None)?;
    let set = c.create_collection(CollectionKind::Set, "Set", None)?;
    c.change_collection(s1, &[ids[0], ids[1]], &[])?;
    c.change_collection(s2, &[ids[0], ids[2]], &[])?;
    c.change_collection(quick, &[ids[0]], &[])?;
    c.db_for_tests().execute(
        "UPDATE collection_photos SET position=NULL WHERE collection=?",
        [quick.0],
    )?;
    let in_ = |c: &Catalog, copy: PhotoId| -> Result<Vec<CollectionId>> {
        let mut found: Vec<CollectionId> = c
            .collection_photos()?
            .into_iter()
            .filter(|(_, m)| m.contains(&copy))
            .map(|(id, _)| id)
            .collect();
        found.sort();
        Ok(found)
    };

    let from_folder = c.create_virtual_copy(ids[0])?;
    assert_eq!(in_(&c, from_folder)?, [quick]);
    assert_eq!(members(&c, quick)?, [(ids[0], None), (from_folder, None)]);

    let shown = c.create_virtual_copy_viewing(ids[0], s2)?;
    assert_eq!(in_(&c, shown)?, [quick, s2]);
    // Right after its source: halfway to the next photo's position.
    assert_eq!(members(&c, s2)?.last(), Some(&(shown, Some("1.5".into()))));

    // A photo in no collection gives its copy none, whatever is shown: a
    // collection it isn't in, a set, the Quick Collection.
    for viewing in [s1, set, quick] {
        let copy = c.create_virtual_copy_viewing(ids[2], viewing)?;
        assert!(in_(&c, copy)?.is_empty(), "{viewing:?}");
    }
    let last = c.create_virtual_copy_viewing(ids[1], s1)?;
    assert_eq!(in_(&c, last)?, [s1]);
    assert_eq!(members(&c, s1)?.last(), Some(&(last, Some("3".into()))));
    Ok(())
}

/// An export prunes collections none of its photos are in; the one it adds
/// must not take a pruned one's id, which the target may still name.
#[test]
fn export_as_catalog_never_gives_a_pruned_collections_id_to_its_own() -> Result<()> {
    let (dir, mut c, ids) = catalog()?;
    // Imported from Lightroom, so no id was ever allocated here.
    for (id, name) in [(1, "One"), (2, "Two")] {
        c.db_for_tests().execute(
            "INSERT INTO collections(id, name, parent, kind)
             VALUES (?, ?, NULL, 'com.adobe.ag.library.collection')",
            rusqlite::params![id, name],
        )?;
    }
    c.change_collection(CollectionId(1), &[ids[0]], &[])?;
    c.change_collection(CollectionId(2), &[ids[1]], &[])?;
    c.set_target_collection(Some(CollectionId(2)))?;
    let destination = dir.path().join("out/Shoot.rawmakase");
    c.export_catalog(&ExportScope::Photos(vec![ids[0]]), &destination)?;
    let out = Catalog::open(destination)?;
    let added = out
        .collections()?
        .into_iter()
        .find(|c| c.name == "Shoot")
        .context("collection")?;
    assert!(added.id.0 > 2);
    assert_eq!(out.target_collection()?, None);
    Ok(())
}

/// A collection made with its photos is made with all of them or not at
/// all, so a failed write never leaves an empty one behind.
#[test]
fn a_collection_and_its_photos_are_made_together_or_not_at_all() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let (made, change) = c.create_collection_of(" Picks ", None, &ids[..2])?;
    assert_eq!(row(&c, made)?.name, "Picks");
    assert_eq!(change.added.len(), 2);
    c.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER no_members BEFORE INSERT ON collection_photos
         BEGIN SELECT RAISE(ABORT, 'disk full'); END",
    )?;
    assert!(c.create_collection_of("Failed", None, &ids).is_err());
    assert!(c.collections()?.iter().all(|c| c.name != "Failed"));
    Ok(())
}

/// Save Quick Collection, its Undo and its Redo are each one transaction:
/// a failure of the second write leaves the first undone too.
#[test]
fn save_quick_collection_and_its_undo_and_redo_are_all_or_nothing() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let quick = c.quick_collection()?;
    c.change_collection(quick, &ids[..2], &[])?;
    let fail_clear = "CREATE TEMP TRIGGER fail_clear BEFORE DELETE ON collection_photos
                      BEGIN SELECT RAISE(ABORT, 'disk full'); END";
    c.db_for_tests().execute_batch(fail_clear)?;
    assert!(c.save_quick_collection("Keepers", true).is_err());
    assert!(c.collections()?.iter().all(|c| c.name != "Keepers"));
    c.db_for_tests().execute_batch("DROP TRIGGER fail_clear")?;

    let (id, cleared) = c.save_quick_collection("Keepers", true)?;
    assert_eq!(members(&c, id)?.len(), 2);
    assert!(members(&c, quick)?.is_empty());
    assert_eq!(cleared.removed.len(), 2);
    // Undo: the photos back in the Quick Collection, the collection gone.
    let fail_delete = "CREATE TEMP TRIGGER fail_delete BEFORE DELETE ON collections
                       BEGIN SELECT RAISE(ABORT, 'disk full'); END";
    c.db_for_tests().execute_batch(fail_delete)?;
    assert!(
        c.unsave_quick_collection(id, quick, &cleared.removed)
            .is_err()
    );
    assert!(members(&c, quick)?.is_empty());
    c.db_for_tests().execute_batch("DROP TRIGGER fail_delete")?;
    let removed = c.unsave_quick_collection(id, quick, &cleared.removed)?;
    assert_eq!(members(&c, quick)?.len(), 2);
    assert!(c.collections()?.iter().all(|c| c.id != id));
    // Redo: the collection back and the Quick Collection cleared, or neither.
    let photos: Vec<PhotoId> = cleared.removed.iter().map(|m| m.0).collect();
    c.db_for_tests().execute_batch(fail_clear)?;
    assert!(c.resave_quick_collection(&removed, quick, &photos).is_err());
    assert!(c.collections()?.iter().all(|c| c.id != id));
    assert_eq!(members(&c, quick)?.len(), 2);
    c.db_for_tests().execute_batch("DROP TRIGGER fail_clear")?;
    c.resave_quick_collection(&removed, quick, &photos)?;
    assert_eq!(members(&c, id)?.len(), 2);
    assert!(members(&c, quick)?.is_empty());
    Ok(())
}

/// Custom Order reads Lightroom's `---N` by number, puts photos with no
/// position after the others, and a reorder numbers them 1, 2, 3… with Undo
/// putting every position back as it was.
#[test]
fn custom_order_moves_photos_and_undo_puts_positions_back() -> Result<()> {
    let (dir, mut c, _) = catalog()?;
    // Five photos.
    let folder = dir.path().join("more");
    std::fs::create_dir(&folder)?;
    for name in ["d.ARW", "e.ARW"] {
        std::fs::write(folder.join(name), name)?;
    }
    c.add_folder(&folder)?;
    let mut ids: Vec<PhotoId> = c.photos()?.iter().map(|p| p.id).collect();
    ids.sort();
    let japan = c.create_collection(CollectionKind::Collection, "Japan", None)?;
    c.change_collection(japan, &ids, &[])?;
    // As Lightroom left them: `---N`, and none for one B added.
    for (photo, position) in [
        (0, Some("---3")),
        (1, Some("---1")),
        (2, None),
        (3, Some("---2")),
        (4, Some("---10")),
    ] {
        c.db_for_tests().execute(
            "UPDATE collection_photos SET position=? WHERE photo=?",
            rusqlite::params![position, ids[photo].0],
        )?;
    }
    let before = c.collection_members(japan)?;
    assert_eq!(
        c.collection_order(japan)?,
        [ids[1], ids[3], ids[0], ids[4], ids[2]]
    );
    // Two photos dragged before the first.
    let undo = c.arrange_collection(japan, &[ids[0], ids[2], ids[1]])?;
    assert_eq!(undo, Some(before.clone()));
    assert_eq!(
        c.collection_order(japan)?,
        [ids[0], ids[2], ids[1], ids[3], ids[4]]
    );
    assert_eq!(
        members(&c, japan)?
            .iter()
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        ["1", "3", "2", "4", "5"].map(|p| Some(p.to_string()))
    );
    // Where they are already: nothing written.
    let same = c.collection_members(japan)?;
    assert_eq!(c.arrange_collection(japan, &[ids[0], ids[2]])?, None);
    assert_eq!(c.collection_members(japan)?, same);
    // To the end: the photos left out follow in their order, and photos
    // not in the collection are ignored.
    c.arrange_collection(japan, &[ids[2], ids[1], ids[3], ids[4], PhotoId(999)])?;
    assert_eq!(c.collection_order(japan)?.last(), Some(&ids[0]));
    c.restore_positions(japan, &before)?;
    assert_eq!(c.collection_members(japan)?, before);
    // Only plain collections have a custom order.
    let set = c.create_collection(CollectionKind::Set, "Set", None)?;
    assert!(c.arrange_collection(set, &[]).is_err());
    Ok(())
}

/// Photos added to a collection go after the last in its custom order, even
/// after photos Lightroom left without a position, which come last.
#[test]
fn photos_added_go_after_photos_without_a_position() -> Result<()> {
    let (_dir, mut c, ids) = catalog()?;
    let quick = c.quick_collection()?;
    c.change_collection(quick, &ids[..2], &[])?;
    c.db_for_tests().execute(
        "UPDATE collection_photos SET position=NULL WHERE photo=?",
        [ids[0].0],
    )?;
    assert_eq!(c.collection_order(quick)?, [ids[1], ids[0]]);
    c.change_collection(quick, &ids[2..], &[])?;
    assert_eq!(c.collection_order(quick)?, [ids[1], ids[0], ids[2]]);
    Ok(())
}
