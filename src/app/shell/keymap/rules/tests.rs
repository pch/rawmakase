use super::*;
use crate::app::shell::commands::registry::command_by_id;

fn chord(text: &str) -> Chord {
    Chord::parse(text).expect("a chord")
}
fn refused(id: &str, keys: &str, os: Os) -> Option<Refusal> {
    let command = command_by_id(id).expect("a command");
    refusal(command, &bindings_of(id), chord(keys), os)
}

#[test]
fn the_text_editing_keys_are_everyones_and_the_systems_are_the_macs() {
    for os in [Os::Macos, Os::Windows, Os::Other] {
        for keys in ["Cmd+C", "Cmd+V", "Cmd+X", "Cmd+A", "Cmd+Z", "Cmd+Shift+Z"] {
            assert_eq!(refused("crop", keys, os), Some(Refusal::Reserved), "{keys}");
        }
        assert_eq!(refused("crop", "Cmd+Alt+C", os), None);
    }
    // Ctrl+Y redoes in a text field off the Mac.
    assert_eq!(
        refused("crop", "Cmd+Y", Os::Windows),
        Some(Refusal::Reserved)
    );
    assert_eq!(refused("crop", "Cmd+Y", Os::Other), Some(Refusal::Reserved));
    for keys in ["Cmd+Q", "Cmd+W", "Cmd+H", "Cmd+M", "Cmd+O", "Cmd+Shift+O"] {
        assert_eq!(refused("crop", keys, Os::Macos), Some(Refusal::Reserved));
        assert_eq!(refused("crop", keys, Os::Other), None);
    }
}

#[test]
fn windows_refuses_what_egui_winit_turns_into_cut_copy_or_paste() {
    for keys in [
        "Cmd+Insert",
        "Shift+Insert",
        "Cmd+Alt+Insert",
        "Shift+Delete",
        "Cmd+Shift+Delete",
        "Cmd+Alt+Shift+Delete",
        "Cmd+Shift+X",
        "Cmd+Alt+Shift+X",
    ] {
        assert_eq!(
            refused("crop", keys, Os::Windows),
            Some(Refusal::WindowsClipboard),
            "{keys}"
        );
        assert_eq!(refused("crop", keys, Os::Other), None, "{keys}");
    }
    // Ctrl+Alt+X arrives as Cut, which the hook turns back into X.
    assert_eq!(refused("crop", "Cmd+Alt+X", Os::Windows), None);
    assert_eq!(refused("crop", "Insert", Os::Windows), None);
    assert_eq!(refused("crop", "Cmd+Delete", Os::Windows), None);
}

#[test]
fn a_chord_may_not_name_its_commands_variant() {
    // Shift goes on to the next photo after a rating; Shift and Alt pick a
    // Basic slider's step.
    assert_eq!(
        refused("rating:3", "Shift+F9", Os::Other),
        Some(Refusal::Variant(Mods::SHIFT))
    );
    assert_eq!(
        refused("basic:increase", "Alt+F9", Os::Other),
        Some(Refusal::Variant(Mods::ALT))
    );
    // The previous photo: Shift extends the grid's selection.
    assert_eq!(
        refused("previous", "Shift+F9", Os::Other),
        Some(Refusal::Variant(Mods::SHIFT))
    );
    assert_eq!(refused("rating:3", "Cmd+F9", Os::Other), None);
}

#[test]
fn a_command_that_works_while_typing_needs_a_key_that_doesnt_type() {
    assert_eq!(
        refused("preferences", "P", Os::Other),
        Some(Refusal::Typing)
    );
    assert_eq!(
        refused("filters", "Shift+L", Os::Other),
        Some(Refusal::Typing)
    );
    assert_eq!(refused("preferences", "Cmd+P", Os::Other), None);
    assert_eq!(refused("preferences", "F9", Os::Other), None);
    assert_eq!(refused("preferences", "Ctrl+P", Os::Macos), None);
    // Plain keys are fine where typing stops the command.
    assert_eq!(refused("crop", "P", Os::Other), None);
}

#[test]
fn a_fixed_key_keeps_its_chord_where_it_works() {
    assert_eq!(
        refused("crop", "Up", Os::Other),
        Some(Refusal::Taken("Nudging the hovered slider"))
    );
    assert_eq!(
        refused("clipping", "Shift+Down", Os::Other),
        Some(Refusal::Taken("Nudging the hovered slider"))
    );
    // Everywhere includes Develop.
    assert_eq!(
        refused("chrome", "Space", Os::Other),
        Some(Refusal::Taken("Panning while a tool is open"))
    );
    // The grid has no hovered slider.
    assert_eq!(refused("grid:cell_style", "Up", Os::Other), None);
    assert_eq!(refused("crop", "Cmd+Up", Os::Other), None);
}

#[test]
fn fixed_commands_keep_their_keys() {
    for id in ["undo", "redo", "select_all"] {
        assert_eq!(refused(id, "F9", Os::Other), Some(Refusal::Fixed), "{id}");
    }
}

#[test]
fn overlapping_contexts_go_both_ways() {
    use Context::*;
    let all = [
        Everywhere,
        Library,
        Grid,
        Loupe,
        Compare,
        Survey,
        LoupeZoom,
        Develop,
        Context::Tool(super::super::Tool::Crop),
        Context::Tool(super::super::Tool::Mask),
        ShortcutSheet,
    ];
    for a in all {
        assert!(a.overlaps(a));
        for b in all {
            assert_eq!(a.overlaps(b), b.overlaps(a), "{a:?} {b:?}");
        }
    }
    assert!(Develop.overlaps(Context::Tool(super::super::Tool::Crop)));
    assert!(!Grid.overlaps(Develop));
    assert!(!Grid.overlaps(Loupe));
    assert!(LoupeZoom.overlaps(Loupe));
    assert!(!LoupeZoom.overlaps(Grid));
    assert!(
        !Context::Tool(super::super::Tool::Crop).overlaps(Context::Tool(super::super::Tool::Mask))
    );
    assert!(!ShortcutSheet.overlaps(Everywhere));
}

#[test]
fn every_refusal_says_why() {
    assert_eq!(
        Refusal::Variant(Mods::SHIFT).to_string(),
        "Shift is held for this command's other form"
    );
    assert_eq!(
        Refusal::Variant(Mods::ALT.with(Mods::SHIFT)).to_string(),
        "Alt+Shift is held for this command's other form"
    );
}

#[test]
fn a_fixed_key_is_refused_with_every_modifier_its_reader_lets_through() {
    // The hovered slider takes Up and Down with Shift or Alt held too; Space
    // pans whatever is held.
    for keys in [
        "Up",
        "Alt+Up",
        "Shift+Down",
        "Alt+Shift+Down",
        "Space",
        "Cmd+Space",
        "Shift+Space",
    ] {
        assert!(
            matches!(
                refused("clipping", keys, Os::Other),
                Some(Refusal::Taken(_))
            ),
            "{keys}"
        );
    }
    // Cmd+Up is no nudge: the slider's reader refuses Cmd.
    assert_eq!(refused("clipping", "Cmd+Alt+Up", Os::Other), None);
}
