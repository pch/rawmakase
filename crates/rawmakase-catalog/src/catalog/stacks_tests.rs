//! Folder stacks (see `stacks`): what each command leaves, and how rows a
//! move, a removal or another release changed are read.
use super::*;
use rusqlite::params;
use std::path::PathBuf;

/// Group into Stack: `photos` one stack, the first its top; the stack.
pub(super) fn group(cat: &mut Catalog, photos: &[PhotoId]) -> Result<StackId> {
    cat.change_stacks(&StackChange::Group(photos.to_vec()))?;
    Ok(cat.stack_places()?[&photos[0]].id)
}

/// A catalog of `names` in folder "photos", their ids by name, and the
/// folder.
fn catalog(names: &[&str]) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), format!("synthetic {name}"))?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let photos = cat.photos()?;
    let ids = names
        .iter()
        .map(|n| photos.iter().find(|p| p.filename == *n).unwrap().id)
        .collect();
    Ok((dir, cat, ids, folder))
}

/// Removes photo `id` from the catalog, as Remove from Catalog does.
fn remove(cat: &mut Catalog, id: PhotoId) -> Result<()> {
    cat.db.write(|w| super::copies::delete_photo(w, id))
}

/// Each stacked photo's (stack, index, count, collapsed), by id.
fn places(cat: &Catalog) -> Result<Vec<(PhotoId, usize, usize, bool)>> {
    let mut places: Vec<_> = cat
        .stack_places()?
        .into_iter()
        .map(|(id, p)| (id, p.index, p.count, p.collapsed))
        .collect();
    places.sort();
    Ok(places)
}

#[test]
fn grouping_makes_a_collapsed_stack_topped_by_the_first_photo() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    let id = group(&mut cat, &[c, a, b])?;
    assert_eq!(cat.stack_of(a)?, [c, a, b]);
    assert!(cat.stack_of(d)?.is_empty());
    let photo = |id: PhotoId| cat.photos().unwrap().into_iter().find(|p| p.id == id);
    let top = photo(c).unwrap().stack.unwrap();
    assert_eq!(
        (top.id, top.index, top.count, top.collapsed),
        (id, 0, 3, true)
    );
    assert!(top.is_top());
    assert_eq!(photo(d).unwrap().stack, None);

    // Fewer than two photos, a photo twice or an unknown one: no stack.
    assert!(group(&mut cat, &[d]).is_err());
    assert!(group(&mut cat, &[d, d]).is_err());
    assert!(group(&mut cat, &[d, PhotoId(9999)]).is_err());
    assert!(cat.stack_of(d)?.is_empty());
    Ok(())
}

#[test]
fn photos_of_different_folders_are_not_stacked() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for sub in ["x", "y"] {
        std::fs::create_dir_all(dir.path().join("photos").join(sub))?;
        std::fs::write(dir.path().join("photos").join(sub).join("a.ARW"), sub)?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&dir.path().join("photos"))?;
    let ids: Vec<_> = cat.photos()?.iter().map(|p| p.id).collect();
    let error = group(&mut cat, &ids).unwrap_err();
    assert!(error.to_string().contains("same folder"), "{error}");
    assert!(cat.stack_places()?.is_empty());
    Ok(())
}

#[test]
fn regrouping_takes_photos_out_of_their_stacks_and_a_single_photo_is_no_stack() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c])?;
    group(&mut cat, &[d, c])?;
    assert_eq!(cat.stack_of(a)?, [a, b]);
    assert_eq!(cat.stack_of(c)?, [d, c]);
    group(&mut cat, &[b, d])?;
    // a is alone in its stack: no stack any more; so is c.
    assert!(cat.stack_of(a)?.is_empty());
    assert!(cat.stack_of(c)?.is_empty());
    assert_eq!(cat.stack_of(b)?, [b, d]);
    let stacks: i64 = cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM stacks", [], |r| r.get(0))?;
    // Two stacks still hold one photo's row each, until it leaves them.
    assert!(stacks <= 3, "{stacks}");
    Ok(())
}

#[test]
fn unstack_remove_and_move_to_top_keep_the_rest_in_order() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c, d])?;
    cat.change_stacks(&StackChange::MoveToTop(c))?;
    assert_eq!(cat.stack_of(a)?, [c, a, b, d]);
    cat.change_stacks(&StackChange::Move(a, 1))?;
    assert_eq!(cat.stack_of(a)?, [c, b, a, d]);
    cat.change_stacks(&StackChange::Move(d, 1))?;
    assert_eq!(cat.stack_of(a)?, [c, b, a, d]);
    // Removing the top makes the next photo the top.
    cat.change_stacks(&StackChange::Remove(vec![c]))?;
    assert_eq!(cat.stack_of(a)?, [b, a, d]);
    assert!(cat.stack_of(c)?.is_empty());
    cat.change_stacks(&StackChange::Remove(vec![a, d]))?;
    assert!(cat.stack_places()?.is_empty());
    assert!(cat.change_stacks(&StackChange::MoveToTop(a)).is_err());

    group(&mut cat, &[a, b])?;
    group(&mut cat, &[c, d])?;
    cat.change_stacks(&StackChange::Unstack(vec![b]))?;
    assert!(cat.stack_of(a)?.is_empty());
    assert_eq!(cat.stack_of(d)?, [c, d]);
    Ok(())
}

#[test]
fn collapsing_is_saved_with_the_catalog() -> Result<()> {
    let (dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW"])?;
    let id = group(&mut cat, &ids)?;
    cat.set_stacks_collapsed(&[id], false)?;
    drop(cat);
    let cat = Catalog::open(dir.path().join("Photos.rawmakase"))?;
    assert!(places(&cat)?.iter().all(|p| !p.3));
    Ok(())
}

#[test]
fn a_virtual_copy_is_stacked_right_after_its_photo_and_shown() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW"])?;
    let [a, b, c] = ids[..] else { panic!() };
    let copy = cat.create_virtual_copy(a)?;
    assert_eq!(cat.stack_of(a)?, [a, copy]);
    assert!(
        places(&cat)?.iter().all(|p| !p.3),
        "open, so the copy shows"
    );

    let id = group(&mut cat, &[b, c])?;
    cat.set_stacks_collapsed(&[id], true)?;
    let second = cat.create_virtual_copy(b)?;
    assert_eq!(cat.stack_of(c)?, [b, second, c]);
    // A copy of a copy follows the copy it was made from.
    let third = cat.create_virtual_copy(copy)?;
    assert_eq!(cat.stack_of(a)?, [a, copy, third]);
    // Set Copy as Master changes no stack.
    cat.set_copy_as_master(copy)?;
    assert_eq!(cat.stack_of(a)?, [a, copy, third]);
    // Removing a copy takes it out of its stack.
    cat.remove_virtual_copy(third)?;
    assert_eq!(cat.stack_of(a)?, [a, copy]);
    Ok(())
}

#[test]
fn undo_puts_back_the_stacks_as_they_were() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW", "e.ARW"])?;
    let [a, b, c, d, e] = ids[..] else { panic!() };
    let first = group(&mut cat, &[a, b, c])?;
    cat.set_stacks_collapsed(&[first], false)?;
    group(&mut cat, &[d, e])?;
    let selection = [b, d];
    let before = cat.stack_state(&selection)?;
    assert_eq!(before.photos.len(), 5);
    // The change reads both states in its own transaction, so Undo and
    // Redo get them whenever it is saved.
    let changed = cat.change_stacks(&StackChange::Group(vec![d, b]))?;
    let after = cat.stack_state(&before.photos)?;
    assert_eq!(changed.before.photos.len(), before.photos.len());
    assert!(
        before
            .stacks
            .iter()
            .all(|s| changed.before.stacks.contains(s))
    );
    assert_eq!(changed.after.stacks, after.stacks);
    assert_eq!(cat.stack_of(a)?, [a, c]);

    cat.change_stacks(&StackChange::Restore(before))?;
    assert_eq!(cat.stack_of(a)?, [a, b, c]);
    assert_eq!(cat.stack_of(e)?, [d, e]);
    assert!(!cat.stack_places()?[&a].collapsed);
    cat.change_stacks(&StackChange::Restore(after))?;
    assert_eq!(cat.stack_of(b)?, [d, b]);
    assert_eq!(cat.stack_of(c)?, [a, c]);
    assert!(cat.stack_of(e)?.is_empty());

    // A photo removed since is left out; a stack it leaves single is none.
    let mut state = cat.stack_state(&[d])?;
    cat.change_stacks(&StackChange::Unstack(vec![d]))?;
    remove(&mut cat, b)?;
    state.forget_photo(b);
    cat.change_stacks(&StackChange::Restore(state.clone()))?;
    assert!(cat.stack_of(d)?.is_empty());
    assert_eq!(cat.stack_of(c)?, [a, c]);
    Ok(())
}

#[test]
fn rows_another_release_changed_are_read_as_stacks_of_one_folder() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for sub in ["x", "y"] {
        std::fs::create_dir_all(dir.path().join("photos").join(sub))?;
    }
    for name in ["x/a.ARW", "x/b.ARW", "x/c.ARW", "y/d.ARW"] {
        std::fs::write(dir.path().join("photos").join(name), name)?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&dir.path().join("photos"))?;
    let id = |name: &str| {
        cat.photos()
            .unwrap()
            .into_iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    };
    let (a, b, c, d) = (id("a.ARW"), id("b.ARW"), id("c.ARW"), id("d.ARW"));
    group(&mut cat, &[a, b, c])?;
    let db = cat.db_for_tests();
    // A release without stacks moves the top to the other folder, and
    // removes c, as SQLite does with its foreign keys on.
    let y: i64 = db.query_row("SELECT folder FROM photos WHERE id=?", [d.0], |r| r.get(0))?;
    db.execute("UPDATE photos SET folder=? WHERE id=?", params![y, a.0])?;
    db.execute("PRAGMA foreign_keys=ON", [])?;
    db.execute("DELETE FROM photos WHERE id=?", [c.0])?;
    assert!(cat.stack_of(a)?.is_empty());
    assert!(cat.stack_of(b)?.is_empty(), "b is alone in its folder now");
    assert!(cat.stack_places()?.is_empty());
    Ok(())
}

#[test]
fn removing_a_photo_from_the_catalog_takes_it_out_of_its_stack() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW"])?;
    let [a, b, c] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c])?;
    remove(&mut cat, a)?;
    assert_eq!(cat.stack_of(b)?, [b, c]);
    assert!(cat.stack_places()?[&b].is_top());
    Ok(())
}

#[test]
fn auto_stack_joins_runs_of_photos_less_than_the_interval_apart() {
    let (f, g) = (FolderId(1), FolderId(2));
    let photos = [
        (PhotoId(1), f, "2024-03-02T10:00:00.000"),
        (PhotoId(2), f, "2024-03-02T10:00:09.000"),
        // 9 s after the one before, 18 s after the first: still the run.
        (PhotoId(3), f, "2024-03-02T10:00:18.000"),
        // Exactly 10 s: a new run.
        (PhotoId(4), f, "2024-03-02T10:00:28.000"),
        (PhotoId(5), f, "2024-03-02T10:00:28.500"),
        // Another folder, at the same time: never with the ones above.
        (PhotoId(6), g, "2024-03-02T10:00:28.200"),
        // No time: left alone.
        (PhotoId(7), f, ""),
        (PhotoId(8), f, "2024-03-02T10:30:00"),
        // Across midnight and a month's end.
        (PhotoId(9), g, "2024-03-31T23:59:59.900"),
        (PhotoId(10), g, "2024-04-01T00:00:00.100"),
    ];
    assert_eq!(
        auto_stacks(&photos, 10.),
        [
            vec![PhotoId(1), PhotoId(2), PhotoId(3)],
            vec![PhotoId(4), PhotoId(5)],
            vec![PhotoId(9), PhotoId(10)],
        ]
    );
    // No interval: nothing is stacked, even photos taken at the same time.
    assert!(auto_stacks(&photos, 0.).is_empty());
    let same = [
        (PhotoId(1), f, "2024-03-02T10:00:00"),
        (PhotoId(2), f, "2024-03-02T10:00:00"),
    ];
    assert_eq!(auto_stacks(&same, 1.), [vec![PhotoId(1), PhotoId(2)]]);
    assert_eq!(
        super::stacks::capture_seconds_for_tests("2024-13-02T10:00:00"),
        None
    );
}

mod bursts {
    use super::super::{BurstCandidate, FolderId, PhotoId, auto_stacks_with_bursts};
    use rawmakase_interop::exif::{Mark, Sequence};

    fn sequence(camera: &str, mark: Mark) -> Sequence {
        Sequence {
            camera: camera.into(),
            mark,
        }
    }
    fn frame(camera: &str, n: u32) -> Sequence {
        sequence(camera, Mark::Frame(n))
    }

    /// Photos of folder 1 in this order, the `n`th taken `at` seconds after
    /// 10:00, as RAWs named in that order.
    fn plan(photos: &[(f64, Option<Sequence>)], seconds: f64) -> Vec<Vec<u64>> {
        let times: Vec<String> = photos
            .iter()
            .map(|(at, _)| {
                let at = *at;
                let whole = at.floor() as u64;
                format!(
                    "2024-03-02T{:02}:{:02}:{:02}.{:03}",
                    10 + whole / 3600,
                    whole / 60 % 60,
                    whole % 60,
                    ((at - at.floor()) * 1000.).round() as u64
                )
            })
            .collect();
        let names: Vec<String> = (0..photos.len())
            .map(|i| format!("DSC{i:05}.ARW"))
            .collect();
        let candidates: Vec<BurstCandidate<'_>> = photos
            .iter()
            .enumerate()
            .map(|(i, (_, sequence))| BurstCandidate {
                id: PhotoId(i as i64 + 1),
                folder: FolderId(1),
                captured: &times[i],
                filename: &names[i],
                format: "ARW",
                sequence: sequence.as_ref(),
            })
            .collect();
        auto_stacks_with_bursts(&candidates, seconds)
            .into_iter()
            .map(|s| s.into_iter().map(|id| id.0 as u64).collect())
            .collect()
    }

    #[test]
    fn two_bursts_inside_the_interval_stay_apart() {
        let photos = [
            (0.0, Some(frame("A", 1))),
            (0.1, Some(frame("A", 2))),
            (0.2, Some(frame("A", 3))),
            (0.5, Some(frame("A", 1))),
            (0.6, Some(frame("A", 2))),
        ];
        assert_eq!(plan(&photos, 60.), [vec![1, 2, 3], vec![4, 5]]);
        // A press of one frame is no stack.
        let single = [(0.0, Some(frame("A", 1))), (0.5, Some(frame("A", 1)))];
        assert!(plan(&single, 60.).is_empty());
    }

    #[test]
    fn a_slow_burst_with_culled_frames_stays_one_stack() {
        let photos = [
            (0.0, Some(frame("A", 1))),
            (35.0, Some(frame("A", 2))),
            // 3 and 4 deleted.
            (90.0, Some(frame("A", 5))),
        ];
        assert_eq!(plan(&photos, 1.), [vec![1, 2, 3]]);
    }

    #[test]
    fn frames_more_than_a_minute_apart_are_two_stacks_whatever_their_numbers() {
        let photos = [
            (0.0, Some(frame("A", 1))),
            (0.1, Some(frame("A", 2))),
            // The next burst's first frames deleted, 100 minutes later.
            (6000.0, Some(frame("A", 102))),
            (6000.1, Some(frame("A", 103))),
        ];
        assert_eq!(plan(&photos, 3600.), [vec![1, 2], vec![3, 4]]);
        // Within the minute, frame numbers can't tell them apart: the
        // heuristic's known limit.
        let close = [
            (0.0, Some(frame("A", 1))),
            (0.1, Some(frame("A", 2))),
            (10.0, Some(frame("A", 3))),
        ];
        assert_eq!(plan(&close, 0.), [vec![1, 2, 3]]);
    }

    #[test]
    fn a_burst_whose_first_frames_were_deleted_starts_its_own_stack_later() {
        let photos = [
            (0.0, Some(frame("A", 1))),
            (0.1, Some(frame("A", 2))),
            // The next burst's frames 1 and 2 deleted; five minutes later.
            (300.0, Some(frame("A", 3))),
            (300.1, Some(frame("A", 4))),
        ];
        assert_eq!(plan(&photos, 600.), [vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn photos_of_one_second_follow_their_file_names_not_their_ids() {
        // Two presses in one second, without subseconds: 1 2 3, then 1 2,
        // listed in the catalog in another order.
        let a = [frame("A", 1), frame("A", 2), frame("A", 3)];
        let b = [frame("A", 1), frame("A", 2)];
        let names = ["DSC1.ARW", "DSC2.ARW", "DSC3.ARW", "DSC4.ARW", "DSC5.ARW"];
        let photo = |id, name: usize, s| BurstCandidate {
            id: PhotoId(id),
            folder: FolderId(1),
            captured: "2024-03-02T10:00:00",
            filename: names[name],
            format: "ARW",
            sequence: Some(s),
        };
        let photos = [
            photo(5, 0, &a[0]),
            photo(3, 3, &b[0]),
            photo(1, 1, &a[1]),
            photo(4, 4, &b[1]),
            photo(2, 2, &a[2]),
        ];
        let stacks = auto_stacks_with_bursts(&photos, 0.);
        assert_eq!(
            stacks,
            [
                vec![PhotoId(5), PhotoId(1), PhotoId(2)],
                vec![PhotoId(3), PhotoId(4)]
            ]
        );
    }

    #[test]
    fn two_bodies_and_burst_ids_never_mix() {
        let photos = [
            (0.0, Some(frame("A", 1))),
            (0.1, Some(frame("B", 1))),
            (0.2, Some(frame("A", 2))),
            (0.3, Some(frame("B", 2))),
            (0.4, Some(sequence("iPhone", Mark::Burst("x".into())))),
            (0.5, Some(sequence("iPhone", Mark::Burst("y".into())))),
            (90.0, Some(sequence("iPhone", Mark::Burst("x".into())))),
        ];
        assert_eq!(plan(&photos, 0.), [vec![1, 3], vec![2, 4], vec![5, 7]]);
    }

    #[test]
    fn single_shots_are_left_alone_and_unknown_photos_go_by_time_between_them() {
        let photos = [
            (0.0, None),
            (1.0, None),
            (2.0, Some(sequence("A", Mark::Single))),
            (3.0, None),
            (4.0, Some(sequence("A", Mark::Single))),
            (5.0, Some(frame("A", 1))),
            (6.0, None),
            (7.0, None),
        ];
        assert_eq!(plan(&photos, 10.), [vec![1, 2], vec![7, 8]]);
        // Unknown photos alone are stacked as Auto-Stack by Capture Time does.
        let unknown = [(0.0, None), (5.0, None), (20.0, None), (21.0, None)];
        assert_eq!(plan(&unknown, 10.), [vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn frames_not_numbered_go_by_time_each_camera_apart() {
        let c = |camera| Some(sequence(camera, Mark::Continuous));
        let photos = [
            (0.0, c("A")),
            (0.3, c("A")),
            (0.6, c("B")),
            (0.9, c("A")),
            (5.0, c("A")),
            (5.2, c("A")),
            (6.0, None),
            (6.5, None),
        ];
        assert_eq!(plan(&photos, 1.), [vec![1, 2, 4], vec![5, 6], vec![7, 8]]);
        // Less than the interval apart, as Auto-Stack by Capture Time has it.
        let same_time = [(0.0, c("A")), (0.0, c("A"))];
        assert!(plan(&same_time, 0.).is_empty());
        assert_eq!(plan(&same_time, 0.001), [vec![1, 2]]);
    }

    #[test]
    fn a_cameras_photo_of_another_kind_ends_its_runs() {
        let c = |camera| Some(sequence(camera, Mark::Continuous));
        let photos = [
            (0.0, c("A")),
            (0.2, Some(sequence("A", Mark::Single))),
            (0.4, c("A")),
            (1.0, Some(frame("A", 1))),
            (1.1, c("A")),
            (1.2, Some(frame("A", 2))),
        ];
        assert!(plan(&photos, 10.).is_empty());
        // Another body's photos don't.
        let photos = [
            (0.0, c("A")),
            (0.1, Some(sequence("B", Mark::Single))),
            (0.2, Some(frame("B", 1))),
            (0.3, c("A")),
            (0.4, Some(frame("A", 1))),
            (0.5, c("B")),
            (0.6, Some(frame("A", 2))),
        ];
        assert_eq!(plan(&photos, 10.), [vec![1, 4], vec![5, 7]]);
    }

    #[test]
    fn a_raw_and_its_jpeg_make_two_runs_and_folders_stay_apart() {
        let (a1, a2) = (frame("A", 1), frame("A", 2));
        let times = ["2024-03-02T10:00:00.000", "2024-03-02T10:00:00.100"];
        let photo = |id, folder, at: usize, format, s| BurstCandidate {
            id: PhotoId(id),
            folder: FolderId(folder),
            captured: times[at],
            filename: "DSC1.ARW",
            format,
            sequence: Some(s),
        };
        let photos = [
            photo(1, 1, 0, "ARW", &a1),
            photo(2, 1, 0, "JPG", &a1),
            photo(3, 1, 1, "ARW", &a2),
            photo(4, 1, 1, "JPG", &a2),
            photo(5, 2, 1, "ARW", &a2),
        ];
        let stacks = auto_stacks_with_bursts(&photos, 60.);
        assert_eq!(
            stacks,
            [vec![PhotoId(1), PhotoId(3)], vec![PhotoId(2), PhotoId(4)]]
        );
        // No time: left alone.
        let no_time = [BurstCandidate {
            captured: "",
            ..photos[4]
        }];
        assert!(auto_stacks_with_bursts(&[photos[3], no_time[0]], 60.).is_empty());
    }
}

#[test]
fn several_stacks_are_made_all_or_none() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    assert!(
        cat.change_stacks(&StackChange::Groups(vec![vec![a, b], vec![c]]))
            .is_err()
    );
    assert!(cat.stack_places()?.is_empty());
    cat.change_stacks(&StackChange::Groups(vec![vec![a, b], vec![c, d]]))?;
    assert_eq!(cat.stack_of(b)?, [a, b]);
    assert_eq!(cat.stack_of(d)?, [c, d]);
    Ok(())
}
