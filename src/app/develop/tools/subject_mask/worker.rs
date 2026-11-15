//! The thread that does the pixel work on either side of a mask request
//! ([`Worker::call`]): rendering the photo as the generator sees it into the
//! request's buffers ([`render_input`]), and copying the coverage out of them
//! and keeping it as an unsaved raster ([`keep`],
//! [`crate::app::extensions::mask_generator`]). Neither runs on the UI thread;
//! the models run in the generator's program.
//!
//! Quitting hands the thread to the shared shutdown deadline rather than
//! joining it.
use super::{Failure, Generated};
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::model::masks::BitmapSource;
use crate::model::recipe::Recipe;
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets;
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Sender},
};

/// The longest side of the photo rendered for the model. The network looks at
/// 1024 pixels; more only makes the stored raster larger.
const INPUT_EDGE: u32 = 1280;

/// Work of a request to run here, and the event to send in its place if it
/// panics or the thread is gone.
type Call = Box<dyn FnOnce() -> Event + Send>;

/// A call, its fallback, and where to send the event.
struct Message(Call, Call, Sender<Event>, egui::Context);

/// A thread that runs a tool's pixel work in order, started at its first
/// call. Select Subject's is this module's; AI Remove has one of its own.
pub(in crate::app) struct Worker {
    name: &'static str,
    messages: Option<Sender<Message>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Default for Worker {
    fn default() -> Self {
        Self::named("subject-select")
    }
}
impl Worker {
    /// A worker whose thread is called `name`.
    pub(in crate::app) fn named(name: &'static str) -> Self {
        Self {
            name,
            messages: None,
            thread: None,
        }
    }
    /// Runs `work` on the thread, after the jobs queued before it, and sends
    /// the event it returns. If it panics, or the thread can't take it,
    /// `fallback`'s event is sent instead, so whatever waits on it ends.
    pub(in crate::app) fn call(
        &mut self,
        work: impl FnOnce() -> Event + Send + 'static,
        fallback: impl FnOnce() -> Event + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        let (work, fallback): (Call, Call) = (Box::new(work), Box::new(fallback));
        // Refused, the fallback has been sent.
        let _ = self.send(Message(work, fallback, tx, ctx));
    }
    /// Sends `message` to the thread, starting it first when it isn't running.
    fn send(&mut self, message: Message) -> Result<(), String> {
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            self.messages = None;
            self.thread = None;
        }
        if self.messages.is_none() {
            let (messages, inbox) = mpsc::channel();
            match std::thread::Builder::new()
                .name(self.name.into())
                .spawn(move || run(inbox))
            {
                Ok(thread) => {
                    self.messages = Some(messages);
                    self.thread = Some(thread);
                }
                Err(e) => {
                    Self::refused(message);
                    return Err(format!("could not start the selection thread: {e}"));
                }
            }
        }
        let Some(messages) = &self.messages else {
            return Err("the selection thread stopped".into());
        };
        if let Err(mpsc::SendError(message)) = messages.send(message) {
            self.messages = None;
            Self::refused(message);
            return Err("the selection thread stopped".into());
        }
        Ok(())
    }
    /// A message the thread never got: a call's fallback is sent, so its
    /// request ends.
    fn refused(message: Message) {
        let Message(_, fallback, tx, ctx) = message;
        let _ = tx.send(fallback());
        ctx.request_repaint();
    }
    /// Closes the mailbox and returns the thread to wait for under the shutdown
    /// deadline.
    pub(in crate::app) fn stop(&mut self) -> Stopping {
        self.messages = None;
        Stopping::new(self.thread.take())
    }
}

fn run(inbox: mpsc::Receiver<Message>) {
    for Message(work, fallback, tx, ctx) in inbox {
        let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .unwrap_or_else(|_| fallback());
        let _ = tx.send(event);
        ctx.request_repaint();
    }
}

/// The photo as a mask generator sees it ([`crate::develop::masks::selection_input`]):
/// its width, height and 8-bit sRGB pixels, at most [`INPUT_EDGE`] on the long
/// side. Fails with [`Failure::Cancelled`] once `cancel` is raised.
pub(in crate::app) fn render_input(
    image: &CameraImage,
    recipe: &Recipe,
    cancel: &AtomicBool,
) -> Result<(u32, u32, Vec<u8>), Failure> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let input =
        crate::develop::masks::selection_input(image, recipe, INPUT_EDGE, Cancel::new(cancel))
            .map_err(|e| {
                if cancelled() {
                    Failure::Cancelled
                } else {
                    Failure::Failed(format!("{e:#}"))
                }
            })?;
    Ok((input.width, input.height, input.rgb8()))
}

/// Keeps a coverage of `width` by `height` as an unsaved raster from
/// `source`; all-zero coverage is `empty` instead. Exactly nothing selected is
/// the only emptiness decided here.
pub(in crate::app) fn keep(
    width: u32,
    height: u32,
    coverage: Vec<u8>,
    empty: Failure,
    source: BitmapSource,
) -> Result<Generated, Failure> {
    if coverage.iter().all(|v| *v == 0) {
        return Err(empty);
    }
    let id = mask_assets::register(Bitmap {
        width,
        height,
        channels: 1,
        depth: 1,
        data: coverage,
    })
    .map_err(|e| Failure::Failed(format!("{e:#}")))?;
    Ok(Generated {
        id,
        width,
        height,
        source,
    })
}
