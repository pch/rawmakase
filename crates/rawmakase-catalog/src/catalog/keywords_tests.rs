use super::*;
use crate::catalog::PhotoId;

/// A catalog with `n` photos added from a folder, and their ids.
fn catalog(n: usize) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for i in 0..n {
        std::fs::write(
            folder.join(format!("image{i}.ARW")),
            format!("synthetic {i}"),
        )?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let ids = cat.photos()?.iter().map(|p| p.id).collect();
    Ok((dir, cat, ids))
}

fn names(cat: &Catalog) -> Result<Vec<String>> {
    Ok(cat.keyword_list()?.into_iter().map(|k| k.name).collect())
}

#[test]
fn the_list_leaves_lightroom_s_root_out_and_counts_photos() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(2)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (1, '', NULL), (2, 'Places', 1), (3, 'Paris', 2);",
    )?;
    cat.add_keyword(&ids, 3)?;
    let list = cat.keyword_list()?;
    assert_eq!(
        list,
        [
            ListedKeyword {
                id: 2,
                name: "Places".into(),
                parent: None,
                photos: 0
            },
            ListedKeyword {
                id: 3,
                name: "Paris".into(),
                parent: Some(2),
                photos: 2
            },
        ]
    );
    Ok(())
}

#[test]
fn a_name_its_parent_has_is_refused_ignoring_case() -> Result<()> {
    let (_dir, mut cat, _) = catalog(0)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (1, '', NULL), (2, 'Places', 1), (3, 'Paris', 2), (4, 'Rome', 2);",
    )?;
    // At the top level, beside or under the root alike.
    assert!(cat.create_keyword("places", None).is_err());
    assert!(cat.rename_keyword(4, "PARIS").is_err());
    // Its own name in another case, or a name used elsewhere, is fine.
    cat.rename_keyword(3, "PARIS")?;
    let (made, _) = cat.create_keyword("Rome", None)?;
    assert_eq!(
        cat.keyword_list()?
            .iter()
            .find(|k| k.id == made)
            .unwrap()
            .parent,
        None
    );
    // Made under the root, as Lightroom's are.
    let parent: Option<i64> =
        cat.db_for_tests()
            .query_row("SELECT parent FROM keywords WHERE id=?", [made], |r| {
                r.get(0)
            })?;
    assert_eq!(parent, Some(1));
    assert!(cat.rename_keyword(1, "Root").is_err());
    // Names that would read as a hierarchy when typed.
    assert!(cat.create_keyword("A > B", None).is_err());
    assert!(cat.rename_keyword(4, "Rome < Italy").is_err());
    assert!(cat.delete_keywords(&[1]).is_err());
    Ok(())
}

#[test]
fn deleting_takes_the_keywords_inside_and_undo_puts_back_everything() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(2)?;
    let (places, _) = cat.create_keyword("Places", None)?;
    let (paris, _) = cat.create_keyword("Paris", Some(places))?;
    let (louvre, _) = cat.create_keyword("Louvre", Some(paris))?;
    cat.db_for_tests().execute_batch(&format!(
        "INSERT INTO keyword_export(keyword, include, parents) VALUES ({paris}, 0, 1);"
    ))?;
    cat.add_keyword(&ids, louvre)?;
    cat.add_keyword(&ids[..1], paris)?;
    let change = cat.delete_keywords(&[paris])?;
    assert_eq!(names(&cat)?, ["Places"]);
    assert!(cat.keywords(ids[0])?.is_empty());
    // A keyword made since never takes a deleted one's id.
    let (other, made) = cat.create_keyword("Other", None)?;
    assert!(other > louvre);
    cat.restore_keywords(&made.before)?;
    cat.restore_keywords(&change.before)?;
    assert_eq!(names(&cat)?, ["Places", "Paris", "Louvre"]);
    let first: Vec<i64> = cat.keywords(ids[0])?.into_iter().map(|k| k.id).collect();
    assert_eq!(first, [paris, louvre]);
    assert_eq!(cat.keywords(ids[1])?.len(), 1);
    // Its export options come back with it.
    assert_eq!(cat.keywords(ids[0])?[0].exported, [false, false]);
    // Redo.
    cat.restore_keywords(&change.after)?;
    assert_eq!(names(&cat)?, ["Places"]);
    Ok(())
}

#[test]
fn a_keyword_with_lightroom_synonyms_is_deleted_and_undo_puts_them_back() -> Result<()> {
    let (_dir, mut cat, _) = catalog(0)?;
    let (car, _) = cat.create_keyword("Car", None)?;
    cat.db_for_tests().execute_batch(&format!(
        "INSERT INTO keyword_synonyms(keyword, name) VALUES ({car}, 'Auto'), ({car}, 'Automobile');"
    ))?;
    let change = cat.delete_keywords(&[car])?;
    assert!(cat.keyword_synonyms()?.is_empty());
    cat.restore_keywords(&change.before)?;
    assert_eq!(cat.keyword_synonyms()?[&car], ["Auto", "Automobile"]);
    Ok(())
}

#[test]
fn ids_are_never_given_again_after_the_newest_is_deleted() -> Result<()> {
    let (_dir, mut cat, _) = catalog(0)?;
    let (a, _) = cat.create_keyword("A", None)?;
    cat.delete_keywords(&[a])?;
    let (b, _) = cat.create_keyword("B", None)?;
    assert!(b > a);
    // Typed keywords too.
    cat.delete_keywords(&[b])?;
    let c = cat.keyword_at(&["C".into()])?;
    assert!(c > b);
    Ok(())
}

#[test]
fn unused_keywords_are_those_no_photo_has_inside_them_either() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let (places, _) = cat.create_keyword("Places", None)?;
    let (paris, _) = cat.create_keyword("Paris", Some(places))?;
    let (rome, _) = cat.create_keyword("Rome", Some(places))?;
    let (empty, _) = cat.create_keyword("Empty", None)?;
    cat.add_keyword(&ids, paris)?;
    let mut unused = cat.unused_keywords()?;
    unused.sort();
    assert_eq!(unused, [rome, empty]);
    Ok(())
}

#[test]
fn a_removed_photo_is_left_out_of_a_restore() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let (k, _) = cat.create_keyword("K", None)?;
    cat.add_keyword(&ids, k)?;
    let change = cat.delete_keywords(&[k])?;
    cat.db_for_tests()
        .execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM photos; PRAGMA foreign_keys=ON;")?;
    cat.restore_keywords(&change.before)?;
    assert_eq!(names(&cat)?, ["K"]);
    Ok(())
}

fn path(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// The keyword photo `id` was given last, by name.
fn added(cat: &mut Catalog, ids: &[PhotoId], names: &[&str]) -> Result<i64> {
    cat.add_keywords(ids, &[path(names)])?;
    let found = cat.find_typed_keyword(&path(names))?;
    Ok(found.expect("the keyword just added"))
}

#[test]
fn a_typed_name_is_found_at_any_level_ignoring_case() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    // As a Lightroom catalog has them: under its hidden root.
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (1, '', NULL), (2, 'Places', 1), (3, 'Paris', 2);",
    )?;
    assert_eq!(added(&mut cat, &ids, &["paris"])?, 3);
    assert_eq!(names(&cat)?, ["Places", "Paris"]);
    assert_eq!(cat.keywords(ids[0])?[0].name, "Paris");
    // A new one keeps its case and goes under the root.
    let new = added(&mut cat, &ids, &["Rome"])?;
    let parent: Option<i64> =
        cat.db_for_tests()
            .query_row("SELECT parent FROM keywords WHERE id=?", [new], |r| {
                r.get(0)
            })?;
    assert_eq!(parent, Some(1));
    Ok(())
}

#[test]
fn among_several_the_same_case_wins_then_the_top_level_then_the_first() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES
         (1, 'A', NULL), (2, 'city', 1), (3, 'B', NULL), (4, 'City', 3), (5, 'CITY', NULL),
         (6, 'Old', 1), (7, 'Town', 6), (8, 'Old', 3), (9, 'TOWN', 3);",
    )?;
    assert_eq!(added(&mut cat, &ids, &["City"])?, 4);
    assert_eq!(added(&mut cat, &ids, &["cITy"])?, 5);
    assert_eq!(added(&mut cat, &ids, &["town"])?, 7);
    Ok(())
}

#[test]
fn a_typed_path_finds_its_top_anywhere_and_the_rest_under_it() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES
         (1, 'World', NULL), (2, 'France', 1), (3, 'Paris', NULL);",
    )?;
    // France is found under World; its Paris is made, not the top-level one taken.
    let paris = added(&mut cat, &ids, &["france", "Paris"])?;
    assert!(paris > 3);
    let parent: Option<i64> =
        cat.db_for_tests()
            .query_row("SELECT parent FROM keywords WHERE id=?", [paris], |r| {
                r.get(0)
            })?;
    assert_eq!(parent, Some(2));
    assert_eq!(added(&mut cat, &ids, &["France", "PARIS"])?, paris);
    // A missing top is made at the top level, with everything below it.
    let new = added(&mut cat, &ids, &["Spain", "Madrid"])?;
    assert!(cat.find_typed_keyword(&path(&["Spain"]))?.is_some());
    assert!(new > paris);
    assert_eq!(cat.find_typed_keyword(&path(&["Nowhere"]))?, None);
    Ok(())
}

#[test]
fn recent_keywords_are_the_latest_given_to_a_photo() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(2)?;
    let a = added(&mut cat, &ids[..1], &["A"])?;
    let b = added(&mut cat, &ids[..1], &["B"])?;
    let (c, _) = cat.create_keyword("C", None)?;
    assert_eq!(cat.recent_keywords()?, [b, a, c]);
    // Given again to a photo without it: the latest.
    cat.add_keyword(&ids[1..], a)?;
    assert_eq!(cat.recent_keywords()?[0], a);
    // Given to photos that all have it: no change.
    cat.add_keyword(&ids, b)?;
    assert_eq!(cat.recent_keywords()?[0], b);
    cat.add_keyword(&ids, a)?;
    assert_eq!(cat.recent_keywords()?[0], b);
    cat.delete_keywords(&[b])?;
    assert_eq!(cat.recent_keywords()?, [a, c]);
    Ok(())
}

#[test]
fn a_catalog_from_before_never_gives_a_deleted_keyword_s_id_again() -> Result<()> {
    // As an earlier release or a Lightroom import leaves it: no counter.
    let (_dir, mut cat, ids) = catalog(1)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (1, 'Kept', NULL), (2, 'Imported', NULL);",
    )?;
    cat.add_keyword(&ids, 2)?;
    let deleted = cat.delete_keywords(&[2])?;
    let new = added(&mut cat, &ids, &["New"])?;
    assert!(new > 2);
    cat.restore_keywords(&deleted.before)?;
    assert_eq!(names(&cat)?, ["Kept", "Imported", "New"]);
    Ok(())
}

#[test]
fn undoing_a_rename_or_deletion_keeps_a_keyword_s_place_in_recent_keywords() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let a = added(&mut cat, &ids, &["A"])?;
    let b = added(&mut cat, &ids, &["B"])?;
    assert_eq!(cat.recent_keywords()?, [b, a]);
    let renamed = cat.rename_keyword(b, "Bee")?;
    cat.restore_keywords(&renamed.before)?;
    assert_eq!(cat.recent_keywords()?, [b, a]);
    let deleted = cat.delete_keywords(&[b])?;
    cat.restore_keywords(&deleted.before)?;
    assert_eq!(cat.recent_keywords()?, [b, a]);
    Ok(())
}

#[test]
fn keywords_are_named_by_their_paths() -> Result<()> {
    let list = [
        ListedKeyword {
            id: 1,
            name: "France".into(),
            parent: None,
            photos: 0,
        },
        ListedKeyword {
            id: 2,
            name: "Paris".into(),
            parent: Some(1),
            photos: 0,
        },
        ListedKeyword {
            id: 3,
            name: "Texas".into(),
            parent: None,
            photos: 0,
        },
        ListedKeyword {
            id: 4,
            name: "paris".into(),
            parent: Some(3),
            photos: 0,
        },
        ListedKeyword {
            id: 5,
            name: "Rome".into(),
            parent: None,
            photos: 0,
        },
        ListedKeyword {
            id: 6,
            name: "Paris".into(),
            parent: None,
            photos: 0,
        },
    ];
    let labels = keyword_labels(&list);
    assert_eq!(labels[&2], "Paris < France");
    assert_eq!(labels[&4], "paris < Texas");
    assert_eq!(labels[&6], "Paris");
    assert_eq!(labels[&5], "Rome");
    assert_eq!(labels[&1], "France");
    assert_eq!(labels_of(&[5, 2], &labels), ["Paris < France", "Rome"]);
    Ok(())
}
