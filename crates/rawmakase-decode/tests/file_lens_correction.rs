//! A DNG's always-applied lens correction (OpcodeList3: FixVignetteRadial,
//! WarpRectilinear and GainMap, as drones and phones write them) switched off
//! (`Recipe::lens_file_correction`) renders exactly as the same file with its
//! opcode list emptied. The photos are synthetic RGGB mosaic DNGs, opened and
//! decoded as the app opens them (#1289).
use rawmakase_cancel::Cancel;
use rawmakase_engine::develop::PreviewRenderer;
use rawmakase_model::camera_data::{CameraImage, Decode, Demosaic};
use rawmakase_model::model::recipe::Recipe;

const W: u32 = 96;
const H: u32 = 64;

/// A little-endian TIFF entry of `kind` (3 short, 4 long, 7 undefined, 10
/// srational) with its value bytes.
fn entry(tag: u16, kind: u16, values: &[i64]) -> (u16, u16, u32, Vec<u8>) {
    let bytes: Vec<u8> = match kind {
        3 => values
            .iter()
            .flat_map(|v| (*v as u16).to_le_bytes())
            .collect(),
        4 => values
            .iter()
            .flat_map(|v| (*v as u32).to_le_bytes())
            .collect(),
        10 => (values.iter())
            .flat_map(|v| [*v as i32, 10000].into_iter().flat_map(i32::to_le_bytes))
            .collect(),
        _ => values.iter().map(|v| *v as u8).collect(),
    };
    let count = bytes.len() / [1, 1, 1, 2, 4, 1, 1, 1, 1, 1, 8][kind as usize];
    (tag, kind, count as u32, bytes)
}

/// A big-endian opcode list of `ops` (id, parameters as doubles).
fn list(ops: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut b = (ops.len() as u32).to_be_bytes().to_vec();
    for (id, params) in ops {
        for v in [*id, 0x0103_0000, 0, params.len() as u32] {
            b.extend(v.to_be_bytes());
        }
        b.extend(params);
    }
    b
}
fn be(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A drone's OpcodeList3, in its order: a FixVignetteRadial, a WarpRectilinear
/// (barrel distortion on one plane) and a GainMap over the whole image, brighter
/// toward the right and different per channel.
fn opcode_list3() -> Vec<u8> {
    let mut warp = 1u32.to_be_bytes().to_vec();
    warp.extend(be(&[1., 0.03, 0., 0., 0., 0., 0.5, 0.5]));
    let mut gains: Vec<u8> = [0, 0, H, W, 0, 3, 1, 1, 2, 3]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    gains.extend(be(&[1., 0.5, 0., 0.]));
    gains.extend(3u32.to_be_bytes());
    for i in 0..6 {
        let x = (i % 3) as f32;
        let point = [1. + 0.4 * x, 1. + 0.2 * x, 1. + 0.1 * x];
        gains.extend(point.iter().flat_map(|g| g.to_be_bytes()));
    }
    let vignette = be(&[0.3, 0.1, 0., 0., 0., 0.5, 0.5]);
    list(&[(3, vignette), (1, warp), (9, gains)])
}

/// An RGGB mosaic DNG of a smooth scene, with `opcodes` as its OpcodeList3.
fn dng(opcodes: &[u8]) -> Vec<u8> {
    let samples: Vec<u8> = (0..W * H)
        .flat_map(|i| {
            let (x, y) = (i % W, i / W);
            let c = [[0, 1], [1, 2]][(y % 2) as usize][(x % 2) as usize];
            let light = 0.15 + 0.25 * (x as f32 / W as f32) + 0.1 * (y as f32 * 0.2).sin();
            let dn = 256. + light * [0.5, 1., 0.6][c] * 16127.;
            (dn.round() as u16).to_le_bytes()
        })
        .collect();
    let opcodes: Vec<i64> = opcodes.iter().map(|b| i64::from(*b)).collect();
    // Header, directory, its values, then the strip.
    let mut entries = vec![
        entry(256, 4, &[W.into()]),
        entry(257, 4, &[H.into()]),
        entry(258, 3, &[16]),
        entry(259, 3, &[1]),
        entry(262, 3, &[32803]),
        entry(273, 4, &[0]),
        entry(277, 3, &[1]),
        entry(278, 4, &[H.into()]),
        entry(279, 4, &[samples.len() as i64]),
        entry(33421, 3, &[2, 2]),
        entry(33422, 7, &[0, 1, 1, 2]),
        entry(50706, 7, &[1, 4, 0, 0]),
        entry(50714, 3, &[256]),
        entry(50717, 3, &[16383]),
        entry(
            50721,
            10,
            &[12000, -3000, -1000, -3000, 12000, 1000, 0, 1000, 6000],
        ),
        entry(50778, 3, &[21]),
        entry(51022, 7, &opcodes),
    ];
    let directory = |entries: &[(u16, u16, u32, Vec<u8>)]| {
        let values_at = 8 + 2 + entries.len() * 12 + 4;
        let mut out = (entries.len() as u16).to_le_bytes().to_vec();
        let mut values = Vec::new();
        for (tag, kind, count, bytes) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                out.extend(bytes.iter().copied().chain([0; 4]).take(4));
            } else {
                out.extend(((values_at + values.len()) as u32).to_le_bytes());
                values.extend(bytes);
                values.resize(values.len().next_multiple_of(2), 0);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(values);
        out
    };
    let strip_at = 8 + directory(&entries).len();
    entries[5] = entry(273, 4, &[strip_at as i64]);
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    out.extend(directory(&entries));
    out.extend(samples);
    out
}

fn developed(bytes: Vec<u8>, name: &str) -> CameraImage {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    rawmakase_decode::photo::open(&path)
        .unwrap()
        .develop(Decode::Full(Demosaic::Rawmakase), Cancel::NEVER)
        .unwrap()
}

#[test]
fn a_dngs_own_correction_switched_off_renders_as_the_file_without_its_opcodes() {
    let with = developed(dng(&opcode_list3()), "with.dng");
    let without = developed(dng(&list(&[])), "without.dng");
    let lens = with.metadata.lens.as_ref().expect("the opcodes are read");
    assert!(lens.default_on && lens.vignetting_measured);
    assert!(lens.distortion.is_some() && lens.gain_map.is_some());
    assert!(without.metadata.lens.is_none());
    assert_eq!(
        with.pixels, without.pixels,
        "OpcodeList3 applies after decoding"
    );

    // Settings that read the photo's measures, so the measurement copy counts too.
    let on = Recipe {
        shadows: 0.4,
        highlights: -0.3,
        effects: rawmakase_model::model::effects::Effects {
            clarity: 0.3,
            dehaze: 0.2,
            ..Default::default()
        },
        ..Recipe::for_metadata(&with.metadata)
    };
    let off = Recipe {
        lens_file_correction: false,
        ..on.clone()
    };
    // One renderer throughout, so its stage cache (samples, measurement copy)
    // can't hand back what the other setting made.
    let mut renderer = PreviewRenderer::default();
    let mut shown = |im: &CameraImage, r: &Recipe| {
        (renderer.render(im, r, 0, None, Cancel::NEVER))
            .unwrap()
            .pixels
    };
    let corrected = shown(&with, &on);
    let switched_off = shown(&with, &off);
    assert_ne!(corrected, switched_off);
    assert_eq!(switched_off, shown(&without, &off));
    assert_eq!(shown(&with, &on), corrected);
}
