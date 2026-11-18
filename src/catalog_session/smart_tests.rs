//! Issue 998: smart collections' members over a real catalog: per entry,
//! with keyword ancestors and synonyms, folder paths, static collection names
//! and edit facts.
use super::*;
use crate::catalog::{Catalog, CatalogLocation};
use crate::catalog_session::DescriptiveEdit;
use crate::collection_rules::{Combine, Item, Rule, lua::Value};
use crate::metadata::TextField;

fn session() -> Result<(tempfile::TempDir, CatalogSession, Vec<PhotoId>)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("trips").join("Kyoto");
    std::fs::create_dir_all(&folder)?;
    for name in ["a.jpg", "b.jpg"] {
        image::RgbImage::new(8, 8).save(folder.join(name))?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let session = CatalogSession::open(&CatalogLocation::from(&path))?.session;
    let ids = session.photos().iter().map(|p| p.id).collect();
    Ok((directory, session, ids))
}

fn clock() -> Clock {
    Clock {
        today: Date::from_ymd(2026, 11, 17),
        week_start: 0,
    }
}

fn rule(criteria: &str, operation: &str, value: Option<Value>) -> Group {
    Group::new(
        Combine::Intersect,
        vec![Item::Rule(Rule::new(criteria, operation, value))],
    )
}
fn text(s: &str) -> Option<Value> {
    Some(Value::Str(s.into()))
}

/// The members of one smart collection with `group`'s rules.
fn members(
    session: &CatalogSession,
    group: &Group,
    synonyms: &Synonyms,
) -> Result<HashSet<PhotoId>> {
    let id = CollectionId(1);
    let mut found = evaluate(
        session.catalog(),
        &[(id, Ok(group.clone()))],
        &clock(),
        synonyms,
        &AtomicBool::new(false),
    )?;
    Ok(found.remove(&id).unwrap().expect("supported"))
}

#[test]
fn copies_are_matched_with_their_own_metadata() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    let copy = session.create_virtual_copy(ids[0], None)?.value;
    session.set_ratings(&[(ids[0], 5, 0, String::new()), (copy, 1, 0, String::new())])?;
    let five = rule("rating", "==", Some(Value::Number(5.)));
    assert_eq!(
        members(&session, &five, &Synonyms::new())?,
        HashSet::from([ids[0]])
    );
    let zero = rule("rating", "==", Some(Value::Number(0.)));
    assert_eq!(
        members(&session, &zero, &Synonyms::new())?,
        HashSet::from([ids[1]])
    );
    Ok(())
}

#[test]
fn keywords_match_ancestors_and_synonyms_and_folders_their_whole_path() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    session.edit_descriptive(
        &[ids[0]],
        crate::catalog_session::DescriptiveEdit::AddKeywords(vec![vec![
            "Places".into(),
            "Japan".into(),
        ]]),
    )?;
    let japan = session
        .catalog()
        .keyword_tree()?
        .into_iter()
        .find(|k| k.name == "Japan")
        .unwrap()
        .id;
    let synonyms = Synonyms::from([(japan, vec!["Nippon".to_string()])]);
    for value in ["Places", "Japan", "Nippon"] {
        let group = rule("keywords", "words", text(value));
        assert_eq!(
            members(&session, &group, &synonyms)?,
            HashSet::from([ids[0]]),
            "{value}"
        );
    }
    let without = rule("keywords", "empty", None);
    assert_eq!(
        members(&session, &without, &synonyms)?,
        HashSet::from([ids[1]])
    );
    let trips = rule("folder", "words", text("trips"));
    assert_eq!(members(&session, &trips, &synonyms)?.len(), 2);
    Ok(())
}

#[test]
fn titles_creators_and_static_collection_names_are_matched() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    session.edit_descriptive(
        &[ids[1]],
        DescriptiveEdit::Text(TextField::Title, "Red fox".into()),
    )?;
    session.edit_descriptive(&[ids[0]], DescriptiveEdit::Creators(vec!["Ann Lee".into()]))?;
    let quick = session.ensure_quick_collection()?;
    session.change_collection(quick, &[ids[0]], &[])?;
    let no = Synonyms::new();
    assert_eq!(
        members(&session, &rule("title", "words", text("fox")), &no)?,
        HashSet::from([ids[1]])
    );
    assert_eq!(
        members(&session, &rule("creator", "==", text("ann lee")), &no)?,
        HashSet::from([ids[0]])
    );
    // The Quick Collection is Lightroom's own, not a static collection by name.
    let named = rule("collection", "any", text("quick"));
    assert!(members(&session, &named, &no)?.is_empty());
    Ok(())
}

/// Review, round 1: Any Searchable Text and Metadata search every text
/// field the catalog keeps, the copyright included.
#[test]
fn searchable_text_and_metadata_find_the_copyright() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    session.edit_descriptive(
        &[ids[1]],
        DescriptiveEdit::Text(TextField::Copyright, "© Zanzibar Studio".into()),
    )?;
    let no = Synonyms::new();
    for criteria in ["all", "metadata"] {
        let found = members(&session, &rule(criteria, "any", text("zanzibar")), &no)?;
        assert_eq!(found, HashSet::from([ids[1]]), "{criteria}");
        let none = members(&session, &rule(criteria, "noneOf", text("zanzibar")), &no)?;
        assert_eq!(none, HashSet::from([ids[0]]), "{criteria}");
    }
    Ok(())
}

/// Review, round 2: GPS Data rules see a location the catalog keeps or
/// cleared.
#[test]
fn gps_data_rules_see_stored_locations() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    let db = session.catalog_mut_for_tests().db_for_tests();
    db.execute(
        "INSERT INTO photo_location(photo, lat, lon) VALUES (?, 35.0, 135.7)",
        [ids[0].0],
    )?;
    db.execute(
        "INSERT INTO photo_location(photo, cleared) VALUES (?, 1)",
        [ids[1].0],
    )?;
    let no = Synonyms::new();
    let yes = Some(Value::Bool(true));
    assert_eq!(
        members(&session, &rule("hasGPSData", "==", yes.clone()), &no)?,
        HashSet::from([ids[0]])
    );
    assert_eq!(
        members(&session, &rule("hasGPSData", "!=", yes), &no)?,
        HashSet::from([ids[1]])
    );
    Ok(())
}

#[test]
fn edit_facts_are_read_when_a_rule_asks() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    let photo = session.photos()[0].clone();
    let mut recipe = crate::model::recipe::Recipe {
        crop: [0.1, 0.1, 0.9, 0.9],
        ..Default::default()
    };
    recipe.effects.monochrome = true;
    session.save_edit(
        photo.id,
        &photo.path,
        &crate::storage::Identity::read(&photo.path)?,
        &recipe,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    // A Lightroom edit with neither.
    session.catalog().db_for_tests().execute(
        "UPDATE photos SET lightroom_develop = 's = { HasCrop = false, Exposure2012 = 0.5 }' WHERE id = ?",
        [ids[1].0],
    )?;
    let no = Synonyms::new();
    let yes = Some(Value::Bool(true));
    assert_eq!(
        members(&session, &rule("cropped", "isTrue", yes.clone()), &no)?,
        HashSet::from([ids[0]])
    );
    assert_eq!(
        members(&session, &rule("treatment", "==", text("grayscale")), &no)?,
        HashSet::from([ids[0]])
    );
    assert_eq!(
        members(&session, &rule("hasAdjustments", "isTrue", yes), &no)?.len(),
        2
    );
    // Saved just now: "in the last 2 days" of today's local date.
    let mut recent = Rule::new("touchTime", "inLast", Some(Value::Number(2.)));
    recent.units = Some("days".into());
    let recent = Group::new(Combine::Intersect, vec![Item::Rule(recent)]);
    let today = Clock {
        today: crate::platform::clock::clock().today,
        week_start: 0,
    };
    let id = CollectionId(1);
    let found = evaluate(
        session.catalog(),
        &[(id, Ok(recent))],
        &today,
        &no,
        &AtomicBool::new(false),
    )?;
    assert_eq!(found[&id].as_ref().unwrap(), &HashSet::from([ids[0]]));
    Ok(())
}

#[test]
fn an_unsupported_rule_lists_no_photos_with_its_reason() -> Result<()> {
    let (_d, session, _) = session()?;
    let likes = rule("likes", "isTrue", Some(Value::Bool(true)));
    let five = rule("rating", ">=", Some(Value::Number(5.)));
    let found = evaluate(
        session.catalog(),
        &[(CollectionId(1), Ok(likes)), (CollectionId(2), Ok(five))],
        &clock(),
        &Synonyms::new(),
        &AtomicBool::new(false),
    )?;
    assert_eq!(
        found[&CollectionId(1)].as_ref().unwrap_err().criteria,
        "likes"
    );
    assert!(found[&CollectionId(2)].as_ref().unwrap().is_empty());
    Ok(())
}

/// Polls `session` until its smart status is no longer pending.
fn settled(session: &mut CatalogSession, clock: &Clock) -> Members {
    let started = std::time::Instant::now();
    loop {
        match session.smart_status(clock) {
            SmartStatus::Pending => {}
            SmartStatus::Ready(members) => return members.clone(),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Adds smart collection `name` with `rules` (None: Lightroom text that
/// can't be read) to the catalog, as an import does, and lists it.
fn add_smart(
    session: &mut CatalogSession,
    name: &str,
    rules: Option<&Group>,
) -> Result<CollectionId> {
    let catalog = session.catalog_mut_for_tests();
    let id: i64 = catalog.db_for_tests().query_row(
        "INSERT INTO collections(name, parent, kind)
         VALUES (?, NULL, 'com.adobe.ag.library.smart_collection') RETURNING id",
        [name],
        |r| r.get(0),
    )?;
    let id = CollectionId(id);
    let rules = match rules {
        Some(group) => crate::catalog::SmartRules::new(group),
        None => crate::catalog::SmartRules::from_lightroom("s = { os.exit() }"),
    };
    catalog.set_smart_rules(id, &rules)?;
    session.reload()?;
    Ok(id)
}

/// A result worked out for an older revision or day is never shown: the
/// status is pending until the job for the current one ends.
#[test]
fn members_follow_the_revision_and_the_day() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    assert_eq!(session.smart_status(&clock()), SmartStatus::None);
    let id = add_smart(
        &mut session,
        "Five",
        Some(&rule("rating", ">=", Some(Value::Number(5.)))),
    )?;
    let broken = add_smart(&mut session, "Broken", None)?;
    assert_eq!(session.smart_status(&clock()), SmartStatus::Pending);
    let members = settled(&mut session, &clock());
    assert!(members[&id].as_ref().unwrap().is_empty());
    let why = members[&broken].as_ref().unwrap_err().to_string();
    assert!(why.starts_with("Its rules can't be read"), "{why}");
    // A rating changed while nothing runs, and again while the job runs.
    session.set_ratings(&[(ids[0], 5, 0, String::new())])?;
    assert_eq!(session.smart_status(&clock()), SmartStatus::Pending);
    session.set_ratings(&[(ids[1], 5, 0, String::new())])?;
    let both = settled(&mut session, &clock());
    assert_eq!(
        both[&id].as_ref().unwrap(),
        &HashSet::from([ids[0], ids[1]])
    );
    // Another day: worked out again.
    let tomorrow = Clock {
        today: Date(clock().today.0 + 1),
        ..clock()
    };
    assert_eq!(session.smart_status(&tomorrow), SmartStatus::Pending);
    settled(&mut session, &tomorrow);
    Ok(())
}

/// Review, round 1: one worker at a time. A job that no longer applies is
/// cancelled and waited for, never left running beside its replacement, and
/// a cancelled evaluation stops without members.
#[test]
fn a_superseded_job_is_cancelled_not_doubled() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    add_smart(
        &mut session,
        "Five",
        Some(&rule("rating", ">=", Some(Value::Number(5.)))),
    )?;
    assert_eq!(session.smart_status(&clock()), SmartStatus::Pending);
    let first = session
        .smart
        .job
        .as_ref()
        .map(|j| (j.at, j.cancel.clone()))
        .unwrap();
    session.set_ratings(&[(ids[0], 5, 0, String::new())])?;
    assert_eq!(session.smart_status(&clock()), SmartStatus::Pending);
    match &session.smart.job {
        // Still running: told to stop, and not replaced yet.
        Some(job) if job.at == first.0 => assert!(first.1.load(Ordering::Relaxed)),
        // It had ended: the job now is the current revision's.
        Some(job) => assert_eq!(job.at.0, session.query_revision()),
        None => panic!("a job runs for the current revision"),
    }
    let members = settled(&mut session, &clock());
    assert_eq!(
        members.values().next().unwrap().as_ref().unwrap(),
        &HashSet::from([ids[0]])
    );
    // Cancelled before it starts: nothing is worked out.
    let stopped = evaluate(
        session.catalog(),
        &[(
            CollectionId(9),
            Ok(rule("rating", ">=", Some(Value::Number(0.)))),
        )],
        &clock(),
        &Synonyms::new(),
        &AtomicBool::new(true),
    )?;
    assert!(stopped.is_empty());
    Ok(())
}

/// The worker reads the keywords' Lightroom synonyms from the catalog.
#[test]
fn keyword_rules_match_synonyms_stored_in_the_catalog() -> Result<()> {
    let (_d, mut session, ids) = session()?;
    session.edit_descriptive(
        &[ids[0]],
        DescriptiveEdit::AddKeywords(vec![vec!["Japan".into()]]),
    )?;
    session.catalog_mut_for_tests().db_for_tests().execute(
        "INSERT INTO keyword_synonyms(keyword, name) SELECT id, 'Nippon' FROM keywords WHERE name='Japan'",
        [],
    )?;
    let id = add_smart(
        &mut session,
        "Nippon",
        Some(&rule("keywords", "any", text("nippon"))),
    )?;
    assert_eq!(
        settled(&mut session, &clock())[&id].as_ref().unwrap(),
        &HashSet::from([ids[0]])
    );
    Ok(())
}

/// A catalog that can't be read gives an error and no members, until the
/// revision changes. Unix only: Windows doesn't delete an open file.
#[cfg(unix)]
#[test]
fn a_failed_evaluation_keeps_no_members() -> Result<()> {
    let (directory, mut session, _) = session()?;
    add_smart(
        &mut session,
        "Five",
        Some(&rule("rating", ">=", Some(Value::Number(5.)))),
    )?;
    settled(&mut session, &clock());
    session.note_external_change([PhotoId(1)]);
    // The worker opens the catalog file, which is gone.
    std::fs::remove_file(directory.path().join("library.rawmakase"))?;
    let started = std::time::Instant::now();
    let error = loop {
        match session.smart_status(&clock()) {
            SmartStatus::Pending => {}
            SmartStatus::Failed(error) => break error.to_string(),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert!(
        error.starts_with("Smart collections could not be read"),
        "{error}"
    );
    Ok(())
}

/// The end-to-end cost at 100,000 photos (issue 998): reading what rules
/// read, building entries and evaluating 20 smart collections. Run with
/// `cargo test --release smart_collections_at_100k -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark"]
fn smart_collections_at_100k() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("bench.rawmakase");
    let catalog = Catalog::create(&path)?;
    {
        let db = catalog.db_for_tests();
        db.execute_batch(
            "BEGIN;
             INSERT INTO roots(id, original_path) VALUES (1, '/photos');
             INSERT INTO folders(id, root, relative_path) VALUES (1, 1, 'trips');",
        )?;
        let mut photo = db.prepare(
            "INSERT INTO photos(id, folder, filename, original_path, captured, rating, label)
             VALUES (?, 1, ?, ?, ?, ?, ?)",
        )?;
        let mut info = db.prepare(
            "INSERT INTO photo_info(photo, camera, lens, focal, aperture, exposure, iso, width, height)
             VALUES (?, 'X-T5', 'XF23mm', 23, 2.8, 0.004, 400, 6000, 4000)",
        )?;
        let mut keyword = db.prepare("INSERT INTO photo_keywords(photo, keyword) VALUES (?, ?)")?;
        db.execute_batch(
            "INSERT INTO keywords(id, name, parent) VALUES (1, 'Places', NULL), (2, 'Japan', 1), (3, 'Kyoto', 2);",
        )?;
        for id in 1..=100_000i64 {
            let name = format!("IMG_{id:06}.RAF");
            let day = 1 + id % 28;
            photo.execute(rusqlite::params![
                id,
                name,
                format!("/photos/trips/{name}"),
                format!("2026-10-{day:02}T12:00:00"),
                id % 6,
                ["", "Red", "Green"][(id % 3) as usize],
            ])?;
            info.execute([id])?;
            if id % 4 == 0 {
                keyword.execute([id, 3])?;
            }
        }
        db.execute_batch("COMMIT;")?;
    }
    drop(catalog);
    let session = CatalogSession::open(&CatalogLocation::from(&path))?.session;
    assert_eq!(session.photos().len(), 100_000);
    let rules: Vec<Group> = (0..20)
        .map(|i| match i % 5 {
            0 => rule("rating", ">=", Some(Value::Number(f64::from(i % 6)))),
            1 => rule("keywords", "any", text("japan")),
            2 => rule("filename", "any", text("IMG_00")),
            3 => {
                let mut r = Rule::new("captureTime", "inLast", Some(Value::Number(30.)));
                r.units = Some("days".into());
                Group::new(
                    Combine::Union,
                    vec![
                        Item::Rule(r),
                        Item::Rule(Rule::new("labelColor", "==", Some(Value::Number(1.)))),
                    ],
                )
            }
            _ => rule("lens", "words", text("XF23mm")),
        })
        .collect();
    let smart: Vec<(CollectionId, Result<Group, Unsupported>)> = rules
        .into_iter()
        .enumerate()
        .map(|(i, g)| (CollectionId(i as i64 + 1), Ok(g)))
        .collect();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let found = evaluate(
            session.catalog(),
            &smart,
            &clock(),
            &Synonyms::new(),
            &AtomicBool::new(false),
        )?;
        let took = started.elapsed();
        let total: usize = found
            .values()
            .map(|m| m.as_ref().map_or(0, HashSet::len))
            .sum();
        println!("100k photos, 20 smart collections: {took:?} ({total} memberships)");
    }
    Ok(())
}
