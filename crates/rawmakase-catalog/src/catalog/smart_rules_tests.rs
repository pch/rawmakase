//! Issue 998: smart collections' rules and keyword synonyms,
//! stored, imported from Lightroom, restored by Undo and kept by Export as
//! Catalog.
use super::lightroom::import_lightroom;
use super::*;
use rawmakase_model::collection_rules::{Combine, Group, Item, Rule, lua::Value};
use rusqlite::Connection;
use std::collections::HashMap;

/// Lightroom Classic 15.6's text for its Five Stars collection.
const FIVE_STARS: &str = "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\t\tvalue = 5,\n\t},\n\tcombine = \"intersect\",\n}\n";

/// A Lightroom catalog with a smart collection in a set, a static one with
/// its sort, a keyword with a synonym and a target collection.
fn lightroom(path: &Path, target: &str) -> Result<()> {
    super::tests::fixture(path)?;
    let db = Connection::open(path)?;
    db.execute_batch(&format!(
        "CREATE TABLE AgLibraryCollectionContent(id_local INTEGER PRIMARY KEY,collection INTEGER NOT NULL DEFAULT 0,content,owningModule);
         CREATE TABLE AgLibraryKeywordSynonym(id_local INTEGER PRIMARY KEY,keyword INTEGER NOT NULL DEFAULT 0,lc_name,name);
         CREATE TABLE Adobe_variablesTable(id_local INTEGER PRIMARY KEY,id_global,name,type,value);
         INSERT INTO AgLibraryCollection VALUES(11,'Smart Collections',NULL,'com.adobe.ag.library.group'),
             (17,'Five Stars',11,'com.adobe.ag.library.smart_collection'),
             (18,'Liked',11,'com.adobe.ag.library.smart_collection'),
             (19,'Broken',NULL,'com.adobe.ag.library.smart_collection');
         INSERT INTO AgLibraryCollectionContent(collection,content,owningModule) VALUES
             (17,'{five}','ag.library.smart_collection'),
             (17,'captureTime','com.adobe.ag.library.sortType'),
             (17,'ascending','com.adobe.ag.library.sortDirection'),
             (18,'s = {{ {{ criteria = \"likes\", operation = \"isTrue\", value = true, }}, combine = \"intersect\", }}','ag.library.smart_collection'),
             (19,'s = {{ os.exit() }}','ag.library.smart_collection'),
             (50,'positionInCollection','com.adobe.ag.library.sortType'),
             (50,'descending','com.adobe.ag.library.sortDirection'),
             (11,'name','com.adobe.ag.library.sortType');
         INSERT INTO AgLibraryKeywordSynonym(keyword,lc_name,name) VALUES(60,'metropolis','Metropolis'),(60,'',''),(999,'x','x');
         INSERT INTO Adobe_variablesTable(name,value) VALUES('targetCollection',{target}),('targetCollectionTable','AgLibraryCollection');",
        five = FIVE_STARS.replace('\'', "''"),
    ))?;
    Ok(())
}

#[test]
fn lightroom_smart_collections_sort_target_and_synonyms_are_imported() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("smart.lrcat");
    lightroom(&source, "50.0")?;
    let destination = d.path().join("smart.rawmakase");
    import_lightroom(&source, &destination)?;
    let cat = Catalog::open(&destination)?;
    let rules = cat.smart_rules()?;
    let five = &rules[&CollectionId(17)];
    assert_eq!(five.lightroom(), Some(FIVE_STARS));
    assert_eq!(five.rules().unwrap().to_lightroom(), FIVE_STARS);
    // Read but unsupported: kept, evaluated as no photos by the session.
    let liked = rules[&CollectionId(18)].rules().unwrap();
    assert_eq!(liked.unsupported().unwrap().criteria, "likes");
    // Not readable: its text is kept verbatim, with why.
    let broken = &rules[&CollectionId(19)];
    assert_eq!(broken.lightroom(), Some("s = { os.exit() }"));
    assert!(broken.rules().is_err());
    let options = cat.collection_options()?;
    assert_eq!(options[&CollectionId(17)].sort, "captureTime");
    assert_eq!(options[&CollectionId(50)].sort, "positionInCollection");
    assert_eq!(options[&CollectionId(50)].direction, "descending");
    // A set has no sort.
    assert!(!options.contains_key(&CollectionId(11)));
    assert_eq!(cat.target_collection()?, Some(CollectionId(50)));
    assert_eq!(
        cat.keyword_synonyms()?,
        HashMap::from([(60, vec!["Metropolis".to_string()])])
    );
    Ok(())
}

#[test]
fn lightroom_quick_collection_target_is_imported() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("quick.lrcat");
    lightroom(&source, "'qc'")?;
    let destination = d.path().join("quick.rawmakase");
    import_lightroom(&source, &destination)?;
    let cat = Catalog::open(&destination)?;
    assert!(cat.has_target_collection()?);
    assert_eq!(cat.target_collection()?, None);
    Ok(())
}

fn catalog_with_photo() -> Result<(tempfile::TempDir, Catalog, PhotoId)> {
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(4, 4).save(folder.join("a.jpg"))?;
    let mut cat = Catalog::create(&d.path().join("c.rawmakase"))?;
    cat.add_folder(&folder)?;
    let id = cat.photos()?[0].id;
    Ok((d, cat, id))
}

/// A smart collection `name` in set `parent`, made as an import makes one.
fn smart(cat: &mut Catalog, id: i64, name: &str, parent: Option<i64>) -> Result<CollectionId> {
    cat.db_for_tests().execute(
        "INSERT INTO collections(id, name, parent, kind) VALUES (?, ?, ?, 'com.adobe.ag.library.smart_collection')",
        rusqlite::params![id, name, parent],
    )?;
    Ok(CollectionId(id))
}

#[test]
fn rules_are_kept_for_smart_collections_only_and_survive_undo_of_a_delete() -> Result<()> {
    let (_d, mut cat, _) = catalog_with_photo()?;
    let set = cat.create_collection(CollectionKind::Set, "Smart Collections", None)?;
    let id = smart(&mut cat, 100, "Rated", Some(set.0))?;
    let group = Group::new(
        Combine::Union,
        vec![Item::Rule(Rule::new(
            "rating",
            ">=",
            Some(Value::Number(4.)),
        ))],
    );
    cat.set_smart_rules(id, &SmartRules::new(&group))?;
    assert_eq!(cat.smart_rules()?[&id].rules().unwrap(), group);
    assert_eq!(cat.smart_rules()?[&id].lightroom(), None);
    let plain = cat.create_collection(CollectionKind::Collection, "Plain", None)?;
    assert!(
        cat.set_smart_rules(plain, &SmartRules::new(&group))
            .is_err()
    );
    assert!(cat.set_smart_rules(set, &SmartRules::new(&group)).is_err());
    let removed = cat.remove_collections(&[set])?;
    assert!(cat.smart_rules()?.is_empty());
    cat.restore_collections(&removed)?;
    assert_eq!(cat.smart_rules()?[&id].rules().unwrap(), group);
    Ok(())
}

#[test]
fn rules_of_a_later_release_are_kept_unread() -> Result<()> {
    let (_d, mut cat, _) = catalog_with_photo()?;
    let id = smart(&mut cat, 100, "Later", None)?;
    cat.db_for_tests().execute(
        "INSERT INTO smart_collection_rules(collection, rules, format) VALUES (?, '{\"combine\":\"all-new\"}', 9)",
        [id.0],
    )?;
    let rules = &cat.smart_rules()?[&id];
    assert!(rules.rules().unwrap_err().0.contains("later release"));
    Ok(())
}

/// Export as Catalog keeps smart collections, whatever photos go, with their
/// sets and rules, although `sources` is emptied; synonyms stay with their
/// keywords.
#[test]
fn export_as_catalog_keeps_smart_collections() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("smart.lrcat");
    lightroom(&source, "50.0")?;
    let imported = d.path().join("smart.rawmakase");
    import_lightroom(&source, &imported)?;
    let cat = Catalog::open(&imported)?;
    let export = cat.export_catalog(
        &ExportScope::Photos(vec![PhotoId(41)]),
        &d.path().join("export.rawmakase"),
    )?;
    let out = Catalog::open(&export.path)?;
    let names: Vec<String> = out.collections()?.into_iter().map(|c| c.name).collect();
    for kept in [
        "Smart Collections",
        "Five Stars",
        "Liked",
        "Broken",
        "Travel",
    ] {
        assert!(names.contains(&kept.to_string()), "{kept}: {names:?}");
    }
    assert_eq!(out.smart_rules()?.len(), 3);
    assert_eq!(
        out.smart_rules()?[&CollectionId(17)].lightroom(),
        Some(FIVE_STARS)
    );
    assert_eq!(out.keyword_synonyms()?.len(), 1);
    Ok(())
}

/// A keyword no exported photo has goes, with its synonyms.
#[test]
fn export_as_catalog_prunes_synonyms_with_their_keywords() -> Result<()> {
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.jpg", "b.jpg"] {
        image::RgbImage::new(4, 4).save(folder.join(name))?;
    }
    let mut cat = Catalog::create(&d.path().join("c.rawmakase"))?;
    cat.add_folder(&folder)?;
    let photos = cat.photos()?;
    cat.add_keywords(&[photos[0].id], &[vec!["City".into()]])?;
    cat.db_for_tests().execute(
        "INSERT INTO keyword_synonyms(keyword, name) SELECT id, 'Metropolis' FROM keywords",
        [],
    )?;
    let export = cat.export_catalog(
        &ExportScope::Photos(vec![photos[1].id]),
        &d.path().join("export.rawmakase"),
    )?;
    let out = Catalog::open(&export.path)?;
    assert!(out.keyword_synonyms()?.is_empty());
    Ok(())
}

/// A recipe with a camera profile is stored with the profile as a
/// reference: Cropped and Treatment still read it.
#[test]
fn edit_facts_read_recipes_that_name_their_profile() -> Result<()> {
    let (_d, mut cat, photo) = catalog_with_photo()?;
    let path = cat.photos()?[0].path.clone();
    let profile = std::sync::Arc::new(crate::camera_profiles::CameraProfile::embedded(
        &crate::camera_data::Metadata {
            source: crate::camera_data::Source::Rendered,
            make: "Test".into(),
            model: "Camera".into(),
            wb: [2., 1., 1.5],
            daylight_wb: [2., 1., 1.5],
            ..Default::default()
        },
    ));
    let mut recipe = crate::model::recipe::Recipe {
        profile: Some(profile),
        crop: [0.1, 0.1, 0.9, 0.9],
        ..Default::default()
    };
    recipe.effects.monochrome = true;
    cat.save_edit(
        photo,
        &path,
        &crate::storage::Identity::read(&path)?,
        &recipe,
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    let stored: String =
        cat.db_for_tests()
            .query_row("SELECT recipe FROM photos WHERE id = ?", [photo.0], |r| {
                r.get(0)
            })?;
    assert!(stored.contains("\"profile\":\"sha256:"), "{stored}");
    crate::storage::recipe_assets::clear_cache();
    let facts = cat.edit_facts()?[&photo];
    assert!(
        facts.adjusted && facts.cropped && facts.grayscale,
        "{facts:?}"
    );
    Ok(())
}

/// A photo with only a Lightroom edit counts as Black & White as it renders: by its
/// look's own `ConvertToGrayscale` when the look carries its settings, else by the
/// edit's (#666).
#[test]
fn edit_facts_take_a_lightroom_looks_treatment() -> Result<()> {
    let (_d, cat, photo) = catalog_with_photo()?;
    let facts = |text: &str| -> Result<bool> {
        cat.db_for_tests().execute(
            "UPDATE photos SET recipe = NULL, lightroom_develop = ?1 WHERE id = ?2",
            rusqlite::params![text, photo.0],
        )?;
        Ok(cat.edit_facts()?[&photo].grayscale)
    };
    let look = |photo: &str, parameters: &str| {
        format!(
            r#"s = {{ ConvertToGrayscale = {photo}, Look = {{ Name = "Adobe Color", Amount = 1{parameters} }} }}"#
        )
    };
    assert!(!facts(&look(
        "true",
        ", Parameters = { ConvertToGrayscale = false }"
    ))?);
    assert!(facts(&look(
        "false",
        ", Parameters = { ConvertToGrayscale = true }"
    ))?);
    assert!(!facts(&look(
        "true",
        ", Parameters = { ProcessVersion = \"11.0\" }"
    ))?);
    // Without the look's settings, or with ones that can't be read, the edit's flag.
    assert!(facts(&look("true", ""))?);
    assert!(facts(&look("true", ", Parameters = 3"))?);
    assert!(facts("s = { ConvertToGrayscale = true, Look = {  } }")?);
    assert!(!facts("s = { ConvertToGrayscale = false }")?);
    Ok(())
}

/// An edit with a look whose Lightroom settings are known stores them once beside its
/// profile, named from the edit, and reads back with its look (#666).
#[test]
fn an_edits_look_settings_are_stored_beside_its_profile() -> Result<()> {
    use crate::camera_profiles::look_source::{self, testing};
    let (_d, mut cat, photo) = catalog_with_photo()?;
    let path = cat.photos()?[0].path.clone();
    let base = crate::camera_profiles::CameraProfile::camera_matrix_default(
        &crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        },
    )
    .unwrap();
    let look = std::sync::Arc::new(testing::look_profile(
        &testing::look_xmp(
            "Catalog look",
            "66600000000000000000000000000030",
            "Someone",
            "128, 150",
        ),
        &base,
    ));
    let source = look_source::find(&look).unwrap();
    let recipe = crate::model::recipe::Recipe {
        profile: Some(look),
        ..Default::default()
    };
    cat.save_edit(
        photo,
        &path,
        &crate::storage::Identity::read(&path)?,
        &recipe,
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    let db = cat.db_for_tests();
    let stored: String =
        db.query_row("SELECT recipe FROM photos WHERE id = ?", [photo.0], |r| {
            r.get(0)
        })?;
    let id = crate::storage::recipe_assets::Asset::look_source(&source).id;
    assert!(
        stored.contains(&format!("\"look_source\":\"{id}\"")),
        "{stored}"
    );
    let kind: String = db.query_row("SELECT kind FROM recipe_assets WHERE id = ?", [&id], |r| {
        r.get(0)
    })?;
    assert_eq!(kind, "look_source");
    crate::storage::recipe_assets::clear_cache();
    let read = cat.load_edit(photo, &path)?.unwrap().recipe;
    assert_eq!(read, recipe);
    assert!(std::sync::Arc::ptr_eq(
        &look_source::find(read.profile.as_ref().unwrap()).unwrap(),
        &source
    ));
    Ok(())
}

/// New Catalog's defaults, as Lightroom Classic 15.6 makes them: six smart
/// collections in a "Smart Collections" set, with Lightroom's text.
#[test]
fn new_catalogs_get_lightroom_default_smart_collections() -> Result<()> {
    let (_d, mut cat, _) = catalog_with_photo()?;
    cat.add_default_smart_collections()?;
    let collections = cat.collections()?;
    let set = collections
        .iter()
        .find(|c| c.name == "Smart Collections" && c.kind == CollectionKind::Set)
        .unwrap();
    let smart: Vec<&Collection> = collections
        .iter()
        .filter(|c| c.kind == CollectionKind::Smart)
        .collect();
    let names: Vec<&str> = smart.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Colored Red",
            "Five Stars",
            "Past Month",
            "Recently Modified",
            "Video Files",
            "Without Keywords"
        ]
    );
    let rules = cat.smart_rules()?;
    for (c, (_, text)) in smart
        .iter()
        .zip(rawmakase_model::collection_rules::LIGHTROOM_DEFAULTS)
    {
        assert_eq!(c.parent, Some(set.id));
        assert_eq!(rules[&c.id].lightroom(), Some(text));
        assert_eq!(
            rules[&c.id].rules().unwrap().unsupported(),
            None,
            "{}",
            c.name
        );
    }
    Ok(())
}

#[test]
fn smart_collections_are_created_with_their_rules_by_the_naming_rules() -> Result<()> {
    let (_d, mut cat, _) = catalog_with_photo()?;
    let set = cat.create_collection(CollectionKind::Set, "Trips", None)?;
    let group = Group::new(
        Combine::Intersect,
        vec![Item::Rule(Rule::new(
            "rating",
            ">=",
            Some(Value::Number(3.)),
        ))],
    );
    let id = cat.create_smart_collection("  Good  ", Some(set), &SmartRules::new(&group))?;
    let made = cat.collections()?.into_iter().find(|c| c.id == id).unwrap();
    assert_eq!(
        (made.name.as_str(), made.parent, made.kind),
        ("Good", Some(set), CollectionKind::Smart)
    );
    assert_eq!(cat.smart_rules()?[&id].rules().unwrap(), group);
    for (name, parent) in [("Good", Some(set)), ("", None), (QUICK_COLLECTION, None)] {
        assert!(
            cat.create_smart_collection(name, parent, &SmartRules::new(&group))
                .is_err(),
            "{name}"
        );
    }
    let plain = cat.create_collection(CollectionKind::Collection, "Plain", None)?;
    assert!(
        cat.create_smart_collection("Inside", Some(plain), &SmartRules::new(&group))
            .is_err()
    );
    Ok(())
}
