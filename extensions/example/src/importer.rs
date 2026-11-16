//! A test-only extension of an importer (`importer/rawmakase-extension.toml`):
//! its source is a small JSON file saying what plan to write and how to
//! behave, so the app's tests can drive an import end to end, its failures
//! included.
//!
//! The source, every field optional:
//!
//! ```json
//! {"photos": 3, "source_photos": 3, "delay_ms": 0, "then": "answer"}
//! ```
//!
//! - `photos`: how many photos the plan holds, under one root and folder,
//!   each with an edit and a History step; the first is in a collection and
//!   has a keyword. 1 by default.
//! - `source_photos`: the photos the plan says the source holds; `photos` by
//!   default, so a larger number stands for photos the program left out.
//! - `delay_ms`: how long it works on each photo, reporting progress after
//!   each.
//! - `then`, once the photos are written: `answer` answers with the photos
//!   written; `progress` reports progress every 50 ms until cancelled;
//!   `silent` waits until cancelled without a word; `fail` answers
//!   `unavailable`; `crash` exits.
use rawmakase_extension::protocol::plan::{PLAN_VERSION, SCHEMA};
use rawmakase_extension::{Context, ErrorCode, Extension, Importer, RemoteError};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// The develop text every photo gets: Exposure +0.5.
pub const DEVELOP: &str = "s = { Exposure2012 = 0.5, ProcessVersion = \"11.0\", }";

struct Script;

impl Importer for Script {
    fn import(&mut self, cx: &Context, source: &Path, plan: &Path) -> Result<u64, RemoteError> {
        let unavailable =
            |e: &dyn std::fmt::Display| RemoteError::new(ErrorCode::Unavailable, e.to_string());
        let text = std::fs::read_to_string(source).map_err(|e| unavailable(&e))?;
        let script: Value = serde_json::from_str(&text).map_err(|e| unavailable(&e))?;
        let number = |key: &str, default: i64| script[key].as_i64().unwrap_or(default);
        let photos = number("photos", 1);
        let delay = Duration::from_millis(number("delay_ms", 0).unsigned_abs());
        let db = Connection::open(plan).map_err(|e| unavailable(&e))?;
        let sql = |sql: &str| db.execute_batch(sql).map_err(|e| unavailable(&e));
        sql(SCHEMA)?;
        db.execute(
            "INSERT INTO plan VALUES (?, ?)",
            params![PLAN_VERSION, number("source_photos", photos)],
        )
        .map_err(|e| unavailable(&e))?;
        sql("INSERT INTO root VALUES ('root', '/photos/');
             INSERT INTO folder VALUES ('folder', 'root', '2024/');
             INSERT INTO collection VALUES ('picks', 'Picks', NULL, 'collection');
             INSERT INTO keyword VALUES ('travel', 'Travel', NULL, 1, 1);")?;
        for n in 0..photos {
            if cx.is_cancelled() {
                return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
            }
            let id = format!("photo-{n}");
            db.execute(
                "INSERT INTO photo VALUES (?1, 'folder', ?2, '/photos/2024/' || ?2,
                     '2024-05-01T10:00:00', 0, 0, '', 'JPG', NULL, NULL, '', NULL)",
                params![id, format!("IMG_{n:04}.jpg")],
            )
            .and_then(|_| {
                db.execute(
                    "INSERT INTO edit VALUES (?, 'lightroom-develop', ?)",
                    params![id, DEVELOP],
                )
            })
            .and_then(|_| {
                db.execute(
                    "INSERT INTO history VALUES (?, 1, 'Exposure', 736000000.0, ?)",
                    params![id, DEVELOP],
                )
            })
            .map_err(|e| unavailable(&e))?;
            if n == 0 {
                sql(
                    "INSERT INTO collection_photo VALUES ('picks', 'photo-0', '0');
                     INSERT INTO photo_keyword VALUES ('photo-0', 'travel');",
                )?;
            }
            std::thread::sleep(delay);
            cx.progress(
                Some((n + 1) as f64 / photos as f64),
                Some(&format!("Reading photos… {} of {photos}", n + 1)),
            );
        }
        db.close().map_err(|(_, e)| unavailable(&e))?;
        match script["then"].as_str().unwrap_or("answer") {
            "progress" => {
                while !cx.is_cancelled() {
                    cx.progress(None, Some("Still working"));
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"))
            }
            "silent" => {
                while !cx.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"))
            }
            "fail" => Err(RemoteError::new(
                ErrorCode::Unavailable,
                "the source couldn't be read",
            )),
            "crash" => std::process::exit(3),
            _ => Ok(photos.unsigned_abs()),
        }
    }
}

/// The importer extension's program.
pub fn extension() -> Extension {
    Extension::new().importer("script", Script)
}

#[cfg(test)]
mod tests {
    #[test]
    fn its_manifest_reads() {
        let manifest = rawmakase_extension::protocol::manifest::Manifest::parse(include_str!(
            "../importer/rawmakase-extension.toml"
        ));
        assert_eq!(manifest.unwrap().contributions(), ["importer/script"]);
    }
}
