//! A plan written from a Lightroom catalog imports to the rows Lightroom
//! import writes, each rule of the plan fails a plan that breaks it, and the
//! gate and publishing keep or delete what they should.
use super::*;
use crate::catalog::lightroom::history::decode_history_text;
use crate::catalog::lightroom::import_lightroom;
use crate::catalog::tests::fixture;
use crate::catalog::{CollectionKind, QUICK_COLLECTION};
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, params, types::Value};
use std::collections::{BTreeMap, HashMap};

const SOURCE: &str = "/Volumes/Photos/Catalog.lrcat";

/// The plan of `tests::fixture`, written by hand: its ids are Lightroom's,
/// its row ids not.
const FIXTURE_PLAN: &str = "
    INSERT INTO plan VALUES (1, 2);
    INSERT INTO root VALUES ('10', '/Volumes/Photos/');
    INSERT INTO folder VALUES ('21', '10', 'Trip/Day2/'), ('20', '10', 'Trip/');
    INSERT INTO photo VALUES
        ('41', '20', 'image.ARW', '/Volumes/Photos/Trip/image.ARW', '2021-06-06T10:00:00',
         2, -1, 'Blue', 'RAW', 'AB', '40', 'B&W', NULL),
        ('40', '20', 'image.ARW', '/Volumes/Photos/Trip/image.ARW', '2021-06-06T10:00:00',
         4, 1, 'Red', 'RAW', 'AB', NULL, '', NULL);
    INSERT INTO edit VALUES ('41', 'lightroom-develop', 's = { ConvertToGrayscale = true }'),
        ('40', 'lightroom-develop', 's = { Exposure2012 = 1.5 }');
    INSERT INTO collection VALUES ('50', 'Travel', NULL, 'collection');
    INSERT INTO collection_photo VALUES ('50', '41', 'b'), ('50', '40', 'a');
    INSERT INTO keyword VALUES ('60', 'City', '59', 1, 1), ('59', '', NULL, 1, 1);
    INSERT INTO photo_keyword VALUES ('40', '60');";

/// Writes the plan an importer would write for the Lightroom catalog
/// `source` into `plan`, which has the plan's tables: what Lightroom import
/// reads, read as it reads it, its ids Lightroom's. Rows go in in reverse,
/// so their row ids aren't those ids.
fn plan_from_lightroom(plan: &Connection, source: &Path) -> Result<()> {
    plan.create_scalar_function("decoded", 1, FunctionFlags::SQLITE_DETERMINISTIC, |f| {
        let bytes = match f.get::<Value>(0)? {
            Value::Text(t) => t.into_bytes(),
            Value::Blob(b) => b,
            _ => return Ok(None),
        };
        Ok(decode_history_text(&bytes))
    })?;
    // APEX values, as `info::copy_lightroom_info` turns them.
    plan.create_scalar_function("f_number", 1, FunctionFlags::SQLITE_DETERMINISTIC, |f| {
        Ok(f.get::<Option<f64>>(0)?.map(|av| 2f64.powf(av / 2.)))
    })?;
    plan.create_scalar_function("seconds", 1, FunctionFlags::SQLITE_DETERMINISTIC, |f| {
        Ok(f.get::<Option<f64>>(0)?.map(|tv| 2f64.powf(-tv)))
    })?;
    plan.execute("ATTACH DATABASE ? AS lr", [source.to_string_lossy()])?;
    let has = |table: &str, column: &str| -> Result<bool> {
        Ok(plan.query_row(
            "SELECT count(*) FROM lr.sqlite_master m JOIN pragma_table_info(m.name, 'lr') c
             WHERE m.name = ? AND (c.name = ? OR ? = '')",
            params![table, column, column],
            |r| r.get::<_, i64>(0),
        )? > 0)
    };
    let file_name = super::super::lightroom::lightroom_filename!();
    let original = if has("AgLibraryFile", "originalFilename")? {
        "f.originalFilename"
    } else {
        "NULL"
    };
    plan.execute_batch(&format!(
        "INSERT INTO plan SELECT 1, count(*) FROM lr.Adobe_images;
         INSERT INTO root SELECT id_local, absolutePath FROM lr.AgLibraryRootFolder
             ORDER BY id_local DESC;
         INSERT INTO folder SELECT id_local, rootFolder, pathFromRoot FROM lr.AgLibraryFolder
             ORDER BY id_local DESC;
         INSERT INTO photo SELECT i.id_local, f.folder, {file_name},
             r.absolutePath || d.pathFromRoot || {file_name}, COALESCE(i.captureTime, ''),
             COALESCE(i.rating, 0), COALESCE(i.pick, 0), COALESCE(i.colorLabels, ''),
             COALESCE(i.fileFormat, ''), i.orientation, i.masterImage, COALESCE(i.copyName, ''),
             {original}
             FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local = i.rootFile
             JOIN lr.AgLibraryFolder d ON d.id_local = f.folder
             JOIN lr.AgLibraryRootFolder r ON r.id_local = d.rootFolder
             ORDER BY i.id_local DESC;
         INSERT INTO edit SELECT image, 'lightroom-develop', text FROM lr.Adobe_imageDevelopSettings
             WHERE text IS NOT NULL AND image IN (SELECT id FROM photo)
             AND rowid IN (SELECT min(rowid) FROM lr.Adobe_imageDevelopSettings GROUP BY image);
         INSERT INTO history SELECT image, row_number() OVER (PARTITION BY image
                 ORDER BY dateCreated, id_local), COALESCE(name, ''), dateCreated, decoded(text)
             FROM lr.Adobe_libraryImageDevelopHistoryStep
             WHERE text IS NOT NULL AND image IN (SELECT id FROM photo);
         INSERT INTO collection SELECT id_local, name, parent, CASE
                 WHEN creationId = 'com.adobe.ag.library.group' THEN 'set'
                 WHEN creationId = 'com.adobe.ag.library.smart_collection' THEN 'smart'
                 WHEN creationId = 'com.adobe.ag.library.collection' AND name = '{QUICK_COLLECTION}'
                     THEN 'quick'
                 WHEN creationId LIKE '%.unsaved' THEN 'system'
                 ELSE 'collection' END
             FROM lr.AgLibraryCollection ORDER BY id_local DESC;
         INSERT INTO collection_photo SELECT collection, image, positionInCollection
             FROM lr.AgLibraryCollectionImage
             WHERE collection IN (SELECT id FROM collection) AND image IN (SELECT id FROM photo);
         INSERT INTO keyword SELECT id_local, COALESCE(name, ''), parent, {export}
             FROM lr.AgLibraryKeyword ORDER BY id_local DESC;
         INSERT INTO photo_keyword SELECT image, tag FROM lr.AgLibraryKeywordImage
             WHERE image IN (SELECT id FROM photo) AND tag IN (SELECT id FROM keyword);",
        export = if has("AgLibraryKeyword", "includeOnExport")? {
            "COALESCE(includeOnExport, 1) <> 0, COALESCE(includeParents, 1) <> 0"
        } else {
            "1, 1"
        }
    ))?;
    if has("Adobe_libraryImageDevelopSnapshot", "")? {
        plan.execute_batch(
            "INSERT INTO snapshot SELECT image, COALESCE(name, ''), decoded(text)
             FROM lr.Adobe_libraryImageDevelopSnapshot
             WHERE text IS NOT NULL AND image IN (SELECT id FROM photo)",
        )?;
    }
    if has("AgLibraryFolderStackImage", "")? {
        plan.execute_batch(
            "INSERT INTO stack SELECT id_local, collapsed <> 0 FROM lr.AgLibraryFolderStack;
             INSERT INTO stack_photo SELECT stack, image, row_number() OVER (PARTITION BY stack
                 ORDER BY CAST(position AS REAL), id_local)
             FROM lr.AgLibraryFolderStackImage
             WHERE image IN (SELECT id FROM photo) AND stack IN (SELECT id FROM stack);",
        )?;
    }
    if has("AgHarvestedExifMetadata", "")? {
        plan.execute_batch(
            "INSERT INTO info SELECT i.id_local, c.value, l.value, e.focalLength,
                 f_number(e.aperture), seconds(e.shutterSpeed), e.isoSpeedRating,
                 CAST(CASE WHEN i.orientation IN ('BC', 'DA', 'AD', 'CB') THEN i.fileHeight
                      ELSE i.fileWidth END AS INTEGER),
                 CAST(CASE WHEN i.orientation IN ('BC', 'DA', 'AD', 'CB') THEN i.fileWidth
                      ELSE i.fileHeight END AS INTEGER)
             FROM lr.Adobe_images i
             LEFT JOIN lr.AgHarvestedExifMetadata e ON e.image = i.id_local
             LEFT JOIN lr.AgInternedExifCameraModel c ON c.id_local = e.cameraModelRef
             LEFT JOIN lr.AgInternedExifLens l ON l.id_local = e.lensRef
             WHERE i.id_local IN (SELECT id FROM photo)",
        )?;
    }
    if has("Adobe_AdditionalMetadata", "")? {
        plan.execute_batch(
            "INSERT INTO metadata SELECT image, COALESCE(CAST(xmp AS TEXT), '')
             FROM lr.Adobe_AdditionalMetadata WHERE image IN (SELECT id FROM photo)",
        )?;
    }
    plan.execute_batch("DETACH DATABASE lr")?;
    Ok(())
}

/// What each catalog id of a record kind stands for, `None` when it is the
/// id itself (a catalog Lightroom import wrote).
type IdMap = Option<HashMap<(&'static str, i64), String>>;

/// The ids of the plan at `plan`: each kind's row id and the id it has.
fn plan_ids(plan: &Connection) -> Result<IdMap> {
    let mut ids = HashMap::new();
    for kind in ["root", "folder", "photo", "collection", "stack", "keyword"] {
        let mut statement = plan.prepare(&format!("SELECT rowid, id FROM {kind}"))?;
        let rows = statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)));
        for row in rows? {
            let (rowid, id) = row?;
            ids.insert((kind, rowid), id);
        }
    }
    Ok(Some(ids))
}

/// The columns that hold an id, and of which kind; a column named `photo`
/// holds a photo's.
const ID_COLUMNS: &[(&str, &str, &str)] = &[
    ("roots", "id", "root"),
    ("folders", "id", "folder"),
    ("folders", "root", "root"),
    ("photos", "id", "photo"),
    ("photos", "folder", "folder"),
    ("photos", "master_id", "photo"),
    ("collections", "id", "collection"),
    ("collections", "parent", "collection"),
    ("collection_photos", "collection", "collection"),
    ("keywords", "id", "keyword"),
    ("keywords", "parent", "keyword"),
    ("photo_keywords", "keyword", "keyword"),
    ("keyword_export", "keyword", "keyword"),
    ("stacks", "id", "stack"),
    ("stack_photos", "stack", "stack"),
    ("folder_paths", "folder", "folder"),
    ("folder_mappings", "folder", "folder"),
    ("folder_locations", "root", "root"),
];

/// Every table's rows as sorted text, ids named through `ids`, Lightroom
/// develop text decoded and a collection's kind as it reads. When the import
/// ran, its archive and snapshot ids aren't compared.
fn rows(catalog: &Path, ids: &IdMap) -> Result<BTreeMap<String, Vec<String>>> {
    let catalog = Catalog::open(catalog)?;
    let db = catalog.db_for_tests();
    let mut all = BTreeMap::new();
    for table in catalog.db.table_names()? {
        let mut statement = db.prepare(&format!("SELECT * FROM \"{table}\""))?;
        let columns: Vec<String> = statement.column_names().map(String::from).collect();
        let mut found = Vec::new();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let mut text = Vec::new();
            for (i, column) in columns.iter().enumerate() {
                let value: Value = row.get(i)?;
                let kind = ID_COLUMNS
                    .iter()
                    .find(|(t, c, _)| *t == table && c == column)
                    .map(|(_, _, kind)| *kind)
                    .or_else(|| (column == "photo").then_some("photo"));
                let shown = match (table.as_str(), column.as_str(), kind, &value) {
                    ("sources", "id" | "imported_at" | "original_catalog", ..)
                    | ("develop_snapshots", "id", ..)
                    | ("computers", "adopted_at", ..) => continue,
                    (.., Some(kind), Value::Integer(id)) => match ids {
                        None => format!("{kind}:{id}"),
                        Some(map) => format!("{kind}:{}", map[&(kind, *id)]),
                    },
                    ("lightroom_history", "text", ..) | ("develop_snapshots", "lightroom", ..) => {
                        match value {
                            Value::Text(t) => format!("{:?}", decode_history_text(t.as_bytes())),
                            Value::Blob(b) => format!("{:?}", decode_history_text(&b)),
                            other => format!("{other:?}"),
                        }
                    }
                    ("collections", "kind", _, Value::Text(kind)) => {
                        let name: String =
                            row.get(columns.iter().position(|c| c == "name").unwrap())?;
                        format!("{:?}", CollectionKind::from_lightroom(kind, &name))
                    }
                    _ => format!("{value:?}"),
                };
                text.push(format!("{column}={shown}"));
            }
            found.push(text.join(", "));
        }
        found.sort();
        all.insert(table, found);
    }
    Ok(all)
}

/// A plan file at `path` with `schema`'s tables.
fn plan_file(path: &Path, schema: &str) -> Result<Connection> {
    let plan = Connection::open(path)?;
    plan.execute_batch(schema)?;
    Ok(plan)
}

/// [`SCHEMA`] without its constraints, as a plan that breaks them has it:
/// the core checks every rule itself.
fn unconstrained() -> &'static str {
    static SCHEMA_: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SCHEMA_.get_or_init(|| {
        let mut out = String::new();
        for line in SCHEMA.lines() {
            let line = line.trim_end_matches(',');
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some("PRIMARY" | "UNIQUE"), _) => {}
                (Some("CREATE"), _) => out += &format!("{line}\n"),
                (Some(")"), _) => {
                    out = out.trim_end_matches(",\n").to_string();
                    out += "\n);\n";
                }
                (Some(column), Some(class)) => out += &format!("    {column} {class},\n"),
                _ => {}
            }
        }
        out
    })
}

/// An import of `source` whose program wrote the plan `write` makes on a
/// file with `schema`'s tables and answered its photos, and the folder its
/// destination is in.
fn prepared(
    source: &Path,
    schema: &'static str,
    write: impl FnOnce(&Connection) -> Result<()>,
) -> Result<(tempfile::TempDir, PlanImport, u64, IdMap)> {
    let dir = tempfile::tempdir()?;
    let mut import = PlanImport::prepare(source, &dir.path().join("new.rawmakase"))?;
    import.schema = schema;
    let plan = plan_file(&import.plan(), schema)?;
    write(&plan)?;
    let photos: i64 = plan.query_row("SELECT count(*) FROM photo", [], |r| r.get(0))?;
    let ids = plan_ids(&plan)?;
    Ok((dir, import, photos as u64, ids))
}

/// The base fixture, changed by `change` before it is read.
fn variation(dir: &Path, name: &str, change: &str) -> Result<PathBuf> {
    let source = dir.join(format!("{name}.lrcat"));
    fixture(&source)?;
    Connection::open(&source)?.execute_batch(change)?;
    Ok(source)
}

/// Imports the Lightroom catalog `source` with Lightroom import and as a
/// plan, and compares the two catalogs.
fn assert_parity(source: &Path) -> Result<()> {
    let d = tempfile::tempdir()?;
    let expected = d.path().join("expected.rawmakase");
    import_lightroom(source, &expected)?;
    let (_dir, import, photos, ids) = prepared(source, SCHEMA, |p| plan_from_lightroom(p, source))?;
    let actual = import.run(photos, &mut |_| {})?;
    let (expected, actual) = (rows(&expected, &None)?, rows(&actual, &ids)?);
    for (table, rows) in &expected {
        assert_eq!(rows, &actual[table], "{}: {table}", source.display());
    }
    assert_eq!(expected.len(), actual.len());
    Ok(())
}

fn compressed(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text.as_bytes()).unwrap();
    let mut blob = (text.len() as u32).to_be_bytes().to_vec();
    blob.extend(z.finish().unwrap());
    blob
}

/// Every row of every plan table, sorted, without row ids.
fn plan_rows(plan: &Connection) -> Result<Vec<String>> {
    let tables: Vec<String> = plan
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut all = Vec::new();
    for table in tables {
        let mut statement = plan.prepare(&format!("SELECT * FROM {table}"))?;
        let n = statement.column_count();
        let rows = statement.query_map([], |r| {
            (0..n)
                .map(|i| r.get::<_, Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        for row in rows {
            all.push(format!("{table} {:?}", row?));
        }
    }
    all.sort();
    Ok(all)
}

#[test]
fn the_fixture_written_by_hand_imports_as_lightroom_import_does() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("fixture.lrcat");
    fixture(&source)?;
    // The tests' reader of Lightroom catalogs reads the plan written by hand.
    let by_hand = plan_file(&d.path().join("by-hand.sqlite"), SCHEMA)?;
    by_hand.execute_batch(FIXTURE_PLAN)?;
    let read = plan_file(&d.path().join("read.sqlite"), SCHEMA)?;
    plan_from_lightroom(&read, &source)?;
    assert_eq!(plan_rows(&by_hand)?, plan_rows(&read)?);

    let expected = d.path().join("expected.rawmakase");
    import_lightroom(&source, &expected)?;
    let (_dir, import, photos, ids) =
        prepared(&source, SCHEMA, |p| Ok(p.execute_batch(FIXTURE_PLAN)?))?;
    let actual = import.run(photos, &mut |_| {})?;
    assert_eq!(rows(&expected, &None)?, rows(&actual, &ids)?);
    Ok(())
}

#[test]
fn every_lightroom_import_variation_imports_the_same_from_a_plan() -> Result<()> {
    let d = tempfile::tempdir()?;
    let dir = d.path();
    let packet = |title: &str| {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
              xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/"
              exif:DateTimeOriginal="2021-06-06T10:00:00.25+01:00" xmp:Rating="1">
              <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
              <dc:creator><rdf:Seq><rdf:li>Example</rdf:li></rdf:Seq></dc:creator>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#
        )
    };
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02X}")).collect::<String>();
    let sources = [
        variation(dir, "base", "")?,
        variation(
            dir,
            "stacks",
            "CREATE TABLE AgLibraryFolderStack(id_local INTEGER PRIMARY KEY,id_global UNIQUE NOT NULL,collapsed INTEGER NOT NULL DEFAULT 0,text NOT NULL DEFAULT '');
             CREATE TABLE AgLibraryFolderStackImage(id_local INTEGER PRIMARY KEY,collapsed INTEGER NOT NULL DEFAULT 0,image INTEGER NOT NULL DEFAULT 0,position NOT NULL DEFAULT '',stack INTEGER NOT NULL DEFAULT 0);
             INSERT INTO AgLibraryFile VALUES(31,20,'other.ARW','other','ARW'),(32,20,'third.ARW','third','ARW');
             INSERT INTO Adobe_images VALUES(42,31,'2021-06-06T10:00:01',0,0,'','RAW','',NULL,'AB'),(43,32,'2021-06-06T10:00:02',0,0,'','RAW','',NULL,'AB');
             INSERT INTO AgLibraryFolderStack VALUES(70,'A',1,''),(71,'B',0,''),(72,'C',0,'');
             INSERT INTO AgLibraryFolderStackImage VALUES(1,1,40,'2',70),(2,1,42,'1',70),(3,1,41,'3',70),
                 (4,0,43,1,71),(5,0,999,2,71);",
        )?,
        variation(
            dir,
            "original-names",
            "ALTER TABLE AgLibraryFile ADD COLUMN originalFilename TEXT;
             UPDATE AgLibraryFile SET originalFilename='DSCF0001.RAF' WHERE id_local=30;
             INSERT INTO AgLibraryFile VALUES(31,20,'other.ARW','other','ARW',NULL),(32,20,'third.ARW','third','ARW','');
             INSERT INTO Adobe_images VALUES(42,31,'',0,0,'','RAW','',NULL,'AB'),(43,32,'',0,0,'','RAW','',NULL,'AB');",
        )?,
        variation(
            dir,
            "labels",
            "UPDATE Adobe_images SET rating=NULL,pick=NULL,colorLabels=NULL WHERE id_local=40;
             INSERT INTO Adobe_images VALUES(100,30,'2021-06-06T10:00:00',0.0,-1.0,'Red','RAW','Virtual',40,'AB'),
                 (101,30,'2021-06-06T10:00:00',1.0,0.0,'Yellow','RAW','Virtual',40,'AB'),
                 (102,30,'2021-06-06T10:00:00',2.0,1.0,'Client approved','RAW','Virtual',40,'AB'),
                 (103,30,'2021-06-06T10:00:00',3.0,-1.0,'Czerwony','RAW','Virtual',40,'AB');",
        )?,
        variation(
            dir,
            "info",
            "ALTER TABLE Adobe_images ADD COLUMN fileWidth;
             ALTER TABLE Adobe_images ADD COLUMN fileHeight;
             UPDATE Adobe_images SET fileWidth=6000, fileHeight=4000;
             UPDATE Adobe_images SET orientation='BC' WHERE id_local=40;
             CREATE TABLE AgHarvestedExifMetadata(image, aperture, shutterSpeed, isoSpeedRating,
                 focalLength, cameraModelRef, lensRef);
             CREATE TABLE AgInternedExifCameraModel(id_local, value);
             CREATE TABLE AgInternedExifLens(id_local, value);
             INSERT INTO AgInternedExifCameraModel VALUES(1,'ILCE-7M2');
             INSERT INTO AgInternedExifLens VALUES(2,'FE 55mm F1.8 ZA');
             INSERT INTO AgHarvestedExifMetadata VALUES(40, 2.0, 4.643856, 1000, 55, 1, 2);",
        )?,
        variation(
            dir,
            "history",
            &format!(
                "INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES
                     (40, 1, 400., 'zlib', X'{}'), (40, 2, 100., 'plain', 's = {{ Contrast = 25 }}'),
                     (40, 3, 300., 'no text', NULL), (41, 4, 200., 'other photo', 's = {{ Highlights = 10 }}');",
                hex(&compressed("s = { Exposure2012 = 0.5 }"))
            ),
        )?,
        variation(
            dir,
            "descriptive",
            &format!(
                "CREATE TABLE Adobe_AdditionalMetadata(id_local INTEGER, image INTEGER, xmp TEXT);
                 INSERT INTO Adobe_AdditionalMetadata VALUES(1, 40, '{}'), (2, 41, 'not XMP');",
                packet("Master")
            ),
        )?,
        variation(
            dir,
            "keyword-export",
            "ALTER TABLE AgLibraryKeyword ADD COLUMN includeOnExport INTEGER NOT NULL DEFAULT 1;
             ALTER TABLE AgLibraryKeyword ADD COLUMN includeParents INTEGER NOT NULL DEFAULT 1;
             UPDATE AgLibraryKeyword SET includeOnExport=0 WHERE id_local=60;
             INSERT INTO AgLibraryKeyword VALUES(61,'Old Town',60,1,1),(62,'Square',61,1,0);
             INSERT INTO AgLibraryKeywordImage VALUES(40,62);",
        )?,
        variation(
            dir,
            "snapshots",
            &format!(
                "CREATE TABLE Adobe_libraryImageDevelopSnapshot(id_local INTEGER, image INTEGER, name TEXT, text BLOB);
                 INSERT INTO Adobe_libraryImageDevelopSnapshot VALUES
                    (1, 40, 'Before crop', 's = {{ Exposure2012 = 0.5 }}'),
                    (2, 40, 'Before crop', 's = {{ Exposure2012 = 0.5 }}'),
                    (3, 41, NULL, 's = {{ Exposure2012 = 1 }}'),
                    (4, 99, 'Other catalog photo', 's = {{ Exposure2012 = 1 }}'),
                    (5, 40, 'Compressed', X'{}');",
                hex(&compressed("s = { Contrast = 5 }"))
            ),
        )?,
        variation(
            dir,
            "collections",
            "INSERT INTO AgLibraryCollection VALUES
                 (51,'Trips',NULL,'com.adobe.ag.library.group'),
                 (52,'Summer',51,'com.adobe.ag.library.collection'),
                 (53,'Five stars',51,'com.adobe.ag.library.smart_collection'),
                 (54,'quick collection',NULL,'com.adobe.ag.library.collection'),
                 (55,'Unsaved Print',NULL,'com.adobe.ag.print.unsaved');
             INSERT INTO AgLibraryCollectionImage VALUES(52,41,'x'),(54,40,'y'),(52,999,'z');",
        )?,
    ];
    for source in sources {
        assert_parity(&source)?;
    }
    Ok(())
}

#[test]
fn an_imported_plan_reads_back_as_its_rows_say() -> Result<()> {
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        p.execute_batch(FIXTURE_PLAN)?;
        Ok(p.execute_batch(
            "UPDATE plan SET source_photos = 3;
             INSERT INTO photo VALUES ('42', '20', 'other.ARW', '/Volumes/Photos/Trip/other.ARW',
                 '', 0, 0, '', 'RAW', NULL, NULL, '', 'DSCF0002.RAF');
             INSERT INTO collection VALUES ('quick', 'quick collection', NULL, 'quick'),
                 ('print', 'Unsaved Print', NULL, 'system'), ('set', 'Trips', NULL, 'set'),
                 ('smart', 'Five stars', 'set', 'smart');
             INSERT INTO stack VALUES ('empty', 1), ('s', 0);
             INSERT INTO stack_photo VALUES ('s', '41', 30), ('s', '40', 10), ('s', '42', 20);
             INSERT INTO history VALUES ('40', 2, 'Second', 2.5, 's = { Contrast = 5 }'),
                 ('40', 1, 'First', NULL, 's = {}');
             INSERT INTO snapshot VALUES ('40', 'Before', 's = { Exposure2012 = 1 }');
             INSERT INTO info VALUES ('40', 'ILCE-7M2', NULL, 55, 2.0, 0.04, 1000, 4000, 6000);",
        )?)
    })?;
    let destination = import.run(photos, &mut |_| {})?;
    let mut cat = Catalog::open(&destination)?;
    let id = |name: &str| -> Result<PhotoId> {
        Ok(cat
            .photos()?
            .into_iter()
            .find(|p| p.filename == name && p.master.is_none())
            .context(name.to_string())?
            .id)
    };
    let (master, other) = (id("image.ARW")?, id("other.ARW")?);
    let copy = cat
        .photos()?
        .into_iter()
        .find(|p| p.master == Some(master))
        .context("copy")?
        .id;
    // Positions as Lightroom keeps them, numbered from 1.
    assert_eq!(cat.stack_of(master)?, [master, other, copy]);
    assert!(!cat.stack_places()?[&master].collapsed);
    let stacks: i64 = cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM stacks", [], |r| r.get(0))?;
    assert_eq!(stacks, 1, "an empty stack is none");
    assert_eq!(
        cat.import_facts(other)?,
        Some(("DSCF0002.RAF".into(), None, None, false))
    );
    let history = cat.lightroom_history(master)?;
    assert_eq!(
        history.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(history[1].created, Some(2.5));
    assert_eq!(cat.snapshots(master)?.len(), 1);
    let info = cat.photo_info(master)?.context("info")?;
    assert_eq!(
        (info.camera.as_deref(), info.dimensions),
        (Some("ILCE-7M2"), Some((4000, 6000)))
    );
    let kinds: Vec<(String, CollectionKind, bool)> = cat
        .collections()?
        .into_iter()
        .map(|c| (c.name, c.kind, c.parent.is_some()))
        .collect();
    for expected in [
        ("Travel".to_string(), CollectionKind::Collection, false),
        (QUICK_COLLECTION.into(), CollectionKind::System, false),
        ("Unsaved Print".into(), CollectionKind::System, false),
        ("Trips".into(), CollectionKind::Set, false),
        ("Five stars".into(), CollectionKind::Smart, true),
    ] {
        assert!(kinds.contains(&expected), "{expected:?} in {kinds:?}");
    }
    let quick = cat
        .collections()?
        .into_iter()
        .find(|c| c.name == QUICK_COLLECTION)
        .context("quick")?
        .id;
    assert_eq!(cat.quick_collection()?, quick, "found, not made again");
    let source: (String, i64, Vec<u8>) = cat.db_for_tests().query_row(
        "SELECT path, original_size, original_catalog FROM sources",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(
        source,
        (
            std::path::absolute(SOURCE)?.to_string_lossy().into(),
            0,
            Vec::new()
        )
    );
    Ok(())
}

/// The import of the fixture's plan changed by `change` on a file without
/// the schema's constraints, answered with its photos plus `answered`.
fn fails_with(change: &str, answered: i64, expected: &str) {
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), unconstrained(), |p| {
        p.execute_batch(FIXTURE_PLAN)?;
        Ok(p.execute_batch(change)?)
    })
    .unwrap();
    let destination = import.destination().to_path_buf();
    let error = import
        .run(photos.saturating_add_signed(answered), &mut |_| {})
        .expect_err(expected);
    assert!(
        format!("{error:#}").contains(expected),
        "{change}: {error:#}"
    );
    assert!(!destination.exists());
}

#[test]
fn a_plan_breaking_any_rule_fails_saying_which() {
    for (change, expected) in [
        ("DELETE FROM plan", "has 0 plan rows"),
        ("INSERT INTO plan VALUES (1, 2)", "has more than one plan row"),
        ("UPDATE plan SET version = 2", "is version 2"),
        ("UPDATE photo SET rating = 'four' WHERE id = '40'", "photo row 2 is malformed"),
        ("UPDATE photo SET file_name = X'00' WHERE id = '41'", "photo row 1 is malformed"),
        ("UPDATE edit SET text = NULL WHERE photo = '40'", "edit row 2 is malformed"),
        ("UPDATE folder SET rowid = 0 WHERE id = '21'", "folder row 0 is malformed"),
        ("INSERT INTO root VALUES ('10', '/')", "two roots with one id: \"10\""),
        ("INSERT INTO photo SELECT * FROM photo WHERE id = '40'", "two photos with one id"),
        ("INSERT INTO keyword VALUES ('59', 'x', NULL, 1, 1)", "two keywords with one id"),
        ("UPDATE folder SET root = '11'", "folder in a root it doesn't have: \"11\""),
        ("UPDATE photo SET folder = '22' WHERE id = '40'", "photo in a folder it doesn't have"),
        ("UPDATE photo SET master = '99' WHERE id = '41'", "virtual copy of a photo it doesn't"),
        ("UPDATE edit SET photo = '99' WHERE photo = '41'", "edit for a photo it doesn't have"),
        ("INSERT INTO history VALUES ('9', 1, '', NULL, '')", "History step for a photo"),
        ("UPDATE collection SET parent = '49'", "collection in a set it doesn't have"),
        ("UPDATE collection_photo SET photo = '9' WHERE photo = '40'", "collection holding a photo"),
        ("INSERT INTO stack_photo VALUES ('7', '40', 1)", "photo in a stack it doesn't have"),
        ("UPDATE keyword SET parent = '58' WHERE id = '60'", "keyword under a keyword it doesn't"),
        ("UPDATE photo_keyword SET keyword = '61'", "photo with a keyword it doesn't have"),
        ("INSERT INTO info (photo) VALUES ('40'), ('40')", "two info rows for one photo: \"40\""),
        ("INSERT INTO metadata VALUES ('40', ''), ('40', '')", "two metadata rows for one photo"),
        ("INSERT INTO edit VALUES ('40', 'lightroom-develop', '')", "two edits for one photo"),
        (
            "INSERT INTO history VALUES ('40', 1, 'a', NULL, ''), ('40', 1, 'b', NULL, '')",
            "two History steps at one position of a photo: \"40\"",
        ),
        ("INSERT INTO collection_photo VALUES ('50', '40', 'a')", "photo twice in one collection"),
        ("INSERT INTO photo_keyword VALUES ('40', '60')", "keyword twice on one photo"),
        (
            "INSERT INTO stack VALUES ('s', 1); INSERT INTO stack_photo VALUES ('s', '40', 1), ('s', '41', 1)",
            "stack with two photos at one position: \"s\"",
        ),
        (
            "INSERT INTO stack VALUES ('s', 1), ('t', 1);
             INSERT INTO stack_photo VALUES ('s', '40', 1), ('t', '40', 1)",
            "photo in two stacks: \"40\"",
        ),
        (
            "UPDATE keyword SET include_on_export = 2 WHERE id = '60'",
            "keyword with an export option other than 0 and 1: \"60\"",
        ),
        (
            "UPDATE keyword SET export_containing = -1 WHERE id = '60'",
            "keyword with an export option other than 0 and 1: \"60\"",
        ),
        (
            "INSERT INTO stack VALUES ('s', 2)",
            "stack collapsed other than 0 and 1: \"s\"",
        ),
        ("UPDATE photo SET rating = 6 WHERE id = '40'", "rating outside 0 to 5: \"40\""),
        ("UPDATE photo SET flag = 2 WHERE id = '41'", "flag other than -1, 0 and 1: \"41\""),
        ("UPDATE collection SET kind = 'album'", "collection of an unknown kind: \"50\""),
        ("UPDATE edit SET format = 'xmp' WHERE photo = '40'", "edit in an unknown format: \"40\""),
        ("UPDATE collection SET kind = 'quick'", "Quick Collection not named quick collection"),
        (
            "INSERT INTO collection VALUES ('q', 'quick collection', '50', 'quick')",
            "Quick Collection not named quick collection at the top: \"q\"",
        ),
        ("UPDATE collection SET kind = 'set'", "collection set holding photos: \"50\""),
        (
            "INSERT INTO photo VALUES ('42', '21', 'x', 'x', '', 0, 0, '', '', NULL, NULL, '', NULL);
             UPDATE plan SET source_photos = 3;
             INSERT INTO stack VALUES ('s', 1); INSERT INTO stack_photo VALUES ('s', '40', 1), ('s', '42', 2)",
            "stack of photos in different folders: \"s\"",
        ),
        (
            "INSERT INTO photo VALUES ('42', '20', 'x', 'x', '', 0, 0, '', '', NULL, '41', '', NULL);
             UPDATE plan SET source_photos = 3",
            "virtual copy of a virtual copy: \"42\"",
        ),
        (
            "UPDATE photo SET folder = '21' WHERE id = '41'",
            "virtual copy in another folder than its master: \"41\"",
        ),
        ("UPDATE plan SET source_photos = 3", "orphaned image records (2/3); import rolled back"),
    ] {
        fails_with(change, 0, expected);
    }
    fails_with(
        "",
        -1,
        "The importer answered 1 photos, but its plan holds 2",
    );
}

/// The published schema's CHECK constraints are the writer's to keep: one
/// that turned them off writes values only the core's own checks refuse.
#[test]
fn a_plan_written_past_its_checks_is_refused() {
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        p.execute_batch(FIXTURE_PLAN)?;
        Ok(p.execute_batch(
            "PRAGMA ignore_check_constraints = ON;
             UPDATE keyword SET include_on_export = 2 WHERE id = '60';",
        )?)
    })
    .unwrap();
    let error = import.run(photos, &mut |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("export option"), "{error:#}");
}

/// Many plan rows are refused as such, without reading them all.
#[test]
fn a_plan_with_many_plan_rows_is_refused() {
    fails_with(
        "INSERT INTO plan SELECT 1, 0 FROM photo, photo, photo, photo, photo",
        0,
        "has more than one plan row",
    );
}

#[test]
fn texts_are_limited_by_their_bytes_and_a_two_megabyte_develop_text_imports() -> Result<()> {
    // 'é' is two bytes: eight million of them are 16,000,000 bytes.
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        p.execute_batch(FIXTURE_PLAN)?;
        p.execute(
            "UPDATE edit SET text = ? WHERE photo = '40'",
            ["é".repeat(8_000_000)],
        )?;
        Ok(())
    })?;
    let error = import.run(photos, &mut |_| {}).unwrap_err();
    assert!(
        format!("{error:#}").contains("edit row 2 is malformed"),
        "{error:#}"
    );

    let text = format!("s = {{ Exposure2012 = 0.5, {} }}", "x".repeat(2 << 20));
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        p.execute_batch(FIXTURE_PLAN)?;
        p.execute("UPDATE edit SET text = ? WHERE photo = '40'", [&text])?;
        Ok(())
    })?;
    let cat = Catalog::open(import.run(photos, &mut |_| {})?)?;
    let master = cat
        .photos()?
        .into_iter()
        .find(|p| p.master.is_none())
        .unwrap()
        .id;
    assert_eq!(cat.lightroom_develop(master)?, Some(text));
    Ok(())
}

#[test]
fn a_plan_with_any_other_schema_is_refused() -> Result<()> {
    for extra in [
        "CREATE TABLE notes (text TEXT)",
        "CREATE VIEW photos AS SELECT * FROM photo",
        "CREATE TRIGGER t AFTER INSERT ON root BEGIN DELETE FROM root; END",
        "CREATE INDEX photo_folder ON photo (folder)",
        "ALTER TABLE photo ADD COLUMN extra TEXT",
        "DROP TABLE photo_keyword",
    ] {
        let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
            p.execute_batch(FIXTURE_PLAN)?;
            Ok(p.execute_batch(extra)?)
        })?;
        let error = import.run(photos, &mut |_| {}).unwrap_err();
        assert!(
            format!("{error:#}").contains("tables aren't the ones its format has"),
            "{extra}: {error:#}"
        );
    }
    Ok(())
}

/// The plan is attached read-only, the connection runs no extension, and
/// `stop` interrupts any statement, during the import and after it.
#[test]
fn the_plan_is_read_only_and_its_connection_stops_when_asked() -> Result<()> {
    let d = tempfile::tempdir()?;
    // A folder whose name SQLite would read as part of a URI. Windows
    // allows no `?` in a name.
    let folder = d.path().join(if cfg!(windows) {
        "a b#c%d"
    } else {
        "a b?c#d%e"
    });
    std::fs::create_dir(&folder)?;
    let plan = folder.join(PLAN);
    plan_file(&plan, SCHEMA)?.execute_batch(FIXTURE_PLAN)?;
    let mut catalog = Catalog::create(&d.path().join("c.rawmakase"))?;
    // Rows enough for the integrity check to ask `stop`, which it does every
    // thousand cells it reads.
    catalog.db_for_tests().execute_batch(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
         INSERT INTO meta SELECT 'k' || i, '' FROM n",
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let asked = stop.clone();
    catalog.db.with_plan(
        &plan,
        SCHEMA,
        move || asked.load(Ordering::SeqCst),
        |plan| {
            let roots: i64 =
                plan.read_sqlite(sqlite_sql!("SELECT count(*) FROM plan.root"), &[])?[0];
            assert_eq!(roots, 1);
            let write = plan.execute_sqlite(sqlite_sql!("DELETE FROM plan.root"), &[]);
            assert!(format!("{:#}", write.unwrap_err()).contains("readonly"));
            let load = plan.read_sqlite::<i64>(sqlite_sql!("SELECT load_extension('x')"), &[]);
            assert!(load.is_err());
            stop.store(true, Ordering::SeqCst);
            let long = plan.read_sqlite::<i64>(
                sqlite_sql!(
                    "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
                     SELECT count(*) FROM n"
                ),
                &[],
            );
            assert!(format!("{:#}", long.unwrap_err()).contains("interrupt"));
            stop.store(false, Ordering::SeqCst);
            Ok(())
        },
    )?;
    assert!(catalog.db.is_intact()?);
    stop.store(true, Ordering::SeqCst);
    assert!(
        catalog.db.is_intact().is_err(),
        "the integrity check stops too"
    );
    Ok(())
}

#[test]
fn a_cancel_before_the_gate_publishes_nothing_and_after_it_changes_nothing() -> Result<()> {
    let write = |p: &Connection| Ok(p.execute_batch(FIXTURE_PLAN)?);
    // Before the import starts, and while it imports.
    for at in ["", "Importing photos…", "Checking the new catalog…"] {
        let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, write)?;
        let (gate, folder) = (import.gate(), import.plan().parent().unwrap().to_path_buf());
        let destination = import.destination().to_path_buf();
        if at.is_empty() {
            assert!(gate.cancel());
        }
        let error = import
            .run(photos, &mut |text| {
                if text == at {
                    assert!(gate.cancel());
                }
            })
            .unwrap_err();
        assert!(error.is::<Cancelled>(), "{at}: {error:#}");
        assert!(!destination.exists() && !folder.exists(), "{at}");
    }
    // Once publishing has begun, a cancel loses.
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, write)?;
    let gate = import.gate();
    let published = import.run(photos, &mut |text| {
        if text == "Saving the new catalog…" {
            assert!(!gate.cancel());
        }
    })?;
    assert!(Catalog::open(&published).is_ok());
    assert!(!gate.is_cancelled());
    Ok(())
}

#[test]
fn the_gate_moves_once_whichever_side_comes_first() {
    let gate = Gate::default();
    assert!(gate.publish());
    assert!(!gate.cancel() && !gate.is_cancelled());
    let gate = Gate::default();
    assert!(gate.cancel() && gate.cancel());
    assert!(!gate.publish());
    // Raced from many threads: exactly one side wins.
    for _ in 0..100 {
        let gate = Gate::default();
        let cancelled = std::thread::scope(|s| {
            let c = s.spawn(|| gate.cancel());
            let published = gate.publish();
            let cancelled = c.join().unwrap();
            assert_ne!(published, cancelled);
            cancelled
        });
        assert_eq!(gate.is_cancelled(), cancelled);
    }
}

#[test]
fn a_destination_made_meanwhile_is_left_alone() -> Result<()> {
    let (dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        Ok(p.execute_batch(FIXTURE_PLAN)?)
    })?;
    std::fs::write(dir.path().join("new.rawmakase"), "theirs")?;
    let error = import.run(photos, &mut |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("already exists"), "{error:#}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.rawmakase"))?,
        "theirs"
    );
    Ok(())
}

/// On a volume without hard links the catalog is copied: completely, or
/// not at all when quitting stops it, and never over a file made meanwhile.
#[test]
fn publishing_by_copy_deletes_what_it_made_unless_it_finished() -> Result<()> {
    let d = tempfile::tempdir()?;
    let working = d.path().join("working");
    let bytes: Vec<u8> = (0..3 * COPY_BLOCK + 5).map(|i| i as u8).collect();
    std::fs::write(&working, &bytes)?;
    let unsupported = |_: &Path, _: &Path| Err(std::io::Error::from(ErrorKind::Unsupported));
    let destination = d.path().join("copied");
    publish(&working, &destination, &|| false, unsupported)?;
    assert_eq!(std::fs::read(&destination)?, bytes);

    // Quitting after the first block: what it wrote goes.
    let stopped = d.path().join("stopped");
    let looks = std::cell::Cell::new(0);
    let quitting = || {
        looks.set(looks.get() + 1);
        looks.get() > 1
    };
    let error = publish(&working, &stopped, &quitting, unsupported).unwrap_err();
    assert!(error.is::<Cancelled>(), "{error:#}");
    assert!(looks.get() > 1 && !stopped.exists());

    // Another writer took the name after the link was refused.
    let taken = d.path().join("taken");
    let error = publish(&working, &taken, &|| false, |_, to| {
        std::fs::write(to, "theirs")?;
        Err(std::io::Error::from(ErrorKind::Unsupported))
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("already exists"), "{error:#}");
    assert_eq!(std::fs::read_to_string(&taken)?, "theirs");

    // Another writer moved the half-copied file away and put its own in
    // its place: stopping deletes neither.
    {
        let replaced = d.path().join("replaced");
        let moved = d.path().join("moved");
        let looks = std::cell::Cell::new(0);
        let quitting = || {
            looks.set(looks.get() + 1);
            if looks.get() == 2 {
                std::fs::rename(&replaced, &moved).unwrap();
                std::fs::write(&replaced, "theirs").unwrap();
            }
            looks.get() > 2
        };
        let error = publish(&working, &replaced, &quitting, unsupported).unwrap_err();
        assert!(error.is::<Cancelled>(), "{error:#}");
        assert_eq!(std::fs::read_to_string(&replaced)?, "theirs");
    }
    Ok(())
}

/// A plan of 100,000 photos, each with an edit and a History step, imports:
/// the plan's rows go from file to file in SQLite.
#[test]
fn a_100000_photo_plan_imports() -> Result<()> {
    let (_dir, import, photos, _) = prepared(Path::new(SOURCE), SCHEMA, |p| {
        Ok(p.execute_batch(
            "INSERT INTO plan VALUES (1, 100000);
             INSERT INTO root VALUES ('r', '/photos/');
             INSERT INTO folder VALUES ('f', 'r', '2024/');
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
             INSERT INTO photo SELECT 'p' || i, 'f', 'p' || i || '.cr3', '/photos/2024/p' || i || '.cr3',
                 '', 0, 0, '', 'RAW', NULL, NULL, '', NULL FROM n;
             INSERT INTO edit SELECT id, 'lightroom-develop', 's = { Exposure2012 = 0.5 }' FROM photo;
             INSERT INTO history SELECT id, 1, 'Import', NULL, 's = {}' FROM photo;",
        )?)
    })?;
    let started = std::time::Instant::now();
    let cat = Catalog::open(import.run(photos, &mut |_| {})?)?;
    let count = |table: &str| -> Result<i64> {
        Ok(cat
            .db_for_tests()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
    };
    assert_eq!(
        (count("photos")?, count("lightroom_history")?),
        (100_000, 100_000)
    );
    eprintln!("imported in {:?}", started.elapsed());
    Ok(())
}

/// A folder a program still holds a file of (Windows refuses to delete it)
/// is deleted once the program lets go, within the time given; else it is
/// named, to be said.
#[test]
fn discarding_tries_again_until_the_program_lets_go() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("source.lrcat");
    std::fs::write(&source, "")?;
    let destination = d.path().join("new.rawmakase");
    let busy = || Err(std::io::Error::from(ErrorKind::PermissionDenied));

    let import = PlanImport::prepare(&source, &destination)?;
    let folder = import.plan().parent().unwrap().to_path_buf();
    let mut tries = 0;
    let discarded = import.discard_with(std::time::Duration::from_secs(5), |f| {
        tries += 1;
        if tries < 3 {
            busy()
        } else {
            std::fs::remove_dir_all(f)
        }
    });
    assert_eq!(discarded, Ok(()));
    assert_eq!(tries, 3);
    assert!(!folder.exists());

    let import = PlanImport::prepare(&source, &destination)?;
    let folder = import.plan().parent().unwrap().to_path_buf();
    let discarded = import.discard_with(std::time::Duration::from_millis(250), |_| busy());
    assert_eq!(discarded, Err(folder.clone()));
    assert!(folder.exists());
    Ok(())
}
