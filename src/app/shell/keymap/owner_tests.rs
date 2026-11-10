//! Shortcuts have one reader of key events: [`super::FrameKeys`]. The app's
//! other sources may read keys only where a key is fixed to a widget or a
//! gesture, as listed here.
use std::path::{Path, PathBuf};

/// The ways egui's key events are read.
const READS: [&str; 4] = [
    "key_pressed(",
    "consume_key(",
    "count_and_consume_key(",
    "Event::Key {",
];

/// The reads allowed outside the key map: the file, the read and how many
/// times it appears there, with the reason.
const FIXED: &[(&str, &str, usize)] = &[
    // Dialogs: Enter confirms; a text field's Enter or Escape when it lets go.
    ("develop/settings_transfer.rs", "consume_key(", 1),
    ("export/dialog.rs", "consume_key(", 1),
    ("export/naming/editor.rs", "consume_key(", 1),
    ("export/naming/presets.rs", "consume_key(", 1),
    ("file_ops/rename.rs", "consume_key(", 1),
    ("file_ops/rename.rs", "key_pressed(", 2),
    ("file_ops/dialogs.rs", "key_pressed(", 1),
    ("develop/panels/curve_menu.rs", "key_pressed(", 1),
    ("develop/panels/snapshots.rs", "key_pressed(", 1),
    ("presets/user_presets.rs", "key_pressed(", 1),
    ("library/metadata_fields.rs", "key_pressed(", 1),
    // A mask's name field keeps the old name on Escape.
    ("develop/tools/mask_tool.rs", "key_pressed(", 1),
    // A popup's own Escape.
    ("library/metadata_bar.rs", "key_pressed(", 1),
    // Widgets: the curve point's Delete, a focused number's arrows, the
    // hovered slider's Up and Down, and the destructive choice swallowing keys
    // held from before it opened.
    ("ui/widgets.rs", "key_pressed(", 3),
    ("ui/widgets.rs", "count_and_consume_key(", 2),
    ("ui/widgets.rs", "Event::Key {", 1),
    // Any key stops the grid's glide or cancels a pending wheel or rail click.
    ("library/kinetic.rs", "Event::Key {", 1),
    ("develop/tools/brush_scroll.rs", "Event::Key {", 1),
    ("develop/panels/color_grading.rs", "Event::Key {", 2),
    // The raw-input hook that turns Copy and Paste with Shift or Option back
    // into keys, and the macOS menu handing its keys to a text field: both make
    // events rather than read them.
    ("shell/workspace.rs", "Event::Key {", 1),
    ("shell/native_menu.rs", "Event::Key {", 1),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the app's sources") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `relative` holds tests only.
fn test_file(relative: &str) -> bool {
    relative.ends_with("_tests.rs")
        || relative.ends_with("/tests.rs")
        || relative.contains("/tests/")
        || relative == "testing.rs"
        // The keymap pins' shared helpers, compiled for tests only.
        || relative.ends_with("_harness.rs")
        || relative.starts_with("shell/keymap/")
}

/// `source` up to its unit tests, a `#[cfg(test)]` module at its end.
fn without_tests(source: &str) -> &str {
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        if line.trim_end() == "#[cfg(test)]" {
            let rest = &source[at + line.len()..];
            let next = rest.lines().next().unwrap_or_default();
            if next.starts_with("mod ") && next.trim_end().ends_with('{') {
                return &source[..at];
            }
        }
        at += line.len();
    }
    source
}

fn count(source: &str, read: &str) -> usize {
    let n = source.matches(read).count();
    // `count_and_consume_key(` holds `consume_key(`.
    if read == "consume_key(" {
        n - source.matches("count_and_consume_key(").count()
    } else {
        n
    }
}

#[test]
fn shortcuts_read_keys_only_through_the_key_map() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    let mut files = Vec::new();
    sources(&app, &mut files);
    let mut found = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&app)
            .expect("under src/app")
            .to_string_lossy()
            .replace('\\', "/");
        if test_file(&relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a source file");
        let source = without_tests(&source);
        for read in READS {
            let n = count(source, read);
            if n > 0 {
                found.push((relative.clone(), read, n));
            }
        }
    }
    found.sort();
    let mut fixed: Vec<(String, &str, usize)> = FIXED
        .iter()
        .map(|(file, read, n)| (file.to_string(), *read, *n))
        .collect();
    fixed.sort();
    assert_eq!(
        found, fixed,
        "a key read outside the key map: take the press with `FrameKeys`, or list a \
         widget's fixed key here with its reason"
    );
}

#[test]
fn a_test_module_at_the_end_is_left_out() {
    let source = "fn a() {}\n#[cfg(test)]\nfn helper() {}\n#[cfg(test)]\nmod tests {\n}\n";
    assert_eq!(
        without_tests(source),
        "fn a() {}\n#[cfg(test)]\nfn helper() {}\n"
    );
}
