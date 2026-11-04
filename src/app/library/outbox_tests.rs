use super::*;
use crate::app::library::cell::PhotoAction;
use crate::app::testing::editor_with_catalog;
use crate::app::{Editor, Modal};

/// Chooses `action` in photo `id`'s thumbnail menu, as the grid does.
fn choose(e: &mut Editor, id: PhotoId, action: PhotoAction) {
    let ctx = e.context.clone();
    let library = e.library.as_mut().unwrap();
    let photo = library.photo(id).unwrap().clone();
    library.photo_action(&ctx, &photo, action, true);
}

/// What the editor does with the Library's requests at the end of a frame.
fn carry_out(e: &mut Editor) {
    let ctx = e.context.clone();
    e.library_requests(&ctx);
}

#[test]
fn a_request_equal_to_the_last_one_queued_is_queued_once() {
    let mut outbox = Outbox::default();
    let find = |id| Request::Find(PhotoId(id));
    outbox.push(find(1), 0);
    outbox.push(find(1), 0);
    assert_eq!(outbox.len(), 1);
    // Different requests, and the first again after another, are all kept.
    outbox.push(find(2), 0);
    outbox.push(find(1), 0);
    assert_eq!(outbox.len(), 3);
    let order: Vec<_> = std::iter::from_fn(|| outbox.pop())
        .map(|(r, _)| r)
        .collect();
    assert_eq!(order, [find(1), find(2), find(1)]);
}

/// A dialog asked for after another in the same frame does not replace it. The
/// slots handed the Library's requests over by kind, Remove Copy before Read
/// Metadata, so Read's dialog replaced the one asked for first.
#[test]
fn a_second_dialog_asked_for_in_a_frame_leaves_the_first_open() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    choose(
        &mut e,
        ids[1],
        PhotoAction::Copy(CopyAction::Remove(ids[1])),
    );
    choose(&mut e, ids[0], PhotoAction::ReadMetadata);
    carry_out(&mut e);
    assert!(matches!(e.modal, Some(Modal::RemoveCopy(id)) if id == ids[1]));
    assert!(
        e.messages
            .line()
            .contains("Finish what the open window asks"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

/// A file operation asked for after a dialog opened in the same frame is refused
/// too: its window would open over the dialog.
#[test]
fn a_file_operation_asked_for_after_a_dialog_is_refused() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    choose(&mut e, ids[0], PhotoAction::ReadMetadata);
    choose(&mut e, ids[0], PhotoAction::Rename);
    carry_out(&mut e);
    assert!(matches!(e.modal, Some(Modal::ReadMetadata(_))));
    assert!(!e.file_ops.dialog_open());
    assert!(
        e.messages
            .line()
            .contains("Finish what the open window asks")
    );
    Ok(())
}

/// A window open that is not one of the editor's dialogs (here Keyboard Shortcuts)
/// keeps a dialog from opening over it too.
#[test]
fn a_dialog_is_not_opened_over_another_window() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.view.shortcuts = true;
    choose(&mut e, ids[0], PhotoAction::ReadMetadata);
    carry_out(&mut e);
    assert!(e.modal.is_none());
    assert!(
        e.messages
            .line()
            .contains("Finish what the open window asks")
    );
    Ok(())
}

/// Requests are handed over in the order they were made, each when its turn comes:
/// one made before the lists changed names only the photos still listed, and one
/// whose photos are all gone is not handed over.
#[test]
fn a_request_made_before_the_lists_changed_names_only_photos_still_listed() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.ask(Request::Read(vec![ids[0], copy]));
    library.ask(Request::Find(copy));
    library.ask(Request::Find(ids[1]));
    library.remove_virtual_copy(copy)?;
    assert_eq!(library.next_request(), Some(Request::Read(vec![ids[0]])));
    // The Find of the copy is gone, and says so; the next one comes instead.
    assert_eq!(library.next_request(), Some(Request::Find(ids[1])));
    assert!(library.messages.line().contains("no longer in the catalog"));
    assert_eq!(library.next_request(), None);
    Ok(())
}

/// Making a virtual copy changes the lists but leaves every photo there: a request
/// made before it keeps all its photos.
#[test]
fn a_request_made_before_a_copy_was_made_keeps_its_photos() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.ask(Request::Read(ids.clone()));
    let before = library.session.generation();
    library.create_virtual_copy(ids[0])?;
    assert_ne!(library.session.generation(), before);
    assert_eq!(library.next_request(), Some(Request::Read(ids)));
    Ok(())
}

/// New Folder Inside offers to move the selection along; with the selection gone it
/// still makes the folder, offering nothing.
#[test]
fn a_new_folder_whose_photos_are_gone_is_still_asked_for() {
    let request = Request::File(FileRequest::NewFolder {
        root: crate::catalog::RootId(1),
        parent: String::new(),
        name: "New".into(),
        photos: vec![PhotoId(7)],
        inside: true,
    });
    assert_eq!(
        request.revalidated(|_| false),
        Some(Request::File(FileRequest::NewFolder {
            root: crate::catalog::RootId(1),
            parent: String::new(),
            name: "New".into(),
            photos: Vec::new(),
            inside: false,
        }))
    );
}

/// A virtual copy asked for while other work holds the catalog is not made, and the
/// message says why; the slots dropped it without a word.
#[test]
fn a_virtual_copy_refused_while_work_is_in_progress_says_why() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    assert!(e.activity.begin_dialog());
    choose(
        &mut e,
        ids[0],
        PhotoAction::Copy(CopyAction::Create(ids[0])),
    );
    carry_out(&mut e);
    assert_eq!(e.library.as_ref().unwrap().session.photos().len(), 1);
    assert!(
        e.messages.line().contains("Virtual copy not made"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

/// Find on Disk while a file dialog is already open is not started, and says so.
#[test]
fn find_on_disk_while_a_file_dialog_is_open_says_why() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    assert!(e.activity.begin_dialog());
    choose(&mut e, ids[0], PhotoAction::FindOnDisk);
    carry_out(&mut e);
    assert!(
        e.messages.line().contains("Find on Disk waits"),
        "{}",
        e.messages.line()
    );
    Ok(())
}
