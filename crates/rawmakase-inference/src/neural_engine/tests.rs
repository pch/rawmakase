use super::*;

#[test]
fn a_load_that_never_returns_fails_after_its_limit_and_later_loads_fail_at_once() {
    static STALLED: AtomicBool = AtomicBool::new(false);
    let start = std::time::Instant::now();
    let hung = within(Duration::from_millis(100), &STALLED, || {
        std::thread::sleep(Duration::from_secs(3600));
        Ok(())
    });
    assert!(hung.is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
    let start = std::time::Instant::now();
    assert!(within(Duration::from_secs(60), &STALLED, || Ok(1)).is_err());
    assert!(start.elapsed() < Duration::from_millis(50));
}

#[test]
fn a_load_within_its_limit_answers_and_its_error_passes_through() {
    static STALLED: AtomicBool = AtomicBool::new(false);
    assert_eq!(within(Duration::from_secs(5), &STALLED, || Ok(7)), Ok(7));
    assert_eq!(
        within::<()>(Duration::from_secs(5), &STALLED, || Err(
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
    discard(dir.path(), &target);
    assert!(!target.exists());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn discarding_a_model_another_load_discarded_first_leaves_no_folder() {
    let dir = tempfile::tempdir().unwrap();
    discard(dir.path(), &dir.path().join("neural-engine-tag.mlmodelc"));
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

/// A protobuf field of `number` holding `body`.
fn delimited(number: u64, body: &[u8], out: &mut Vec<u8>) {
    varint(number << 3 | DELIMITED, out);
    varint(body.len() as u64, out);
    out.extend_from_slice(body);
}
fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
/// A float `TensorProto` named `name` of shape `dims` with `values` inline.
fn tensor(name: &str, dims: &[u64], values: &[f32]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in dims {
        varint(TENSOR_DIMS << 3 | VARINT, &mut out);
        varint(*d, &mut out);
    }
    varint(TENSOR_DATA_TYPE << 3 | VARINT, &mut out);
    varint(FLOAT, &mut out);
    delimited(TENSOR_NAME, name.as_bytes(), &mut out);
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    delimited(TENSOR_RAW_DATA, &raw, &mut out);
    out
}
/// A `ModelProto` whose graph holds `tensors`.
fn model(tensors: &[Vec<u8>]) -> Vec<u8> {
    let mut graph = Vec::new();
    for t in tensors {
        delimited(GRAPH_INITIALIZER, t, &mut graph);
    }
    let mut out = Vec::new();
    // A field before the graph, as an exporter writes the IR version.
    varint(1 << 3 | VARINT, &mut out);
    varint(8, &mut out);
    delimited(MODEL_GRAPH, &graph, &mut out);
    out
}
fn half(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect()
}

#[test]
fn weights_are_scaled_by_input_channel_rounded_to_half_precision_and_laid_out_as_core_ml_reads_them()
 {
    // A convolution's weight [out 2, in 3, 1, 1] and its bias [2].
    let onnx = model(&[
        tensor("w", &[2, 3, 1, 1], &[1., 2., 3., 4., 5., 6.]),
        tensor("b", &[2], &[0.5, -0.25]),
    ]);
    let out = weights_with(&onnx, "w 1 1*1 -1*2\nb - 3\n").unwrap();
    assert_eq!(&out[..8], &[2, 0, 0, 0, 2, 0, 0, 0], "two blobs, version 2");
    assert!(out[8..64].iter().all(|b| *b == 0));
    let meta = |at: usize| {
        let word = |o: usize| u64::from_le_bytes(out[at + o..at + o + 8].try_into().unwrap());
        (
            u32::from_le_bytes(out[at..at + 4].try_into().unwrap()),
            u32::from_le_bytes(out[at + 4..at + 8].try_into().unwrap()),
            word(8) as usize,
            word(16) as usize,
        )
    };
    let (sentinel, kind, size, data) = meta(64);
    assert_eq!((sentinel, kind, size, data), (0xDEAD_BEEF, 1, 12, 128));
    // Channel 0 ×2, channels 1 and 2 ×½, for each output channel.
    assert_eq!(half(&out[data..data + size]), [2., 1., 1.5, 8., 2.5, 3.]);
    // The next blob's metadata at the next multiple of 64; the file ends with its data.
    let (sentinel, kind, size, data) = meta(192);
    assert_eq!((sentinel, kind, size, data), (0xDEAD_BEEF, 1, 4, 256));
    assert_eq!(half(&out[data..]), [4., -2.]);
}

#[test]
fn a_model_without_a_tensor_or_with_scales_that_dont_fit_is_refused() {
    let onnx = model(&[tensor("w", &[2, 3, 1, 1], &[0.; 6])]);
    assert!(matches!(
        weights_with(&onnx, "x 1 0*3\n"),
        Err(InferenceError::ModelInvalid(_))
    ));
    assert!(matches!(
        weights_with(&onnx, "w 1 0*2\n"),
        Err(InferenceError::ModelInvalid(_))
    ));
    assert!(matches!(
        weights_with(&onnx, "w 7 0*3\n"),
        Err(InferenceError::ModelInvalid(_))
    ));
    // Bytes that aren't a model, and a model that isn't RAWmakase's.
    assert!(matches!(
        weights(&[0xff, 0xff]),
        Err(InferenceError::ModelInvalid(_))
    ));
    assert!(matches!(
        weights(&onnx),
        Err(InferenceError::ModelInvalid(_))
    ));
}

#[test]
fn a_folder_without_the_onnx_model_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        NeuralDenoiser::load(dir.path()),
        Err(InferenceError::Failed(_))
    ));
}

/// A folder holding RAWmakase's installed AI Denoise model (`model_bayer.onnx`), named by
/// `RAWMAKASE_NEURAL_ENGINE_MODEL`; the model is too large for the repository. Unset, the
/// test says so and passes.
fn installed() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("RAWMAKASE_NEURAL_ENGINE_MODEL")?);
    assert!(
        dir.join(RAWNIND.model.name).is_file(),
        "{} has no {}",
        dir.display(),
        RAWNIND.model.name
    );
    Some(dir)
}

#[test]
fn the_model_is_made_from_the_installed_one_compiled_once_and_answers_a_tile() {
    let Some(source) = installed() else {
        eprintln!("RAWMAKASE_NEURAL_ENGINE_MODEL is not set: the model isn't run");
        return;
    };
    // A copy, so making the model leaves the installed folder as it is.
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        source.join(RAWNIND.model.name),
        dir.path().join(RAWNIND.model.name),
    )
    .unwrap();
    let mut model = NeuralDenoiser::load(dir.path()).unwrap();
    assert!(model.input_scale > 1. && model.output_scale < 1.);
    let made: Vec<_> = std::fs::read_dir(folder(dir.path()))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        made.iter().filter(|n| n.ends_with(".mlmodelc")).count(),
        1,
        "one compiled model, no work left: {made:?}"
    );
    assert!(
        !made.iter().any(|n| n.starts_with(".neural-engine")),
        "{made:?}"
    );
    let notice = std::fs::read_to_string(folder(dir.path()).join("NOTICE")).unwrap();
    assert!(notice.starts_with(RAWNIND.attribution) && notice.contains(CHANGES));
    let side = RAWNIND.tile;
    // A smooth ramp, as a dim photo's mosaic.
    let input: Vec<f32> = (0..4 * side * side)
        .map(|i| (i % side) as f32 / side as f32 * 0.2)
        .collect();
    let out = model.run(&input, Cancel::NEVER).unwrap();
    assert_eq!(out.len(), 3 * 4 * side * side);
    assert!(out.iter().any(|v| *v != 0.));
    // Loaded again, it takes the compiled model.
    let mut again = NeuralDenoiser::load(dir.path()).unwrap();
    assert_eq!(again.run(&input, Cancel::NEVER).unwrap(), out);
    let cancel = AtomicBool::new(true);
    assert!(matches!(
        again.run(&input, Cancel::new(&cancel)),
        Err(InferenceError::Cancelled)
    ));
    assert!(matches!(
        again.run(&[0.; 4], Cancel::NEVER),
        Err(InferenceError::OutputInvalid(_))
    ));
}
