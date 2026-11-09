//! The key map file through its owner: what it reads, what it keeps and how it
//! fails. Failures come from [`Faulty`], a [`Disk`] over a temporary folder that
//! fails when told to.
use super::*;
use crate::app::shell::commands::registry::bindings;
use crate::app::shell::keymap::Mods;
use eframe::egui::Key;
use std::sync::{Arc as Shared, Mutex};

/// How the next write fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteFault {
    /// Before anything is put in place, as when the folder is full.
    Before,
    /// After the new file is in place, as when syncing the folder fails.
    AfterPublishing,
}

#[derive(Default)]
struct Faults {
    write: Option<WriteFault>,
    /// Moving the file aside fails.
    move_aside: bool,
    /// Reading fails (other than with the file missing).
    read: bool,
}

/// The file system, failing as [`Faults`] says.
struct Faulty(Shared<Mutex<Faults>>);

impl Disk for Faulty {
    fn read(&self, path: &Path) -> io::Result<String> {
        if self.0.lock().unwrap().read {
            return Err(io::Error::other("the disk is unreadable"));
        }
        Files.read(path)
    }
    fn write(&self, path: &Path, text: &str) -> anyhow::Result<()> {
        match self.0.lock().unwrap().write.take() {
            Some(WriteFault::Before) => anyhow::bail!("no space left"),
            Some(WriteFault::AfterPublishing) => {
                std::fs::write(path, text)?;
                anyhow::bail!("the folder could not be synced")
            }
            None => Files.write(path, text),
        }
    }
    fn move_aside(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.0.lock().unwrap().move_aside {
            return Err(io::Error::other("permission denied"));
        }
        Files.move_aside(from, to)
    }
}

struct Setup {
    dir: tempfile::TempDir,
    faults: Shared<Mutex<Faults>>,
}

impl Setup {
    /// A folder whose keymap.toml holds `text`, or none.
    fn new(text: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("a temporary folder");
        if let Some(text) = text {
            std::fs::write(dir.path().join("keymap.toml"), text).expect("written");
        }
        Self {
            dir,
            faults: Shared::default(),
        }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("keymap.toml")
    }
    fn open(&self) -> KeymapFile {
        KeymapFile::load_with(self.path(), Box::new(Faulty(self.faults.clone())))
    }
    fn text(&self) -> Option<String> {
        std::fs::read_to_string(self.path()).ok()
    }
    fn fail_write(&self, fault: WriteFault) {
        self.faults.lock().unwrap().write = Some(fault);
    }
}

fn chord(text: &str) -> Chord {
    Chord::parse(text).expect("a chord")
}
/// The chords `binding` runs on with `file`'s key map.
fn keys_of(file: &KeymapFile, binding: &crate::app::shell::keymap::Binding) -> Vec<String> {
    file.effective()
        .defaults_of(binding)
        .iter()
        .flat_map(|d| d.chords().collect::<Vec<_>>())
        .map(|c| c.to_string())
        .collect()
}

#[test]
fn with_no_file_the_defaults_run_and_the_first_change_writes_one() {
    let setup = Setup::new(None);
    let mut file = setup.open();
    assert_eq!(*file.state(), State::Absent);
    assert!(file.editable());
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
    assert_eq!(file.set("crop", &[chord("F9")]), Ok(Saved::Durably));
    assert_eq!(keys_of(&file, &bindings::CROP), ["F9"]);
    let text = setup.text().expect("a file");
    assert!(text.contains("version = 1"), "{text}");
    assert!(text.contains("[bindings]\ncrop = [\"F9\"]"), "{text}");
    // It reads back as written.
    assert_eq!(keys_of(&setup.open(), &bindings::CROP), ["F9"]);
}

#[test]
fn an_override_replaces_one_commands_keys_in_every_context() {
    let setup = Setup::new(Some(
        "version = 1\n[bindings]\ndelete = [\"Cmd+Backspace\"]\n",
    ));
    let file = setup.open();
    for b in [
        &bindings::GRID_DELETE,
        &bindings::LOUPE_DELETE,
        &bindings::DEVELOP_DELETE,
    ] {
        assert_eq!(keys_of(&file, b), ["Cmd+Backspace"], "{:?}", b.context);
    }
    // Commands the file doesn't name keep this release's defaults, so new
    // defaults arrive.
    assert_eq!(
        keys_of(&file, &bindings::GRID_TRASH),
        ["Cmd+Alt+Shift+Delete", "Cmd+Alt+Shift+Backspace"]
    );
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
}

#[test]
fn an_empty_list_leaves_a_command_with_no_key() {
    let file = Setup::new(Some("version = 1\n[bindings]\ncompare = []\n")).open();
    assert_eq!(keys_of(&file, &bindings::BEFORE_ONLY), Vec::<String>::new());
    assert!(file.effective().overrides("compare"));
}

#[test]
fn unknown_entries_and_comments_survive_a_change() {
    let text = "\
# My keys.
version = 1
theme = \"dark\" # a newer release's

[bindings]
# Crop where my hand is.
crop = [\"F9\"]
\"someone/extension\" = [\"F10\"]
\"a_newer_command\" = [\"F11\"]
";
    let setup = Setup::new(Some(text));
    let mut file = setup.open();
    assert_eq!(file.unknown(), ["someone/extension", "a_newer_command"]);
    file.set("clipping", &[chord("F12")]).expect("saved");
    let written = setup.text().expect("a file");
    assert_eq!(
        written,
        format!("{text}clipping = [\"F12\"]\n"),
        "everything else stays as it was"
    );
    file.reset("crop").expect("saved");
    let written = setup.text().expect("a file");
    assert!(!written.contains("crop ="), "{written}");
    assert!(written.contains("\"someone/extension\" = [\"F10\"]"));
    assert!(written.contains("theme = \"dark\" # a newer release's"));
    assert!(written.contains("# My keys."));
}

#[test]
fn reset_all_empties_the_bindings_and_keeps_the_rest() {
    let setup = Setup::new(Some(
        "version = 1\ntheme = \"dark\"\n[bindings]\ncrop = [\"F9\"]\nclipping = []\n",
    ));
    let mut file = setup.open();
    assert_eq!(file.reset_all(), Ok(Saved::Durably));
    assert_eq!(
        setup.text().unwrap(),
        "version = 1\ntheme = \"dark\"\n[bindings]\n"
    );
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
    assert_eq!(keys_of(&file, &bindings::CLIPPING), ["J"]);
}

/// Each kind of file this release doesn't read: the defaults run, it is
/// reported, and nothing but Reset All writes it.
#[test]
fn a_file_that_doesnt_read_is_reported_used_by_no_one_and_never_written() {
    for (text, newer) in [
        ("version = 1\n[bindings\ncrop = [\"F9\"]\n", false),
        ("version = 2\n[bindings]\ncrop = [\"F9\"]\n", true),
        ("version = \"1\"\n[bindings]\ncrop = [\"F9\"]\n", false),
        ("version = 1\nbindings = []\n", false),
        ("version = 1\n[[bindings]]\ncrop = [\"F9\"]\n", false),
        ("version = 0\n", false),
    ] {
        let setup = Setup::new(Some(text));
        let mut file = setup.open();
        match file.state() {
            State::Unreadable(Unreadable::Newer(2)) => assert!(newer, "{text}"),
            State::Unreadable(Unreadable::Malformed(_)) => assert!(!newer, "{text}"),
            other => panic!("{text}: {other:?}"),
        }
        assert!(!file.editable());
        assert!(
            file.report()
                .is_some_and(|r| r.contains("keymap.toml isn't used"))
        );
        assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"], "{text}");
        assert!(file.set("crop", &[chord("F8")]).is_err());
        assert!(file.reset("crop").is_err());
        assert_eq!(setup.text().as_deref(), Some(text), "byte for byte");
    }
}

#[test]
fn a_bad_entry_skips_its_command_alone_and_is_kept() {
    let text = "\
version = 1
[bindings]
crop = [1]
compare = \"R\"
clipping = [\"Hyper+R\"]
zoom = [\"Cmd+C\"]
info = [\"F9\"]
";
    let setup = Setup::new(Some(text));
    let mut file = setup.open();
    assert_eq!(*file.state(), State::Read);
    let skipped: Vec<&str> = file.skipped().iter().map(|s| s.id.as_str()).collect();
    assert_eq!(skipped, ["crop", "compare", "clipping", "zoom"]);
    let report = file.report().expect("reported");
    assert!(report.contains("crop and 3 others"), "{report}");
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
    assert_eq!(keys_of(&file, &bindings::CLIPPING), ["J"]);
    assert_eq!(keys_of(&file, &bindings::DEVELOP_INFO), ["F9"]);
    // Changing another command keeps them as written.
    file.set("white_balance", &[chord("F10")]).expect("saved");
    let written = setup.text().unwrap();
    for line in text.lines().skip(2) {
        assert!(written.contains(line), "{line} in {written}");
    }
    // Giving one of them keys replaces its entry.
    file.set("crop", &[chord("F11")]).expect("saved");
    assert!(setup.text().unwrap().contains("crop = [\"F11\"]"));
    assert_eq!(keys_of(&file, &bindings::CROP), ["F11"]);
}

#[test]
fn a_refused_chord_changes_nothing() {
    let setup = Setup::new(None);
    let mut file = setup.open();
    // Select All is the text fields'; Shift goes on to the next photo after a
    // rating; Undo is fixed.
    for (id, keys) in [
        ("crop", "Cmd+A"),
        ("rating:3", "Shift+F9"),
        ("undo", "F9"),
        ("preferences", "P"),
        ("crop", "Up"),
    ] {
        let refused = file.set(id, &[chord(keys)]);
        assert!(refused.is_err(), "{id} on {keys}");
    }
    assert_eq!(setup.text(), None);
    assert_eq!(*file.effective(), Effective::default());
}

#[test]
fn a_write_that_fails_before_publishing_reverts_the_change() {
    let setup = Setup::new(Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n"));
    let mut file = setup.open();
    setup.fail_write(WriteFault::Before);
    let result = file.set("crop", &[chord("F10")]);
    assert!(result.is_err_and(|e| e.contains("no space left")));
    assert_eq!(keys_of(&file, &bindings::CROP), ["F9"]);
    assert_eq!(
        setup.text().as_deref(),
        Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n")
    );
}

#[test]
fn a_write_that_fails_after_publishing_keeps_the_change_and_says_so() {
    let setup = Setup::new(Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n"));
    let mut file = setup.open();
    setup.fail_write(WriteFault::AfterPublishing);
    let result = file.set("crop", &[chord("F10")]);
    assert_eq!(
        result,
        Ok(Saved::MaybeNotDurably(
            "the folder could not be synced".into()
        ))
    );
    assert_eq!(keys_of(&file, &bindings::CROP), ["F10"]);
    assert!(setup.text().unwrap().contains("F10"));
}

#[test]
fn a_failed_first_write_leaves_no_file_the_defaults_and_a_retry() {
    let setup = Setup::new(None);
    let mut file = setup.open();
    setup.fail_write(WriteFault::Before);
    assert!(file.set("crop", &[chord("F9")]).is_err());
    assert_eq!(*file.state(), State::Absent);
    assert!(file.editable());
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
    assert_eq!(setup.text(), None);
    assert_eq!(file.set("crop", &[chord("F9")]), Ok(Saved::Durably));
    assert_eq!(keys_of(&file, &bindings::CROP), ["F9"]);
}

#[test]
fn a_failed_write_whose_file_wont_read_back_leaves_the_defaults() {
    let setup = Setup::new(Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n"));
    let mut file = setup.open();
    setup.fail_write(WriteFault::AfterPublishing);
    setup.faults.lock().unwrap().read = true;
    assert!(file.set("crop", &[chord("F10")]).is_err());
    assert!(matches!(
        file.state(),
        State::Unreadable(Unreadable::Malformed(_))
    ));
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
}

#[test]
fn reset_all_moves_a_broken_file_aside_without_clobbering() {
    let broken = "version = 1\n[bindings\n";
    let setup = Setup::new(Some(broken));
    std::fs::write(setup.dir.path().join("keymap.toml.broken"), "an older one").unwrap();
    let mut file = setup.open();
    assert!(!file.editable());
    assert_eq!(file.reset_all(), Ok(Saved::Durably));
    assert_eq!(*file.state(), State::Read);
    assert!(file.editable());
    let read = |name: &str| std::fs::read_to_string(setup.dir.path().join(name)).unwrap();
    assert_eq!(read("keymap.toml.broken"), "an older one");
    assert_eq!(read("keymap.toml.broken-2"), broken);
    assert!(setup.text().unwrap().contains("version = 1"));
    file.set("crop", &[chord("F9")]).expect("editable again");
}

#[test]
fn reset_all_that_fails_after_the_move_leaves_the_defaults_and_the_moved_file() {
    let broken = "version = 3\n";
    let setup = Setup::new(Some(broken));
    let mut file = setup.open();
    setup.fail_write(WriteFault::Before);
    let result = file.reset_all();
    assert!(result.is_err_and(|e| e.contains("keymap.toml.broken")));
    assert_eq!(*file.state(), State::Absent);
    assert!(file.editable());
    assert_eq!(setup.text(), None);
    let moved = std::fs::read_to_string(setup.dir.path().join("keymap.toml.broken")).unwrap();
    assert_eq!(moved, broken);
}

#[test]
fn reset_all_that_cant_move_the_file_leaves_it_alone() {
    let broken = "version = 3\n";
    let setup = Setup::new(Some(broken));
    let mut file = setup.open();
    setup.faults.lock().unwrap().move_aside = true;
    assert!(file.reset_all().is_err());
    assert!(!file.editable());
    assert_eq!(setup.text().as_deref(), Some(broken));
}

#[test]
fn a_test_editors_key_map_never_touches_a_file() {
    let mut file = KeymapFile::defaults();
    assert_eq!(file.path(), None);
    assert_eq!(file.set("crop", &[chord("F9")]), Ok(Saved::Durably));
    assert_eq!(keys_of(&file, &bindings::CROP), ["F9"]);
    assert_eq!(
        file.effective().user_chords("crop"),
        Some(&[Chord::new(Key::F9, Mods::NONE)][..])
    );
}

/// The user's guide, docs/shortcuts.md: its example reads as it says, and it
/// lists every command's id.
#[test]
fn the_shortcuts_guide_matches_the_key_map() {
    let guide = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/shortcuts.md"),
    )
    .expect("the guide");
    let example = guide
        .split("```toml\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("an example");
    let parsed = match parse(example) {
        Ok(parsed) => parsed,
        Err(why) => panic!("the example doesn't read: {why}"),
    };
    assert_eq!(parsed.skipped, Vec::<Skipped>::new());
    assert!(parsed.unknown.is_empty(), "{:?}", parsed.unknown);
    assert_eq!(parsed.user["crop"], [chord("R")]);
    for command in crate::app::shell::commands::registry::COMMANDS {
        assert!(
            guide.contains(&format!("| `{}` |", command.id)),
            "{} is not in the guide's list",
            command.id
        );
    }
}

#[test]
fn two_owners_of_one_file_keep_each_others_changes() {
    let setup = Setup::new(None);
    let mut first = setup.open();
    let mut second = setup.open();
    first.set("crop", &[chord("F9")]).expect("saved");
    second.set("clipping", &[chord("F10")]).expect("saved");
    let text = setup.text().expect("a file");
    assert!(text.contains("crop = [\"F9\"]"), "{text}");
    assert!(text.contains("clipping = [\"F10\"]"), "{text}");
    // The second owner runs what the file holds, the first's change too.
    assert_eq!(keys_of(&second, &bindings::CROP), ["F9"]);
    first.reset("clipping").expect("saved");
    assert!(!setup.text().unwrap().contains("clipping"));
    assert!(setup.text().unwrap().contains("crop = [\"F9\"]"));
}

#[test]
fn an_entry_added_after_start_up_survives_a_change() {
    let setup = Setup::new(Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n"));
    let mut file = setup.open();
    std::fs::write(
        setup.path(),
        "version = 1\n[bindings]\ncrop = [\"F9\"]\n\"someone/extension\" = [\"F10\"]\n",
    )
    .unwrap();
    file.set("clipping", &[chord("F11")]).expect("saved");
    let text = setup.text().unwrap();
    assert!(text.contains("\"someone/extension\" = [\"F10\"]"), "{text}");
    assert_eq!(file.unknown(), ["someone/extension"]);
}

#[test]
fn a_newer_file_written_after_start_up_is_left_alone_and_read_only() {
    let setup = Setup::new(Some("version = 1\n[bindings]\ncrop = [\"F9\"]\n"));
    let mut file = setup.open();
    let newer = "version = 2\n[bindings]\ncrop = [\"F9\"]\n\"someone/extension\" = [\"F10\"]\n";
    std::fs::write(setup.path(), newer).unwrap();
    let refused = file.set("clipping", &[chord("F11")]);
    assert!(
        refused.is_err_and(|e| e.contains("changed since it was read")),
        "refused"
    );
    assert_eq!(setup.text().as_deref(), Some(newer), "byte for byte");
    assert_eq!(*file.state(), State::Unreadable(Unreadable::Newer(2)));
    assert!(!file.editable());
    assert_eq!(keys_of(&file, &bindings::CROP), ["C", "R"]);
}

#[test]
fn a_fixed_commands_keys_cant_be_emptied() {
    let text = "version = 1\n[bindings]\nundo = []\nredo = []\nselect_all = []\n";
    let setup = Setup::new(Some(text));
    let mut file = setup.open();
    let skipped: Vec<&str> = file.skipped().iter().map(|s| s.id.as_str()).collect();
    assert_eq!(skipped, ["undo", "redo", "select_all"]);
    assert!(file.effective().is_default());
    assert_eq!(keys_of(&file, &bindings::LIBRARY_UNDO), ["Cmd+Z"]);
    assert_eq!(keys_of(&file, &bindings::GRID_SELECT_ALL), ["Cmd+A"]);
    for id in ["undo", "redo", "select_all"] {
        assert!(file.set(id, &[]).is_err(), "{id}");
    }
    assert_eq!(setup.text().as_deref(), Some(text));
}

#[test]
fn giving_a_command_other_keys_keeps_its_comments() {
    let text = "\
version = 1
[bindings]
# Crop where my hand is.
crop = [\"F9\"] # since 2026
clipping = [\"F10\"]
";
    let setup = Setup::new(Some(text));
    let mut file = setup.open();
    file.set("crop", &[chord("F11")]).expect("saved");
    assert_eq!(
        setup.text().unwrap(),
        text.replace("[\"F9\"]", "[\"F11\"]"),
        "the comments stay"
    );
}
