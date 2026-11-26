use super::*;

#[test]
fn a_load_that_never_returns_fails_after_its_limit_and_later_loads_fail_at_once() {
    static STALLED: AtomicBool = AtomicBool::new(false);
    let start = std::time::Instant::now();
    let hung = within(Duration::from_millis(100), &STALLED, "a test", || {
        std::thread::sleep(Duration::from_secs(3600));
        Ok(())
    });
    assert!(hung.is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
    let start = std::time::Instant::now();
    assert!(within(Duration::from_secs(60), &STALLED, "a test", || Ok(1)).is_err());
    assert!(start.elapsed() < Duration::from_millis(50));
}

#[test]
fn a_load_within_its_limit_answers_and_its_error_passes_through() {
    static STALLED: AtomicBool = AtomicBool::new(false);
    assert_eq!(
        within(Duration::from_secs(5), &STALLED, "a test", || Ok(7)),
        Ok(7)
    );
    assert_eq!(
        within::<()>(Duration::from_secs(5), &STALLED, "a test", || Err(
            InferenceError::Failed("bad".into())
        )),
        Err(InferenceError::Failed("bad".into()))
    );
    assert!(!STALLED.load(Ordering::Relaxed));
}

/// An array over `data` of `shape` with `strides`, in half precision.
fn half_array(data: &mut [u16], shape: &[usize], strides: &[usize]) -> Retained<MLMultiArray> {
    let numbers = |v: &[usize]| {
        let n: Vec<Retained<NSNumber>> = v.iter().map(|&n| NSNumber::new_usize(n)).collect();
        NSArray::from_retained_slice(&n)
    };
    unsafe {
        MLMultiArray::initWithDataPointer_shape_dataType_strides_deallocator_error(
            MLMultiArray::alloc(),
            NonNull::new(data.as_mut_ptr().cast()).unwrap(),
            &numbers(shape),
            MLMultiArrayDataType::Float16,
            &numbers(strides),
            None,
        )
    }
    .unwrap()
}

#[test]
fn an_answer_with_padded_rows_is_read_by_its_strides_never_its_padding() {
    // Two rows of three values, each row padded to four with NaN.
    let nan = half::f16::NAN.to_bits();
    let v = |x: f32| half::f16::from_f32(x).to_bits();
    let mut data = vec![v(1.), v(2.), v(3.), nan, v(4.), v(5.), v(6.), nan];
    let array = half_array(&mut data, &[1, 2, 3], &[8, 4, 1]);
    assert_eq!(
        read(&array, &[1, 2, 3], 2.).unwrap(),
        [2., 4., 6., 8., 10., 12.]
    );
    assert!(matches!(
        read(&array, &[1, 3, 2], 1.),
        Err(InferenceError::OutputInvalid(_))
    ));
    // Every other value of each row: read one by one.
    let mut data = vec![v(1.), nan, v(2.), nan, v(3.), nan, v(4.), nan];
    let array = half_array(&mut data, &[1, 2, 2], &[8, 4, 2]);
    assert_eq!(read(&array, &[1, 2, 2], 1.).unwrap(), [1., 2., 3., 4.]);
}

#[test]
fn only_contiguous_strides_are_taken_for_row_major() {
    assert!(row_major(&[6, 3, 1], &[1, 2, 3]));
    assert!(row_major(&[99, 3, 1], &[1, 2, 3]));
    assert!(!row_major(&[8, 4, 1], &[1, 2, 3]));
    assert!(!row_major(&[3, 1], &[1, 2, 3]));
}

/// A compiled model's folder with `files` in it.
fn compiled_folder(at: &Path, files: &[&str]) {
    std::fs::create_dir_all(at).unwrap();
    for name in files {
        std::fs::write(at.join(name), name).unwrap();
    }
}

#[test]
fn a_compiled_model_replaces_one_a_removal_left_unfinished() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join(".neural-engine-work");
    std::fs::create_dir(&work).unwrap();
    let staged = work.join("compiled.mlmodelc");
    compiled_folder(&staged, &["coremldata.bin", "model.mil"]);
    // A removal cut short: some files gone, coremldata.bin among them.
    let target = dir.path().join("neural-engine-tag.mlmodelc");
    compiled_folder(&target, &["model.mil"]);
    publish(&staged, &target, &work).unwrap();
    assert!(target.join("coremldata.bin").is_file());
    assert!(!staged.exists());
}

#[test]
fn a_compiled_model_another_load_put_in_place_first_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join(".neural-engine-work");
    std::fs::create_dir(&work).unwrap();
    let staged = work.join("compiled.mlmodelc");
    compiled_folder(&staged, &["coremldata.bin", "ours"]);
    let target = dir.path().join("neural-engine-tag.mlmodelc");
    compiled_folder(&target, &["coremldata.bin", "theirs"]);
    publish(&staged, &target, &work).unwrap();
    assert!(target.join("theirs").is_file());
}

#[test]
fn a_kept_model_that_wont_load_leaves_no_folder_behind_under_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("neural-engine-tag.mlmodelc");
    compiled_folder(&target, &["coremldata.bin", "model.mil"]);
    discard("neural-engine", dir.path(), &target);
    assert!(!target.exists());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn discarding_a_model_another_load_discarded_first_leaves_no_folder() {
    let dir = tempfile::tempdir().unwrap();
    discard(
        "neural-engine",
        dir.path(),
        &dir.path().join("neural-engine-tag.mlmodelc"),
    );
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}
