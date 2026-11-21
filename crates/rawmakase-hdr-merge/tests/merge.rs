//! Merges of synthetic brackets: a known radiance map sampled at three
//! exposures with shot and read noise, clipped and quantised to 14 bits, written
//! as RGGB mosaic DNGs whose EXIF rounds the shutter speeds as cameras do.
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::bracket::Refusal;
use rawmakase_hdr_merge::{
    Deghost, Destination, Error, Extras, FrameInput, MergeOptions, Merged, Stage, WHITE, merge,
    write_dng,
};
use rawmakase_model::camera_data::{Decode, Demosaic};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const W: u32 = 384;
const H: u32 = 256;
const BLACK: f64 = 256.;
const WHITE_DN: f64 = 16383.;
/// Electrons per DN and the read noise in DN.
const GAIN: f64 = 2.;
const READ: f64 = 3.;
/// The frames' true exposures, relative to the darkest, and the shutter
/// speeds their EXIF records: 1/256, 1/64 and 1/16 s, recorded as cameras
/// round them.
const TRUE: [f64; 3] = [1., 4., 16.];
const RECORDED: [(u32, u32); 3] = [(1, 250), (1, 60), (1, 15)];
/// The camera's color of a neutral: camera RGB is not white balanced.
const NEUTRAL: [f64; 3] = [0.5, 1., 0.6];

/// The scene's light per pixel and channel, in the darkest frame's units (1
/// reaches the white level): flat blocks from 2^-14 to 2 above, and two
/// gradients across the same range below.
fn scene() -> Vec<[f64; 3]> {
    (0..H)
        .flat_map(|y| (0..W).map(move |x| light(f64::from(x), f64::from(y))))
        .collect()
}

/// The scene's light at any place, the frame's edge extending beyond it.
fn light(x: f64, y: f64) -> [f64; 3] {
    let x = x.clamp(0., f64::from(W - 1));
    let y = y.clamp(0., f64::from(H - 1));
    if y < 128. {
        let block = ((y / 32.).floor() * f64::from(W / 32) + (x / 32.).floor()).min(47.);
        let level = 2f64.powf(-14. + 15. * block / 47.);
        let tint = [[1., 0.8, 0.6], [0.6, 1., 0.9], [0.9, 0.7, 1.]][(block as usize) % 3];
        std::array::from_fn(|c| level * tint[c] * NEUTRAL[c])
    } else {
        let level = 2f64.powf(-14. + 15. * x / f64::from(W - 1));
        let tint = if y < 192. { [1.; 3] } else { [1., 0.7, 0.4] };
        std::array::from_fn(|c| level * tint[c] * NEUTRAL[c])
    }
}

/// The scene as a frame moved by `dx`, `dy` pixels and turned `degrees`
/// about its centre sees it: its pixel `p` sees the scene's place `p` moved
/// back, blurred over about two pixels as a lens and sensor blur it (a
/// tent, sampled 8 × 8), so the blocks' edges move by fractions of a pixel
/// too and the demosaic sees no sharper edge than a camera gives it.
fn moved(dx: f64, dy: f64, degrees: f64) -> Vec<[f64; 3]> {
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = (f64::from(W - 1) / 2., f64::from(H - 1) / 2.);
    (0..H)
        .flat_map(|y| {
            (0..W).map(move |x| {
                let (mut sum, mut total) = ([0.; 3], 0.);
                for j in 0..8 {
                    for i in 0..8 {
                        let (a, b) = ((f64::from(i) - 3.5) / 4., (f64::from(j) - 3.5) / 4.);
                        let weight = (1. - a.abs()) * (1. - b.abs());
                        let u = f64::from(x) + a - dx - cx;
                        let v = f64::from(y) + b - dy - cy;
                        let l = light(c * u + s * v + cx, -s * u + c * v + cy);
                        for k in 0..3 {
                            sum[k] += weight * l[k];
                        }
                        total += weight;
                    }
                }
                sum.map(|v| v / total)
            })
        })
        .collect()
}

/// A small deterministic generator of normal deviates.
struct Noise(u64);
impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2. * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// The RGGB mosaic of `scene` exposed `exposure` times the darkest frame, in
/// DN with noise, clipped and quantised.
fn mosaic(scene: &[[f64; 3]], exposure: f64, seed: u64) -> Vec<u16> {
    let mut noise = Noise(seed);
    (0..H)
        .flat_map(|y| (0..W).map(move |x| (x, y)))
        .map(|(x, y)| {
            let c = [[0, 1], [1, 2]][(y % 2) as usize][(x % 2) as usize];
            let dn = scene[(y * W + x) as usize][c] * exposure * (WHITE_DN - BLACK);
            let sigma = (dn.max(0.) / GAIN + READ * READ).sqrt();
            (BLACK + dn + sigma * noise.normal())
                .round()
                .clamp(0., WHITE_DN) as u16
        })
        .collect()
}

/// A little-endian TIFF entry: tag, type, count, value bytes.
type Entry = (u16, u16, u32, Vec<u8>);

fn short(tag: u16, v: &[u16]) -> Entry {
    (
        tag,
        3,
        v.len() as u32,
        v.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
}
fn long(tag: u16, v: &[u32]) -> Entry {
    (
        tag,
        4,
        v.len() as u32,
        v.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    (tag, 2, b.len() as u32, b)
}
fn rational(tag: u16, v: &[(u32, u32)]) -> Entry {
    let b = v
        .iter()
        .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
        .collect();
    (tag, 5, v.len() as u32, b)
}
fn srational(tag: u16, v: &[f64]) -> Entry {
    let b = v
        .iter()
        .flat_map(|x| {
            ((x * 10000.).round() as i32)
                .to_le_bytes()
                .into_iter()
                .chain(10000i32.to_le_bytes())
        })
        .collect();
    (tag, 10, v.len() as u32, b)
}

/// A directory of `entries` at `at` in a file, its values after it; its bytes,
/// and where in them each value that didn't fit in its entry went.
fn directory(mut entries: Vec<Entry>, at: usize) -> Vec<u8> {
    entries.sort_by_key(|e| e.0);
    let values_at = at + 2 + entries.len() * 12 + 4;
    let mut out = (entries.len() as u16).to_le_bytes().to_vec();
    let mut values = Vec::new();
    for (tag, kind, count, bytes) in &entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            out.extend(bytes.iter().copied().chain([0; 4]).take(4));
        } else {
            out.extend(((values_at + values.len()) as u32).to_le_bytes());
            values.extend(bytes);
            if values.len() % 2 == 1 {
                values.push(0);
            }
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(values);
    out
}

/// A mosaic DNG of `samples`, its EXIF recording `shutter` seconds at f/8,
/// ISO 100, taken `subseconds` into a second.
fn dng(samples: &[u16], shutter: (u32, u32), subseconds: &str) -> Vec<u8> {
    let exif = vec![
        rational(33434, &[shutter]),
        rational(33437, &[(8, 1)]),
        short(34855, &[100]),
        ascii(36867, "2026:11:18 12:00:00"),
        ascii(37521, subseconds),
    ];
    // Header, the EXIF directory, the main directory, the strip.
    let exif_at = 8;
    let exif = directory(exif, exif_at);
    let main_at = exif_at + exif.len();
    let strip: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    // The main directory's length doesn't depend on the strip's offset.
    let main = |strip_at: u32| {
        directory(
            vec![
                long(254, &[0]),
                long(256, &[W]),
                long(257, &[H]),
                short(258, &[16]),
                short(259, &[1]),
                short(262, &[32803]),
                ascii(271, "RAWmakase"),
                ascii(272, "Synthetic Bracket"),
                long(273, &[strip_at]),
                short(274, &[1]),
                short(277, &[1]),
                long(278, &[H]),
                long(279, &[strip.len() as u32]),
                short(284, &[1]),
                short(33421, &[2, 2]),
                (33422, 1, 4, vec![0, 1, 1, 2]),
                long(34665, &[exif_at as u32]),
                (50706, 1, 4, vec![1, 4, 0, 0]),
                (50707, 1, 4, vec![1, 1, 0, 0]),
                ascii(50708, "RAWmakase Synthetic Bracket"),
                short(50714, &[BLACK as u16]),
                short(50717, &[WHITE_DN as u16]),
                srational(50721, &[1.2, -0.3, -0.1, -0.3, 1.2, 0.1, 0., 0.1, 0.6]),
                rational(50728, &NEUTRAL.map(|v| ((v * 10000.) as u32, 10000))),
                short(50778, &[21]),
            ],
            main_at,
        )
    };
    let len = main(0).len();
    let strip_at = (main_at + len) as u32;
    let mut out = b"II*\0".to_vec();
    out.extend((main_at as u32).to_le_bytes());
    out.extend(exif);
    out.extend(main(strip_at));
    out.extend(strip);
    out
}

/// The bracket in `dir`, the frames out of exposure order.
fn bracket(dir: &Path) -> Vec<PathBuf> {
    let scene = scene();
    [1usize, 0, 2]
        .iter()
        .map(|&k| {
            let path = dir.join(format!("IMG_{}.dng", k + 1));
            let samples = mosaic(&scene, TRUE[k], 7 + k as u64);
            std::fs::write(
                &path,
                dng(&samples, RECORDED[k], &format!("{}", 10 + k * 30)),
            )
            .unwrap();
            path
        })
        .collect()
}

fn options() -> MergeOptions {
    MergeOptions {
        demosaic: Demosaic::Rawmakase,
        align: true,
        deghost: Deghost::None,
        software: "RAWmakase test".into(),
    }
}

fn inputs(paths: &[PathBuf]) -> Vec<FrameInput> {
    paths.iter().map(|p| FrameInput::read(p).unwrap()).collect()
}

fn merged(paths: &[PathBuf]) -> Merged {
    merge(&inputs(paths), &options(), &|_, _| {}, Cancel::NEVER).unwrap()
}

/// The merged DNG read back as the app develops it, not white balanced: the
/// light per pixel in the darkest frame's units.
fn developed(path: &Path) -> (Vec<[f64; 3]>, f64) {
    let raw = rawmakase_decode::photo::open(path).unwrap();
    let wb = raw.metadata.wb;
    let baseline = f64::from(raw.metadata.baseline_exposure.unwrap());
    let image = raw
        .develop(Decode::Full(Demosaic::Rawmakase), Cancel::NEVER)
        .unwrap();
    let pixels = image
        .pixels
        .iter()
        .map(|p| std::array::from_fn(|c| f64::from(p[c] / wb[c])))
        .collect();
    (pixels, baseline)
}

/// A pixel's level: the sum of its channels.
fn level(p: [f64; 3]) -> f64 {
    p.iter().sum()
}

/// One merge of the bracket, written beside it, shared by the tests that only
/// read it.
fn written() -> &'static (tempfile::TempDir, Merged, PathBuf) {
    static WRITTEN: std::sync::OnceLock<(tempfile::TempDir, Merged, PathBuf)> =
        std::sync::OnceLock::new();
    WRITTEN.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let paths = bracket(dir.path());
        let merged = merged(&paths);
        let path = write_dng(
            &merged,
            &Extras::default(),
            &Destination::beside(&paths[0]),
            Cancel::NEVER,
        )
        .unwrap();
        (dir, merged, path)
    })
}

/// The bracket in `dir` taken by hand: the darkest and brightest frames
/// moved by `motion` (dx, dy, degrees) each.
fn handheld(dir: &Path, motion: [[f64; 3]; 3]) -> Vec<PathBuf> {
    (0..3)
        .map(|k| {
            let path = dir.join(format!("IMG_{}.dng", k + 1));
            let [dx, dy, degrees] = motion[k];
            let samples = mosaic(&moved(dx, dy, degrees), TRUE[k], 7 + k as u64);
            std::fs::write(
                &path,
                dng(&samples, RECORDED[k], &format!("{}", 10 + k * 30)),
            )
            .unwrap();
            path
        })
        .collect()
}

/// The blocks whose merged light is further from the scene than their noise
/// allows, away from their edges and where the darkest frame clips.
fn wrong_blocks(pixels: &[[f64; 3]], scene: &[[f64; 3]]) -> Vec<u32> {
    wrong_blocks_within(pixels, scene, f64::INFINITY)
}

/// [`wrong_blocks`], the noise allowed that of a merge of the frames
/// exposed at most `most` times the darkest.
fn wrong_blocks_within(pixels: &[[f64; 3]], scene: &[[f64; 3]], most: f64) -> Vec<u32> {
    (0..48u32)
        .filter(|block| {
            let (bx, by) = (block % 12 * 32, block / 12 * 32);
            let (mut got, mut want, mut n) = (0., 0., 0.);
            for y in by + 6..by + 26 {
                for x in bx + 6..bx + 26 {
                    got += level(pixels[(y * W + x) as usize]);
                    want += level(scene[(y * W + x) as usize]);
                    n += 1.;
                }
            }
            let (got, want) = (got / n, want / n);
            let exposure = TRUE
                .iter()
                .copied()
                .filter(|t| want * t < 0.8 && *t <= most)
                .fold(1., f64::max);
            let dn = want * exposure * (WHITE_DN - BLACK);
            let sigma = (dn / GAIN + 3. * READ * READ).sqrt() / (exposure * (WHITE_DN - BLACK));
            // The demosaic correlates neighbouring pixels: on a flat frame a
            // 20×20 block's mean varied 1.8 times as much as independent
            // pixels would give. A quarter of the pixels as independent
            // samples leaves about twice that as headroom.
            let bound = 6. * sigma / (n / 4.).sqrt() + want * 2e-3;
            let clipped = scene[((by + 16) * W + bx + 16) as usize]
                .into_iter()
                .fold(0., f64::max)
                > 0.9;
            !clipped && (got - want).abs() >= bound
        })
        .collect()
}

/// A bracket taken by hand is aligned to its middle frame: the motion is
/// found to a fifth of a pixel, the merge follows the scene as the reference
/// frame saw it, and what the moved frames leave uncovered is the DNG's
/// transparency mask. Without Auto Align the same bracket merges blurred.
#[test]
fn a_handheld_bracket_is_aligned() {
    let dir = tempfile::tempdir().unwrap();
    let motion = [[6.5, -4.25, 0.5], [0.; 3], [-5.2, 3.1, -0.3]];
    let paths = handheld(dir.path(), motion);
    let merged = merged(&paths);
    assert_eq!(merged.reference, 1);
    for (frame, [dx, dy, degrees]) in merged.frames.iter().zip(motion) {
        let Some(a) = &frame.alignment else {
            assert_eq!((dx, dy, degrees), (0., 0., 0.));
            continue;
        };
        assert!(a.warped);
        // The frame's pixel for the reference's centre is the centre moved.
        assert!(
            (a.shift[0] - dx).abs() < 0.2 && (a.shift[1] - dy).abs() < 0.2,
            "{a:?} for {dx}, {dy}"
        );
        assert!((a.rotation - degrees).abs() < 0.05, "{a:?} for {degrees}°");
    }
    let path = write_dng(
        &merged,
        &Extras::default(),
        &Destination::beside(&paths[1]),
        Cancel::NEVER,
    )
    .unwrap();
    let (pixels, _) = developed(&path);
    let scene = scene();
    let wrong = wrong_blocks(&pixels, &scene);
    assert!(wrong.is_empty(), "blocks {wrong:?}");
    // The mask: the frames' moved edges are uncovered, the middle covered.
    let raw = rawmakase_decode::photo::open(&path).unwrap();
    let coverage = raw.metadata.coverage.as_ref().expect("a transparency mask");
    let at = |x: u32, y: u32| coverage.values[(y * coverage.width + x) as usize];
    assert_eq!((at(0, 0), at(W - 1, H - 1)), (0, 0));
    assert_eq!(at(W / 2, H / 2), 255);
    // Without Auto Align, the edges of most blocks are smeared.
    let unaligned = merge(
        &inputs(&paths),
        &MergeOptions {
            align: false,
            ..options()
        },
        &|_, _| {},
        Cancel::NEVER,
    )
    .unwrap();
    assert!(unaligned.frames.iter().all(|f| f.alignment.is_none()));
    let path = dir.path().join("unaligned.dng");
    write_dng(
        &unaligned,
        &Extras::default(),
        &Destination::Exactly(path.clone()),
        Cancel::NEVER,
    )
    .unwrap();
    let (pixels, _) = developed(&path);
    assert!(
        wrong_blocks(&pixels, &scene).len() > 10,
        "{:?}",
        wrong_blocks(&pixels, &scene)
    );
}

/// The bracket in `dir` with something that moved: the darkest frame shows
/// a block the others don't, and another where the reference clips; the
/// brightest a block elsewhere. Each is (frame, columns, rows, light).
fn with_movers(dir: &Path) -> Vec<PathBuf> {
    let movers = [
        (0, 64..128, 32..96, 0.05),
        (0, 288..352, 100..128, 0.01),
        (2, 224..288, 0..64, 0.002),
    ];
    (0..3)
        .map(|k| {
            let mut light = scene();
            for (frame, columns, rows, level) in movers.clone() {
                if frame != k {
                    continue;
                }
                for y in rows {
                    for x in columns.clone() {
                        light[(y * W + x) as usize] = NEUTRAL.map(|n| level * n);
                    }
                }
            }
            let path = dir.join(format!("IMG_{}.dng", k + 1));
            let samples = mosaic(&light, TRUE[k], 7 + k as u64);
            std::fs::write(
                &path,
                dng(&samples, RECORDED[k], &format!("{}", 10 + k * 30)),
            )
            .unwrap();
            path
        })
        .collect()
}

/// Deghosting takes what moved from the reference frame: merged without it,
/// the blocks something moved over in another frame show its ghost; with it,
/// every block follows the scene as the reference saw it, within the noise
/// the reference's exposure leaves. Where the reference clipped, what the
/// darkest frame alone saw comes from it whole, not as a hole. (A tripod
/// bracket: the darkest frame's block is most of what it exposes well, so
/// Auto Align would refuse it as disagreeing with its fit.)
#[test]
fn what_moved_comes_from_the_reference() {
    let dir = tempfile::tempdir().unwrap();
    let paths = with_movers(dir.path());
    let scene = scene();
    let merge_with = |deghost: Deghost, name: &str| {
        let merged = merge(
            &inputs(&paths),
            &MergeOptions {
                deghost,
                align: false,
                ..options()
            },
            &|_, _| {},
            Cancel::NEVER,
        )
        .unwrap();
        let path = dir.path().join(name);
        write_dng(
            &merged,
            &Extras::default(),
            &Destination::Exactly(path.clone()),
            Cancel::NEVER,
        )
        .unwrap();
        (merged, developed(&path).0)
    };
    let (merged, ghosted) = merge_with(Deghost::None, "none.dng");
    assert!(merged.frames.iter().all(|f| f.deghosted == 0.));
    let wrong = wrong_blocks_within(&ghosted, &scene, 4.);
    for block in [14, 15, 26, 27, 7, 8, 19, 20] {
        assert!(wrong.contains(&block), "{block} in {wrong:?}");
    }
    for deghost in [Deghost::Low, Deghost::Medium, Deghost::High] {
        let (merged, pixels) = merge_with(deghost, &format!("{deghost:?}.dng"));
        let deghosted: Vec<f64> = merged.frames.iter().map(|f| f.deghosted).collect();
        assert!(
            deghosted[0] > 0. && deghosted[1] == 0. && deghosted[2] > 0.,
            "{deghost:?}: {deghosted:?}"
        );
        // Under a tenth of each frame: the blocks, dilated and feathered.
        assert!(
            deghosted.iter().all(|d| *d < 0.1),
            "{deghost:?}: {deghosted:?}"
        );
        let wrong = wrong_blocks_within(&pixels, &scene, 4.);
        assert!(wrong.is_empty(), "{deghost:?}: blocks {wrong:?}");
        // The darkest frame's block where the reference clipped.
        let (mut got, mut n) = (0., 0.);
        for y in 104..124 {
            for x in 296..344 {
                got += level(pixels[(y * W + x) as usize]);
                n += 1.;
            }
        }
        let want = 0.01 * NEUTRAL.iter().sum::<f64>();
        assert!(
            (got / n / want - 1.).abs() < 0.05,
            "{deghost:?}: {} for {want}",
            got / n
        );
    }
}

/// A bracket in which nothing moved leaves nothing out at any level.
#[test]
fn noise_alone_is_not_deghosted() {
    let dir = tempfile::tempdir().unwrap();
    let paths = bracket(dir.path());
    for deghost in [Deghost::Low, Deghost::Medium, Deghost::High] {
        let merged = merge(
            &inputs(&paths),
            &MergeOptions {
                deghost,
                ..options()
            },
            &|_, _| {},
            Cancel::NEVER,
        )
        .unwrap();
        let deghosted: Vec<f64> = merged.frames.iter().map(|f| f.deghosted).collect();
        assert_eq!(deghosted, [0.; 3], "{deghost:?}");
    }
}

/// A tripod bracket stays as it was taken: Auto Align finds every frame
/// within half a pixel of the reference everywhere, and leaves uncovered at
/// most a pixel's border.
#[test]
fn a_tripod_bracket_stays_in_place() {
    let (_, merged, path) = written();
    let aligned: Vec<_> = merged
        .frames
        .iter()
        .filter_map(|f| f.alignment.as_ref())
        .collect();
    assert_eq!(aligned.len(), 2);
    let (w, h) = (f64::from(W - 1), f64::from(H - 1));
    for a in aligned {
        let m = &a.matrix;
        for (x, y) in [(0., 0.), (w, 0.), (0., h), (w, h)] {
            let d = m[2][0] * x + m[2][1] * y + m[2][2];
            let u = (m[0][0] * x + m[0][1] * y + m[0][2]) / d;
            let v = (m[1][0] * x + m[1][1] * y + m[1][2]) / d;
            assert!((u - x).hypot(v - y) < 0.5, "{a:?}");
        }
    }
    let raw = rawmakase_decode::photo::open(path).unwrap();
    if let Some(coverage) = &raw.metadata.coverage {
        let (w, h) = (coverage.width, coverage.height);
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                assert_eq!(coverage.values[(y * w + x) as usize], 255, "{x}, {y}");
            }
        }
    }
}

/// The exposures between frames are measured: 2 stops each, where the EXIF's
/// rounded shutter speeds say 2.06 and 2.
#[test]
fn exposure_ratios_are_measured_not_read() {
    let (_, merged, _) = written();
    let ev: Vec<f64> = merged.frames.iter().map(|f| f.ev).collect();
    for (got, want) in ev.iter().zip([0., 2., 4.]) {
        assert!((got - want).abs() < 0.01, "{ev:?}");
    }
    assert!(merged.frames.iter().all(|f| f.measured));
    assert!((merged.frames[1].nominal_ev - (250f64 / 60.).log2()).abs() < 1e-5);
    // Merged from the darkest (IMG_1, the second input) to the brightest.
    assert_eq!(
        merged.frames.iter().map(|f| f.input).collect::<Vec<_>>(),
        [1, 0, 2]
    );
}

/// The middle frame is the reference: the DNG renders at its brightness, two
/// stops over the darkest frame, whose white is the DNG's.
#[test]
fn the_dng_renders_at_the_reference_frames_brightness() {
    let (_, merged, path) = written();
    assert_eq!(merged.reference, 1);
    assert!(!merged.reference_moved);
    assert!((merged.baseline_exposure - 2.).abs() < 0.01);
    assert_eq!(path.file_name().unwrap(), "IMG_2-HDR.dng");
    let raw = rawmakase_decode::photo::open(path).unwrap();
    let m = &raw.metadata;
    assert!((f64::from(m.baseline_exposure.unwrap()) - merged.baseline_exposure).abs() < 1e-4);
    assert_eq!((m.width, m.height), (W, H));
    for (got, want) in m.wb.iter().zip([2., 1., 1. / 0.6]) {
        assert!((f64::from(*got) - want).abs() < 1e-3, "{:?}", m.wb);
    }
    assert!(
        (m.shutter - 1. / 250.).abs() < 1e-6,
        "the darkest frame's EXIF"
    );
    assert_eq!(WHITE, 32768);
}

/// The merged light is the scene's within its noise, from 2^-14 of the darkest
/// frame's white to its clipping: no seam where the merge passes from one
/// frame to the next at their clip ramps, and the 0.06 stop the EXIF's rounded
/// shutter speeds would put there is not there.
#[test]
fn the_merge_follows_the_scene_without_seams() {
    let (_, _, path) = written();
    let (pixels, _) = developed(path);
    let scene = scene();
    // Over windows of 8 columns of a gradient's rows, away from their edges:
    // within 0.01 stop of the scene everywhere, and 0.002 on average, from
    // 2^-9 of the darkest frame's white, where the brightest frame's noise
    // over a window is about 0.002 stop, to where the darkest clips.
    for rows in [132..188, 196..252] {
        let (mut worst, mut sum, mut windows) = ((0f64, 0), 0., 0.);
        for x in (4..W - 12).step_by(8) {
            let (mut got, mut want) = (0., 0.);
            for y in rows.clone() {
                for x in x..x + 8 {
                    got += level(pixels[(y * W + x) as usize]);
                    want += level(scene[(y * W + x) as usize]);
                }
            }
            let brightest = scene[(rows.start * W + x + 7) as usize]
                .into_iter()
                .fold(0., f64::max);
            if want / (rows.len() * 8) as f64 > 2f64.powi(-9) && brightest < 0.9 {
                let off = (got / want).log2();
                sum += off;
                windows += 1.;
                if off.abs() > worst.0.abs() {
                    worst = (off, x);
                }
            }
        }
        assert!(worst.0.abs() < 0.01, "{worst:?} stops off at rows {rows:?}");
        assert!((sum / windows).abs() < 0.002, "{} stops off", sum / windows);
    }
    // Every block, to within its noise.
    let wrong = wrong_blocks(&pixels, &scene);
    assert!(wrong.is_empty(), "blocks {wrong:?}");
}

/// In the shadows the merge is about as noisy as the brightest frame alone.
#[test]
fn shadow_noise_is_the_brightest_frames() {
    let (dir, _, path) = written();
    let (merged, _) = developed(path);
    let brightest = rawmakase_decode::photo::open(&dir.path().join("IMG_3.dng"))
        .unwrap()
        .develop_frame(Demosaic::Rawmakase, Cancel::NEVER)
        .unwrap();
    // Block 8: about 2^-11.4 of the darkest frame's white.
    let (bx, by) = (8 * 32, 0);
    let spread = |values: &[f64]| {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
    };
    let block = |f: &dyn Fn(usize) -> f64| {
        let mut v = Vec::new();
        for y in by + 4..by + 28 {
            for x in bx + 4..bx + 28 {
                v.push(f((y * W + x) as usize));
            }
        }
        spread(&v)
    };
    let merged_noise = block(&|i| merged[i][1]);
    let alone = block(&|i| f64::from(brightest.pixels[i][1]) / TRUE[2]);
    let ratio = merged_noise / alone;
    assert!(
        (0.8..1.05).contains(&ratio),
        "{merged_noise} against {alone}"
    );
}

/// Frames that don't show one scene are refused, by name.
#[test]
fn another_scene_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = bracket(dir.path());
    // The brightest frame of a scene a stop darker.
    let darker: Vec<[f64; 3]> = scene().iter().map(|p| p.map(|v| v / 2.)).collect();
    let path = dir.path().join("IMG_3.dng");
    std::fs::write(&path, dng(&mosaic(&darker, TRUE[2], 9), RECORDED[2], "70")).unwrap();
    paths[2] = path;
    let got = merge(&inputs(&paths), &options(), &|_, _| {}, Cancel::NEVER).unwrap_err();
    assert!(
        matches!(&got, Error::Refused(Refusal::AnotherScene(a, b, _)) if a == "IMG_2.dng" && b == "IMG_3.dng"),
        "{got}"
    );
    let one = merge(&inputs(&paths[..1]), &options(), &|_, _| {}, Cancel::NEVER).unwrap_err();
    assert_eq!(one, Error::Refused(Refusal::Count(1)));
}

/// A cancel at any stage ends the merge with [`Error::Cancelled`]; a cancelled
/// or refused write leaves no file, not even its staged one.
#[test]
fn cancelling_at_any_stage_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = bracket(dir.path());
    let inputs = inputs(&paths);
    let stages = Mutex::new(Vec::new());
    merge(
        &inputs,
        &options(),
        &|stage, _| stages.lock().unwrap().push(stage),
        Cancel::NEVER,
    )
    .unwrap();
    let mut stages = stages.into_inner().unwrap();
    stages.dedup();
    assert_eq!(stages.first(), Some(&Stage::Check));
    assert_eq!(stages.last(), Some(&Stage::Encode));
    assert!(stages.contains(&Stage::Measure(2)) && stages.contains(&Stage::Merge(0)));
    // The reference is decoded and prepared first, then each frame aligned.
    assert_eq!(&stages[1..3], &[Stage::Decode(1), Stage::Align(1)]);
    assert!(stages.contains(&Stage::Align(0)) && stages.contains(&Stage::Align(2)));
    // Deghosting without Auto Align also decodes the reference first, and
    // compares each other frame with it.
    let deghosting = MergeOptions {
        align: false,
        deghost: Deghost::High,
        ..options()
    };
    let deghost_stages = Mutex::new(Vec::new());
    merge(
        &inputs,
        &deghosting,
        &|stage, _| deghost_stages.lock().unwrap().push(stage),
        Cancel::NEVER,
    )
    .unwrap();
    let mut deghost_stages = deghost_stages.into_inner().unwrap();
    deghost_stages.dedup();
    assert_eq!(&deghost_stages[1..3], &[Stage::Decode(1), Stage::Decode(0)]);
    assert!(
        deghost_stages.contains(&Stage::Deghost(0))
            && deghost_stages.contains(&Stage::Deghost(2))
            && !deghost_stages.contains(&Stage::Deghost(1))
    );
    let runs = stages.into_iter().map(|at| (at, options())).chain(
        deghost_stages
            .into_iter()
            .map(|at| (at, deghosting.clone())),
    );
    for (at, options) in runs {
        let flag = AtomicBool::new(false);
        let got = merge(
            &inputs,
            &options,
            &|stage, _| {
                if stage == at {
                    flag.store(true, Ordering::Relaxed);
                }
            },
            Cancel::new(&flag),
        );
        assert_eq!(got.unwrap_err(), Error::Cancelled, "at {at:?}, {options:?}");
    }
    let files = || {
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = files();
    let merged = merge(&inputs, &options(), &|_, _| {}, Cancel::NEVER).unwrap();
    let set = AtomicBool::new(true);
    assert_eq!(
        write_dng(
            &merged,
            &Extras::default(),
            &Destination::beside(&paths[0]),
            Cancel::new(&set)
        ),
        Err(Error::Cancelled)
    );
    assert_eq!(files(), before);
    // A source changed since the merge read it: nothing is published.
    let mut bytes = std::fs::read(&paths[1]).unwrap();
    bytes.push(0);
    std::fs::write(&paths[1], bytes).unwrap();
    assert_eq!(
        write_dng(
            &merged,
            &Extras::default(),
            &Destination::beside(&paths[0]),
            Cancel::NEVER
        ),
        Err(Error::SourceChanged("IMG_1.dng".into()))
    );
    assert_eq!(files(), before);
}

/// A name that is taken is never replaced: Beside numbers the next, Exactly
/// fails.
#[test]
fn names_that_are_taken_are_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let paths = bracket(dir.path());
    let merged = merged(&paths);
    let taken = dir.path().join("IMG_2-HDR.dng");
    std::fs::write(&taken, b"mine").unwrap();
    let beside = Destination::beside(&paths[0]);
    let path = write_dng(&merged, &Extras::default(), &beside, Cancel::NEVER).unwrap();
    assert_eq!(path, dir.path().join("IMG_2-HDR-2.dng"));
    assert_eq!(std::fs::read(&taken).unwrap(), b"mine");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        merged.dng.with(&Extras::default()).unwrap().to_vec()
    );
    assert_eq!(
        write_dng(
            &merged,
            &Extras::default(),
            &Destination::Exactly(taken.clone()),
            Cancel::NEVER
        ),
        Err(Error::Exists(taken.clone()))
    );
    assert_eq!(std::fs::read(&taken).unwrap(), b"mine");
    let leftovers = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".partial")
        })
        .count();
    assert_eq!(leftovers, 0);
}
