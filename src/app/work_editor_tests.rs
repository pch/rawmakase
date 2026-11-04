//! Editor tests of `work`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

#[test]
fn edited_previews_take_the_computer_only_with_the_library_in_front_and_nothing_else_rendering()
-> anyhow::Result<()> {
    use crate::app::library::Pace;
    let (_directory, mut editor, _) = editor_with_catalog(&["a.RAF"])?;
    editor.module = Module::Library;
    assert_eq!(editor.preview_pace(), Pace::Full);
    editor.module = Module::Develop;
    assert_eq!(editor.preview_pace(), Pace::Background);
    // An output a command started, still running in the Library.
    editor.module = Module::Library;
    editor.automation.hold_running_output();
    assert_eq!(editor.preview_pace(), Pace::Background);
    editor.automation.cancel_outputs();
    assert_eq!(editor.preview_pace(), Pace::Full);
    Ok(())
}
