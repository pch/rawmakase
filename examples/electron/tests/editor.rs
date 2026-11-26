//! The editor as the Node addon drives it, on a color-chart DNG: edits by a
//! control's name, the curve, a change of view or photo.
//!
//! Whether a draft comes first is the library's rule, by how long the last
//! frames took, so these tests skip drafts and wait for frames; the render
//! thread's handling of a draft is tested in `scheduling.rs`.
use rawmakase_electron::editor::{Config, Editor, Error, Update};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const SOON: Duration = Duration::from_secs(30);

fn chart() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng")
}

/// A frame as the tests look at it.
#[derive(Debug, PartialEq)]
struct Frame {
    edit: u32,
    draft: bool,
    edge: u32,
}

fn editor(home: &Path) -> (Editor, Receiver<Frame>) {
    let (frames_tx, frames) = channel();
    let config = Config {
        data_dir: home.join("data"),
        cache_dir: home.join("cache"),
        build_id: "test".into(),
    };
    let editor = Editor::new(config, move |update| match update {
        Update::Frame {
            edit,
            draft,
            pixels,
            ..
        } => {
            assert_eq!(
                pixels.rgba.len(),
                (pixels.width * pixels.height * 4) as usize
            );
            let edge = pixels.width.max(pixels.height);
            let _ = frames_tx.send(Frame { edit, draft, edge });
        }
        Update::Failed { edit, message } => panic!("edit {edit} failed to render: {message}"),
    })
    .unwrap();
    (editor, frames)
}

/// The next frame of `edit` that is not a draft, skipping what came before it.
fn frame_of(frames: &Receiver<Frame>, edit: u32) -> Frame {
    loop {
        let frame = frames.recv_timeout(SOON).expect("a frame");
        if frame.edit == edit && !frame.draft {
            return frame;
        }
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_photo_opens_with_its_controls_and_renders_once_the_view_is_known() {
    let home = tempfile::tempdir().unwrap();
    let (editor, frames) = editor(home.path());
    assert_eq!(json(&editor.describe())["photo"], Value::Null);
    let state = json(&editor.open(&chart(), 1).unwrap());
    assert_eq!(state["photo"]["name"], "synthetic-d65.dng");
    assert_eq!(
        (&state["photo"]["width"], &state["photo"]["height"]),
        (&970.into(), &742.into())
    );
    let names: Vec<&str> = state["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|control| control["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "temperature",
            "tint",
            "exposure",
            "contrast",
            "highlights",
            "shadows",
            "whites",
            "blacks"
        ]
    );
    let exposure = &state["controls"][2];
    assert_eq!(
        (
            &exposure["label"],
            &exposure["min"],
            &exposure["max"],
            &exposure["text"]
        ),
        (
            &"Exposure".into(),
            &(-5.0).into(),
            &5.0.into(),
            &"0.00".into()
        )
    );
    assert_eq!(state["controls"][0]["scale"], "reciprocal");
    assert_eq!(state["curve"]["points"], json("[[0.0,0.0],[1.0,1.0]]"));
    let samples = state["curve"]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 65);
    assert_eq!(
        samples[32],
        json("[0.5,0.5]"),
        "the straight curve, sampled"
    );
    // Nothing renders until the interface says how large the view is.
    assert!(frames.recv_timeout(Duration::from_millis(200)).is_err());
    editor.set_view(400, 1);
    assert_eq!(
        frame_of(&frames, 1),
        Frame {
            edit: 1,
            draft: false,
            edge: 400
        }
    );
}

#[test]
fn edits_cross_as_a_name_and_a_value_and_come_back_as_the_controls_state() {
    let home = tempfile::tempdir().unwrap();
    let (editor, frames) = editor(home.path());
    editor.open(&chart(), 1).unwrap();
    editor.set_view(300, 1);
    let control = json(&editor.set_parameter("exposure", 0.5, 2).unwrap());
    assert_eq!(
        (&control["value"], &control["text"]),
        (&0.5.into(), &"+0.50".into())
    );
    let control = json(&editor.set_parameter("temperature", 4000., 3).unwrap());
    assert_eq!(control["text"], "4000");
    frame_of(&frames, 3);
    let reset = json(&editor.reset_parameter("exposure", 4).unwrap());
    assert_eq!(reset["text"], "0.00");

    let added = json(&editor.curve_insert(0.5, 0.4, 5).unwrap());
    assert_eq!(
        (&added["index"], &added["added"]),
        (&1.into(), &true.into())
    );
    assert_eq!(added["curve"]["points"].as_array().unwrap().len(), 3);
    // A point already at that input is the one to drag.
    let again = json(&editor.curve_insert(0.5, 0.9, 5).unwrap());
    assert_eq!(
        (&again["index"], &again["added"]),
        (&1.into(), &false.into())
    );
    assert_eq!(again["curve"], added["curve"]);
    // A point stays between its neighbours however far it is dragged.
    let moved = json(&editor.curve_move(1, 2., 0.3, 6).unwrap());
    let x = moved["points"][1][0].as_f64().unwrap();
    assert!(x > 0.99 && x < 1., "{x}");
    let removed = json(&editor.curve_remove(1, 7).unwrap());
    assert_eq!(removed["points"].as_array().unwrap().len(), 2);
    frame_of(&frames, 7);

    assert!(matches!(
        editor.set_parameter("no such control", 0., 8),
        Err(Error::UnknownControl(_))
    ));
    match editor.set_parameter("exposure", 99., 8) {
        Err(Error::Library(e)) => assert_eq!(e.kind(), rawmakase_core::ErrorKind::Invalid),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        editor.curve_move(9, 0.5, 0.5, 8),
        Err(Error::NoSuchPoint(9))
    ));
}

/// A new view or photo cancels the renders of the old one; whatever of them was
/// stopped shows nothing and fails nothing (the sink above panics on a failure).
#[test]
fn a_change_of_view_or_photo_leaves_only_the_new_ones_frames() {
    let home = tempfile::tempdir().unwrap();
    let (editor, frames) = editor(home.path());
    editor.open(&chart(), 1).unwrap();
    for (edge, edit) in [(900, 1), (850, 2), (800, 3), (333, 4)] {
        editor.set_view(edge, edit);
        editor
            .set_parameter("contrast", edge as f32 / 2000., edit)
            .unwrap();
    }
    assert_eq!(frame_of(&frames, 4).edge, 333);
    let other = chart().with_file_name("synthetic-d50.dng");
    let state = json(&editor.open(&other, 5).unwrap());
    assert_eq!(state["photo"]["name"], "synthetic-d50.dng");
    assert_eq!(
        state["controls"][3]["text"], "0",
        "the new photo's own recipe"
    );
    assert_eq!(frame_of(&frames, 5).edge, 333);
    assert!(
        frames.recv_timeout(Duration::from_millis(300)).is_err(),
        "nothing stale follows"
    );
    // A photo that cannot be opened leaves the one that is open.
    assert!(editor.open(Path::new("/nonexistent.dng"), 6).is_err());
    assert_eq!(
        json(&editor.describe())["photo"]["name"],
        "synthetic-d50.dng"
    );
}
