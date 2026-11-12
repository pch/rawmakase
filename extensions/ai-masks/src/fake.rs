//! A stand-in for the selection models, for tests: it answers from the photo's
//! pixels and the prompt alone, so the whole of a selection runs, through the
//! extension and the app, with no model or runtime present.
//!
//! - Subject: the left half of the photo where it isn't black, so an
//!   all-black photo has no subject;
//! - Sky: the top half, where it isn't black;
//! - a prompt: the box, then a disc of a fifth of the photo around each click,
//!   added or taken away.
//!
//! Selected pixels have the value of the first pixel's red (at least 1), so
//! photos of different levels give different coverage. Each call is counted,
//! and the folder it named is kept, in [`Calls`].
use crate::engine::Engine;
use rawmakase_cancel::Cancel;
use rawmakase_inference::InferenceError;
use rawmakase_selection::{Coverage, Prompt, RgbImage};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the fake was asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calls {
    /// Photos analysed for their subject; a second request on the same photo
    /// isn't analysed again.
    pub analyses: usize,
    /// Prompts answered.
    pub segments: usize,
    /// Skies found.
    pub skies: usize,
    /// The model folders the calls named, last first.
    pub folders: Vec<PathBuf>,
    /// Times the models were let go of.
    pub unloads: usize,
}

/// The fake engine. Clones share their [`Calls`].
#[derive(Clone, Default)]
pub struct Fake {
    calls: Arc<Mutex<Calls>>,
    analysed: Option<String>,
    /// What every call fails with instead, when set.
    pub fail: Arc<Mutex<Option<InferenceError>>>,
}

impl Fake {
    /// What it was asked so far.
    pub fn calls(&self) -> Calls {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn call(&self, models: &Path, count: impl FnOnce(&mut Calls)) -> Result<(), InferenceError> {
        if let Some(error) = self.fail.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Err(error);
        }
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.folders.insert(0, models.to_path_buf());
        count(&mut calls);
        Ok(())
    }
}

impl Engine for Fake {
    fn subject(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        if cancel.is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        let fresh = self.analysed.as_deref() != Some(key);
        self.call(models, |c| c.analyses += usize::from(fresh))?;
        self.analysed = Some(key.to_string());
        Ok(half(photo, |x, _, w, _| x < w.div_ceil(2)))
    }

    fn segment(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        if cancel.is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        let fresh = self.analysed.as_deref() != Some(key);
        self.call(models, |c| {
            c.analyses += usize::from(fresh);
            c.segments += 1;
        })?;
        self.analysed = Some(key.to_string());
        Ok(prompted(photo, prompt))
    }

    fn sky(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        if cancel.is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        self.call(models, |c| c.skies += 1)?;
        Ok(half(photo, |_, y, _, h| y < h.div_ceil(2)))
    }

    fn unload(&mut self) {
        self.analysed = None;
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).unloads += 1;
    }
}

/// The value of selected pixels: the first pixel's red, at least 1.
pub fn ink(photo: &RgbImage) -> u8 {
    photo.data.first().copied().unwrap_or(255).max(1)
}

/// The pixels `inside` keeps, where the photo isn't black.
fn half(photo: &RgbImage, inside: impl Fn(usize, usize, usize, usize) -> bool) -> Coverage {
    let (w, h) = (photo.width, photo.height);
    let ink = ink(photo);
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let black = photo.data[i * 3..][..3] == [0, 0, 0];
            if inside(x, y, w, h) && !black { ink } else { 0 }
        })
        .collect();
    Coverage {
        width: w,
        height: h,
        data,
    }
}

/// The box, then the clicks' discs.
pub fn prompted(photo: &RgbImage, prompt: &Prompt) -> Coverage {
    let (w, h) = (photo.width, photo.height);
    let ink = ink(photo);
    let centre = |i: usize, n: usize| (i as f32 + 0.5) / n as f32;
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (centre(x, w), centre(y, h));
            if let Some([x0, y0, x1, y1]) = prompt.bounds
                && (x0..=x1).contains(&u)
                && (y0..=y1).contains(&v)
            {
                data[y * w + x] = ink;
            }
            for p in &prompt.points {
                let (du, dv) = (u - p.x, v - p.y);
                if du * du + dv * dv <= 0.2 * 0.2 {
                    data[y * w + x] = if p.positive { ink } else { 0 };
                }
            }
        }
    }
    Coverage {
        width: w,
        height: h,
        data,
    }
}

/// A fake remote worker, the real server of `rawmakase-remote-worker` over
/// the fake's answers, on a loopback port of this process: its endpoint,
/// and what it was asked.
pub fn remote_worker() -> (String, Arc<Mutex<Calls>>) {
    use rawmakase_remote::server::{Engine, EngineError, Info, Outcome, Server};
    use std::collections::HashMap;

    struct Worker {
        photos: HashMap<String, RgbImage>,
        calls: Arc<Mutex<Calls>>,
    }
    impl Worker {
        fn count(&self, count: impl FnOnce(&mut Calls)) {
            count(&mut self.calls.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }
    impl Engine for Worker {
        fn analyze(&mut self, photo: &RgbImage, _: Cancel<'_>) -> Result<Outcome, EngineError> {
            let key = rawmakase_remote::protocol::analysis_key(photo);
            self.count(|c| c.analyses += 1);
            self.photos.insert(key.clone(), photo.clone());
            Ok(Outcome {
                key,
                subject: half(photo, |x, _, w, _| x < w.div_ceil(2)),
            })
        }
        fn segment(
            &mut self,
            key: &str,
            prompt: &Prompt,
            _: Cancel<'_>,
        ) -> Result<Coverage, EngineError> {
            let photo = self.photos.get(key).ok_or(EngineError::UnknownKey)?;
            let coverage = prompted(photo, prompt);
            self.count(|c| c.segments += 1);
            Ok(coverage)
        }
        fn sky(&mut self, photo: &RgbImage, _: Cancel<'_>) -> Result<Coverage, EngineError> {
            self.count(|c| c.skies += 1);
            Ok(half(photo, |_, y, _, h| y < h.div_ceil(2)))
        }
        fn describe(&self) -> Info {
            Info {
                model: "fake@0".into(),
                panoptic: "fake".into(),
                sky: "fake-sky".into(),
                provider: "CPU".into(),
            }
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let endpoint = listener.local_addr().expect("its address").to_string();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let worker = Worker {
        photos: HashMap::new(),
        calls: calls.clone(),
    };
    std::thread::spawn(move || Arc::new(Server::new(worker)).serve(listener));
    (endpoint, calls)
}
