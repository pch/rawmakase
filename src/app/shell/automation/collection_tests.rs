//! Collections through the control socket, as `rawmakase control` and the
//! MCP server send them (issue 1207): listing them, making a set with a
//! collection of photos in it, adding photos and taking them out, each
//! change one undoable step that leaves the files alone.
use super::{Hub, Settings, socket};
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::{CollectionId, PhotoId};
use serde_json::{Value, json};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Sends `request` through the control socket announced in `data`, running
/// the editor's control requests until it is answered.
fn ask(e: &mut Editor, data: &std::path::Path, request: Value) -> Value {
    let connection = rawmakase_ctl::Connection::read(data).unwrap();
    let reply = std::thread::spawn(move || connection.request(request));
    let ctx = e.context.clone();
    let until = Instant::now() + Duration::from_secs(10);
    while !reply.is_finished() {
        e.control_commands(&ctx);
        assert!(Instant::now() < until, "no reply");
        std::thread::sleep(Duration::from_millis(2));
    }
    reply.join().unwrap().unwrap()
}

/// The editor's catalog photos and a control socket announced in a data
/// folder of its own.
fn editor() -> (tempfile::TempDir, tempfile::TempDir, Editor, Vec<PhotoId>) {
    let (dir, mut e, mut ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"]).unwrap();
    ids.sort();
    let data = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::sync_channel(256);
    e.controls = Hub::new(Settings::default(), rx);
    e.controls.socket = Some(
        socket::start_at(
            tx,
            e.context.clone(),
            rawmakase_protocol::Endpoint::path(data.path()),
        )
        .unwrap(),
    );
    (dir, data, e, ids)
}

fn members(e: &Editor, id: i64) -> Vec<PhotoId> {
    let mut photos: Vec<PhotoId> = e
        .library
        .as_ref()
        .unwrap()
        .session
        .collection_photos()
        .get(&CollectionId(id))
        .map(|m| m.iter().copied().collect())
        .unwrap_or_default();
    photos.sort();
    photos
}

#[test]
fn a_set_of_collections_is_made_and_filled_by_command_with_undo() {
    let (_dir, data, mut e, ids) = editor();
    let id = |p: PhotoId| p.0;
    let files: Vec<_> = e
        .library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| (p.path.clone(), std::fs::read(&p.path).unwrap()))
        .collect();

    let reply = ask(&mut e, data.path(), json!({"cmd":"collections"}));
    assert_eq!(reply["result"]["collections"], json!([]), "{reply}");

    let reply = ask(
        &mut e,
        data.path(),
        json!({"cmd":"create_collection","kind":"set","name":"Kansai trip"}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let set = reply["result"]["created"]["id"].as_i64().unwrap();
    assert_eq!(reply["result"]["created"]["kind"], "set");

    let reply = ask(
        &mut e,
        data.path(),
        json!({"cmd":"create_collection","name":"09-25 Nara","parent_id":set,
               "photo_ids":[id(ids[0]), id(ids[1]), id(ids[0])]}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let day = reply["result"]["created"]["id"].as_i64().unwrap();
    assert_eq!(
        reply["result"]["created"],
        json!({"id": day, "name": "09-25 Nara", "kind": "collection", "parent_id": set,
               "photos": 2, "takes_photos": true})
    );
    assert_eq!(members(&e, day), ids[..2]);

    // The same name beside it is refused, naming the one already there.
    let reply = ask(
        &mut e,
        data.path(),
        json!({"cmd":"create_collection","name":"09-25 Nara","parent_id":set}),
    );
    assert_eq!(reply["code"], "already_exists", "{reply}");
    assert!(reply["error"].as_str().unwrap().contains(&day.to_string()));

    // Added after its photos; one already in it is left as it is.
    let reply = ask(
        &mut e,
        data.path(),
        json!({"cmd":"add_to_collection","collection_id":day,
               "photo_ids":[id(ids[1]), id(ids[2])]}),
    );
    assert_eq!(reply["result"]["added"], 1, "{reply}");
    assert_eq!(reply["result"]["collection"]["photos"], 3);
    let reply = ask(
        &mut e,
        data.path(),
        json!({"cmd":"remove_from_collection","collection_id":day,"photo_ids":[id(ids[0])]}),
    );
    assert_eq!(reply["result"]["removed"], 1, "{reply}");
    assert_eq!(members(&e, day), ids[1..]);

    let reply = ask(&mut e, data.path(), json!({"cmd":"collections"}));
    assert_eq!(
        reply["result"]["collections"],
        json!([
            {"id": set, "name": "Kansai trip", "kind": "set", "parent_id": null,
             "photos": 2, "takes_photos": false},
            {"id": day, "name": "09-25 Nara", "kind": "collection", "parent_id": set,
             "photos": 2, "takes_photos": true},
        ])
    );

    // Each change is one step: Undo takes them back one at a time.
    e.sync_undo();
    e.undo();
    assert_eq!(members(&e, day), ids);
    e.undo();
    assert_eq!(members(&e, day), ids[..2]);
    e.undo();
    let listed = ask(&mut e, data.path(), json!({"cmd":"collections"}));
    assert_eq!(listed["result"]["collections"].as_array().unwrap().len(), 1);
    e.undo();
    let listed = ask(&mut e, data.path(), json!({"cmd":"collections"}));
    assert_eq!(listed["result"]["collections"], json!([]));
    e.redo();
    e.redo();
    assert_eq!(members(&e, day), ids[..2]);

    // The files are as they were.
    for (path, bytes) in files {
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn collection_commands_refuse_what_the_panel_cant_do() {
    let (_dir, data, mut e, ids) = editor();
    let set = ask(
        &mut e,
        data.path(),
        json!({"cmd":"create_collection","kind":"set","name":"Trips"}),
    )["result"]["created"]["id"]
        .as_i64()
        .unwrap();
    let collection = ask(
        &mut e,
        data.path(),
        json!({"cmd":"create_collection","name":"Picks"}),
    )["result"]["created"]["id"]
        .as_i64()
        .unwrap();
    for (request, code) in [
        // A set holds collections, not photos.
        (
            json!({"cmd":"create_collection","kind":"set","name":"x","photo_ids":[ids[0].0]}),
            "invalid_request",
        ),
        (
            json!({"cmd":"add_to_collection","collection_id":set,"photo_ids":[ids[0].0]}),
            "invalid_request",
        ),
        // Only a set holds collections.
        (
            json!({"cmd":"create_collection","name":"x","parent_id":collection}),
            "invalid_request",
        ),
        (
            json!({"cmd":"create_collection","name":"x","parent_id":9999}),
            "not_found",
        ),
        (
            json!({"cmd":"add_to_collection","collection_id":9999,"photo_ids":[ids[0].0]}),
            "not_found",
        ),
        // A photo not in the catalog changes nothing, the others included.
        (
            json!({"cmd":"add_to_collection","collection_id":collection,
                   "photo_ids":[ids[0].0, 9999]}),
            "not_found",
        ),
        (
            json!({"cmd":"create_collection","name":"x","photo_ids":[9999]}),
            "not_found",
        ),
        (
            json!({"cmd":"create_collection","name":"  "}),
            "not_applied",
        ),
    ] {
        let reply = ask(&mut e, data.path(), request.clone());
        assert_eq!(reply["code"], code, "{request}: {reply}");
    }
    assert!(members(&e, collection).is_empty());
    let listed = ask(&mut e, data.path(), json!({"cmd":"collections"}));
    assert_eq!(listed["result"]["collections"].as_array().unwrap().len(), 2);
}
