use super::*;
use crate::process::RgbImage;
use crate::runtime::Sky;
use rawmakase_cancel::Cancel;
use rawmakase_inference::LoadOptions;

/// A protobuf field of `number` holding `body`.
fn delimited(number: u64, body: &[u8], out: &mut Vec<u8>) {
    varint(number << 3 | 2, out);
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
/// A `TensorProto` named `name` of shape `dims`, its data type `kind` (1 for `f32`, 10 for
/// `f16`) and its `raw` bytes inline.
fn tensor(name: &str, dims: &[u64], kind: u64, raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in dims {
        varint(1 << 3, &mut out);
        varint(*d, &mut out);
    }
    varint(2 << 3, &mut out);
    varint(kind, &mut out);
    delimited(8, name.as_bytes(), &mut out);
    delimited(9, raw, &mut out);
    out
}
/// A `ModelProto` whose graph holds `tensors`.
fn model(tensors: &[Vec<u8>]) -> Vec<u8> {
    let mut graph = Vec::new();
    for t in tensors {
        delimited(5, t, &mut graph);
    }
    let mut out = Vec::new();
    varint(1 << 3, &mut out);
    varint(8, &mut out);
    delimited(7, &graph, &mut out);
    out
}
fn halves(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| half::f16::from_f32(*v).to_le_bytes())
        .collect()
}
fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn read(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect()
}

#[test]
fn weights_are_the_models_tensors_in_half_precision_transposed_where_marked() {
    // A linear layer's weight [2, 3] in half precision, and a small bias in single.
    let onnx = model(&[
        tensor("w", &[2, 3], 10, &halves(&[1., 2., 3., 4., 5., 6.])),
        tensor("b", &[2], 1, &floats(&[0.5, -0.25])),
    ]);
    let out = weights_with(&onnx, "w T\nb -\nw -\n").unwrap();
    assert_eq!(
        &out[..8],
        &[3, 0, 0, 0, 2, 0, 0, 0],
        "three blobs, version 2"
    );
    let blob = |at: usize| {
        let word = |o: usize| u64::from_le_bytes(out[at + o..at + o + 8].try_into().unwrap());
        let (size, data) = (word(8) as usize, word(16) as usize);
        read(&out[data..data + size])
    };
    // Its columns become rows.
    assert_eq!(blob(64), [1., 4., 2., 5., 3., 6.]);
    assert_eq!(blob(192), [0.5, -0.25]);
    assert_eq!(blob(320), [1., 2., 3., 4., 5., 6.]);
}

#[test]
fn a_model_without_a_tensor_or_with_one_that_doesnt_fit_is_refused() {
    let onnx = model(&[
        tensor("w", &[2, 3], 10, &halves(&[0.; 6])),
        tensor("v", &[6], 10, &halves(&[0.; 6])),
        tensor("short", &[4], 10, &halves(&[0.; 3])),
    ]);
    for tensors in ["x -\n", "v T\n", "short -\n", "w\n", "w X\n", "w - more\n"] {
        assert!(
            matches!(
                weights_with(&onnx, tensors),
                Err(InferenceError::ModelInvalid(_))
            ),
            "{tensors:?}"
        );
    }
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
fn the_functions_are_the_long_edge_with_each_proportions_short_side_and_its_neighbours() {
    let sizes = functions();
    // 3:2 and 4:3 and 16:9 with one either side, both ways round, and 1023 x 1024, its
    // turn and the square: 3 x 3 x 2 + 3.
    assert_eq!(sizes.len(), 21);
    for size in [
        (1024, 683),
        (683, 1024),
        (1024, 682),
        (1024, 768),
        (1024, 576),
    ] {
        assert!(CoreMlSky::fits(size.0, size.1), "{size:?}");
    }
    assert!(CoreMlSky::fits(1024, 1024) && CoreMlSky::fits(1023, 1024));
    // What Select Sky feeds a 3:2 photo has a function; a panorama's doesn't.
    let fed = crate::runtime::sky_input_size(6000, 4000);
    assert!(CoreMlSky::fits(fed.0, fed.1), "{fed:?}");
    let fed = crate::runtime::sky_input_size(12000, 3000);
    assert!(!CoreMlSky::fits(fed.0, fed.1), "{fed:?}");
}

#[test]
fn a_folder_without_the_onnx_model_is_refused_and_a_size_without_a_function_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut sky = CoreMlSky::new(dir.path());
    assert!(sky.run(&[0.; 3 * 100 * 100], 100, 100).is_err());
    if supported() {
        assert!(matches!(
            sky.run(&vec![0.; 3 * 1024 * 683], 1024, 683),
            Err(InferenceError::Failed(_))
        ));
        assert!(!folder(dir.path()).join("NOTICE").exists());
    }
}

/// A folder holding RAWmakase's installed sky model, named by `RAWMAKASE_SKY_MODEL`; the
/// model is too large for the repository. Unset, the test says so and passes.
fn installed() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("RAWMAKASE_SKY_MODEL")?);
    assert!(
        dir.join(SKY.file.name).is_file(),
        "{} has no {}",
        dir.display(),
        SKY.file.name
    );
    Some(dir)
}

/// A photo of `width` x `height`: blue above two fifths of it, green below.
fn landscape(width: usize, height: usize) -> RgbImage {
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for _ in 0..width {
            data.extend(if y * 5 < height * 2 {
                [110, 160, 230]
            } else {
                [60, 110, 40]
            });
        }
    }
    RgbImage {
        width,
        height,
        data,
    }
}

#[test]
fn the_model_is_made_from_the_installed_one_and_selects_as_the_cpu_does() {
    let Some(source) = installed() else {
        eprintln!("RAWMAKASE_SKY_MODEL is not set: the model isn't run");
        return;
    };
    if !supported() {
        eprintln!("this computer can't run the converted model");
        return;
    }
    // A copy, so making the model leaves the installed folder as it is.
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(source.join(SKY.file.name), dir.path().join(SKY.file.name)).unwrap();
    let options = LoadOptions::default();
    let photo = landscape(1536, 1024);
    let mut gpu = Sky::load_on_gpu(dir.path(), &options).unwrap();
    assert_eq!(gpu.provider(), rawmakase_inference::Provider::CoreML);
    let on_gpu = gpu.select(&photo, Cancel::NEVER).unwrap();
    assert_eq!(
        gpu.provider(),
        rawmakase_inference::Provider::CoreML,
        "it ran on the GPU"
    );
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
    assert!(!made.iter().any(|n| n.starts_with(".core-ml")), "{made:?}");
    let notice = std::fs::read_to_string(folder(dir.path()).join("NOTICE")).unwrap();
    assert!(notice.starts_with(SKY.attribution) && notice.contains(CHANGES));
    let mut cpu = Sky::load_with(dir.path(), &options).unwrap();
    let on_cpu = cpu.select(&photo, Cancel::NEVER).unwrap();
    let (a, b) = (&on_gpu.data, &on_cpu.data);
    assert_eq!(a.len(), b.len());
    let both = a
        .iter()
        .zip(b)
        .filter(|(x, y)| **x > 127 && **y > 127)
        .count();
    let either = a
        .iter()
        .zip(b)
        .filter(|(x, y)| **x > 127 || **y > 127)
        .count();
    assert!(
        either > a.len() / 5,
        "the sky is found: {either} of {}",
        a.len()
    );
    assert!(both as f64 / either as f64 > 0.99, "{both} of {either}");
    // A panorama's size has no function: it is selected on the CPU, in the same session.
    gpu.select(&landscape(3000, 600), Cancel::NEVER).unwrap();
    assert_eq!(gpu.provider(), rawmakase_inference::Provider::Cpu);
}
