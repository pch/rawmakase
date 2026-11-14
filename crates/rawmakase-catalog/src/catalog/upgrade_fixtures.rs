//! Catalogs of earlier formats as their releases made them, for the upgrade's
//! tests: the schema each format was created with (copied from the release that
//! created it), and edits, Histories and snapshots holding their camera profiles
//! and Lensfun calibrations inline, as those releases stored them.
use super::{Catalog, PhotoId};
use crate::model::recipe::Recipe;
use crate::storage::Identity;
use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The schema a catalog of `version` was created with: version 1 before raster
/// masks, version 2 before the one engine, version 6 before profiles were stored
/// once. Each lacks tables added since.
fn schema(version: i64) -> &'static str {
    match version {
        1 => include_str!("upgrade_fixtures/schema-v1.sql"),
        2 => include_str!("upgrade_fixtures/schema-v2.sql"),
        _ => include_str!("upgrade_fixtures/schema-v6.sql"),
    }
}

/// A catalog of an earlier format with photos, closed.
pub struct Earlier {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
    /// Each photo's id and file, in the order made.
    pub photos: Vec<(PhotoId, PathBuf)>,
    db: Option<Connection>,
}

impl Earlier {
    /// A catalog of format `version` (1, 2 or 6) with `count` photos, each a copy
    /// of the synthetic chart DNG, and no edits yet.
    pub fn new(version: i64, count: usize) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let photos_dir = dir.path().join("photos");
        std::fs::create_dir(&photos_dir)?;
        let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/synthetic-d65.dng");
        let path = dir.path().join(format!("Format {version}.rawmakase"));
        let db = Connection::open(&path)?;
        db.execute_batch(&format!(
            "PRAGMA application_id={}; PRAGMA user_version={version};",
            0x4f4d4152
        ))?;
        db.execute_batch(schema(version))?;
        db.execute(
            "INSERT INTO roots(id, original_path) VALUES (1, ?1)",
            [photos_dir.to_string_lossy()],
        )?;
        db.execute(
            "INSERT INTO folders(id, root, relative_path) VALUES (1, 1, '')",
            [],
        )?;
        let mut photos = Vec::new();
        for n in 0..count {
            let name = format!("p{n:05}.dng");
            let file = photos_dir.join(&name);
            std::fs::copy(&chart, &file)?;
            db.execute(
                "INSERT INTO photos(id, folder, filename, original_path, format)
                 VALUES (?1, 1, ?2, ?3, 'DNG')",
                rusqlite::params![n as i64 + 1, name, file.to_string_lossy()],
            )?;
            photos.push((PhotoId(n as i64 + 1), file));
        }
        Ok(Self {
            dir,
            path,
            photos,
            db: Some(db),
        })
    }
    fn db(&self) -> &Connection {
        self.db.as_ref().expect("open until closed")
    }
    /// Stores `text` as photo `n`'s edit, saved for its file as it is.
    pub fn set_edit_text(&self, n: usize, text: &str) -> Result<()> {
        let (id, file) = &self.photos[n];
        let identity = serde_json::to_string(&Identity::read(file)?)?;
        let export = serde_json::to_string(&crate::export_settings::ExportOptions::default())?;
        self.db().execute(
            "UPDATE photos SET recipe=?1, export_options=?2, identity=?3,
             edited_at='2026-10-01 10:00:00' WHERE id=?4",
            rusqlite::params![text, export, identity, id.0],
        )?;
        Ok(())
    }
    /// Stores `recipe` as photo `n`'s edit, inline as earlier releases stored it,
    /// with a copy of an applied preset's settings.
    pub fn set_edit(&self, n: usize, recipe: &Recipe) -> Result<()> {
        self.set_edit_text(n, &inline(recipe).to_string())
    }
    /// Makes photo `n` a virtual copy of photo `of`, with a copy of its edit.
    pub fn make_copy(&self, n: usize, of: usize) -> Result<()> {
        let (id, _) = &self.photos[n];
        let (master, _) = &self.photos[of];
        self.db().execute(
            "UPDATE photos SET master_id=?1, copy_name='Copy 1',
             (recipe, export_options, identity, edited_at) =
             (SELECT recipe, export_options, identity, edited_at FROM photos WHERE id=?1)
             WHERE id=?2",
            rusqlite::params![master.0, id.0],
        )?;
        Ok(())
    }
    /// Stores `data` as photo `n`'s History, as it is.
    pub fn set_history_data(&self, n: usize, data: &[u8]) -> Result<()> {
        self.db().execute(
            "INSERT INTO develop_history(photo, data) VALUES (?1, ?2)
             ON CONFLICT(photo) DO UPDATE SET data=excluded.data",
            rusqlite::params![self.photos[n].0.0, data],
        )?;
        Ok(())
    }
    /// Stores `states` (origin first) as photo `n`'s History, every step applied,
    /// as earlier releases encoded it: settings longer than 256 bytes pooled once,
    /// profiles inline.
    pub fn set_history(&self, n: usize, states: &[Recipe]) -> Result<()> {
        self.set_history_data(n, &compress(&inline_history(states)))
    }
    /// Adds a snapshot of photo `n` holding `text`.
    pub fn add_snapshot_text(&self, n: usize, name: &str, text: &str) -> Result<()> {
        self.db().execute(
            "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.photos[n].0.0, name, text],
        )?;
        Ok(())
    }
    /// Adds a snapshot of photo `n` holding `recipe` inline.
    pub fn add_snapshot(&self, n: usize, name: &str, recipe: &Recipe) -> Result<()> {
        self.add_snapshot_text(n, name, &inline(recipe).to_string())
    }
    /// Runs `f` on the catalog's connection, to set up what the helpers don't.
    pub fn with_db<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        f(self.db())
    }
    /// Closes the catalog, so it can be opened as a user would.
    pub fn close(&mut self) {
        self.db = None;
    }
    /// Closes the catalog and opens it with this release.
    pub fn open(&mut self) -> Result<Catalog> {
        self.close();
        Catalog::open(&self.path)
    }
}

/// `recipe` inline, as releases before profiles were stored once wrote it, with a
/// copy of an applied preset's settings.
pub fn inline(recipe: &Recipe) -> serde_json::Value {
    let mut value = serde_json::to_value(recipe).expect("a recipe serializes");
    value["preset_settings"] = serde_json::json!({"Exposure2012": "+0.50", "Look": "Vivid"});
    value
}

/// A History of `states` (origin first, every step applied), as earlier releases
/// encoded it, as JSON.
pub fn inline_history(states: &[Recipe]) -> serde_json::Value {
    let mut pool: Vec<serde_json::Value> = Vec::new();
    let mut index = std::collections::HashMap::new();
    let mut state = |r: &Recipe| {
        let serde_json::Value::Object(all) = inline(r) else {
            unreachable!("a recipe is an object")
        };
        let mut fields = serde_json::Map::new();
        let mut pooled = serde_json::Map::new();
        for (key, value) in all {
            let text = value.to_string();
            if text.len() <= 256 {
                fields.insert(key, value);
                continue;
            }
            let n = *index.entry(text).or_insert_with(|| {
                pool.push(value);
                pool.len() - 1
            });
            pooled.insert(key, n.into());
        }
        let mut stored = serde_json::json!({ "fields": fields });
        if !pooled.is_empty() {
            stored["pooled"] = pooled.into();
        }
        stored
    };
    let origin = state(&states[0]);
    let steps: Vec<serde_json::Value> = states[1..]
        .iter()
        .enumerate()
        .map(|(i, r)| {
            serde_json::json!({"name": format!("Step {i}"), "value": format!("{i}"), "state": state(r)})
        })
        .collect();
    let applied = steps.len();
    serde_json::json!({"version": 1, "pool": pool, "origin": origin, "steps": steps, "applied": applied})
}

/// `value` zlib-compressed, as a History is stored.
pub fn compress(value: &serde_json::Value) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(value.to_string().as_bytes())
        .and_then(|()| z.finish())
        .expect("compressing into memory")
}

/// A stored History's JSON.
pub fn decompress(data: &[u8]) -> serde_json::Value {
    use std::io::Read;
    let mut text = String::new();
    flate2::read::ZlibDecoder::new(data)
        .read_to_string(&mut text)
        .expect("a stored History inflates");
    serde_json::from_str(&text).expect("a stored History is JSON")
}
