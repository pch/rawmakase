//! The Node addon: the editor's operations as functions JavaScript calls, and
//! its frames as calls back into JavaScript.
//!
//! Quick operations run on the calling (JavaScript) thread: they change the
//! recipe and hand the render to the editor's own thread. Opening and exporting
//! take as long as a decode or a full-size render, so they run on Node's thread
//! pool and return promises.
use crate::editor::{self, Config, Editor, Update};
use napi::bindgen_prelude::{AsyncTask, Buffer, Function};
use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use napi::{Env, Status, Task};
use napi_derive::napi;
use std::path::PathBuf;

/// An editor error for JavaScript: its message, and as its `code` what the caller
/// may act on (`InvalidArg` for a value or file the editor refuses, `Cancelled`
/// for work that was superseded).
fn js_error(error: editor::Error) -> napi::Error {
    use rawmakase_core::ErrorKind;
    let status = match &error {
        editor::Error::Superseded => Status::Cancelled,
        editor::Error::Library(e) => match e.kind() {
            ErrorKind::Cancelled => Status::Cancelled,
            ErrorKind::Invalid | ErrorKind::Unsupported | ErrorKind::Missing => Status::InvalidArg,
            _ => Status::GenericFailure,
        },
        _ => Status::InvalidArg,
    };
    napi::Error::new(status, error.to_string())
}

/// Where the app keeps RAWmakase's data.
#[napi(object)]
pub struct Folders {
    pub data_dir: String,
    pub cache_dir: String,
    pub build_id: String,
}

/// A rendered frame, or a render's failure, as JavaScript receives it.
#[napi(object)]
pub struct Rendered {
    /// The number of the edit this shows.
    pub edit: u32,
    /// Whether this is a draft, with the frame to follow.
    pub draft: bool,
    pub width: u32,
    pub height: u32,
    /// The render on the Rust side, in milliseconds.
    pub render_ms: f64,
    /// RGBA, 8 bits a channel; absent when the render failed.
    pub pixels: Option<Buffer>,
    pub error: Option<String>,
}

#[napi]
pub struct Addon {
    editor: Editor,
}

#[napi]
impl Addon {
    /// An editor that calls `on_rendered` for every frame.
    #[napi(constructor)]
    pub fn new(folders: Folders, on_rendered: Function<'_, Rendered, ()>) -> napi::Result<Self> {
        let rendered = on_rendered
            .build_threadsafe_function::<Rendered>()
            .callee_handled::<false>()
            .build()?;
        let config = Config {
            data_dir: folders.data_dir.into(),
            cache_dir: folders.cache_dir.into(),
            build_id: folders.build_id,
        };
        let editor = Editor::new(config, move |update| {
            let message = match update {
                Update::Frame {
                    edit,
                    draft,
                    pixels,
                    render_ms,
                } => Rendered {
                    edit,
                    draft,
                    width: pixels.width,
                    height: pixels.height,
                    render_ms,
                    pixels: Some(pixels.rgba.into()),
                    error: None,
                },
                Update::Failed { edit, message } => Rendered {
                    edit,
                    draft: false,
                    width: 0,
                    height: 0,
                    render_ms: 0.,
                    pixels: None,
                    error: Some(message),
                },
            };
            rendered.call(message, ThreadsafeFunctionCallMode::NonBlocking);
        })
        .map_err(js_error)?;
        Ok(Self { editor })
    }

    /// Opens a photo; resolves with the editor's state as JSON.
    #[napi]
    pub fn open(&self, path: String, edit: u32) -> AsyncTask<Open> {
        AsyncTask::new(Open {
            editor: self.editor.clone(),
            path: path.into(),
            edit,
        })
    }

    #[napi]
    pub fn set_view(&self, max_edge: u32, edit: u32) {
        self.editor.set_view(max_edge, edit);
    }

    #[napi]
    pub fn set_parameter(&self, name: String, value: f64, edit: u32) -> napi::Result<String> {
        self.editor
            .set_parameter(&name, value as f32, edit)
            .map_err(js_error)
    }

    #[napi]
    pub fn reset_parameter(&self, name: String, edit: u32) -> napi::Result<String> {
        self.editor.reset_parameter(&name, edit).map_err(js_error)
    }

    #[napi]
    pub fn curve_insert(&self, x: f64, y: f64, edit: u32) -> napi::Result<String> {
        self.editor
            .curve_insert(x as f32, y as f32, edit)
            .map_err(js_error)
    }

    #[napi]
    pub fn curve_move(&self, index: u32, x: f64, y: f64, edit: u32) -> napi::Result<String> {
        self.editor
            .curve_move(index, x as f32, y as f32, edit)
            .map_err(js_error)
    }

    #[napi]
    pub fn curve_remove(&self, index: u32, edit: u32) -> napi::Result<String> {
        self.editor.curve_remove(index, edit).map_err(js_error)
    }

    #[napi]
    pub fn describe(&self) -> String {
        self.editor.describe()
    }

    /// Exports the photo; resolves with what the export has to say, if anything.
    #[napi]
    pub fn export(&self, path: String) -> AsyncTask<Export> {
        AsyncTask::new(Export {
            editor: self.editor.clone(),
            path: path.into(),
        })
    }
}

pub struct Open {
    editor: Editor,
    path: PathBuf,
    edit: u32,
}

impl Task for Open {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<String> {
        self.editor.open(&self.path, self.edit).map_err(js_error)
    }

    fn resolve(&mut self, _env: Env, state: String) -> napi::Result<String> {
        Ok(state)
    }
}

pub struct Export {
    editor: Editor,
    path: PathBuf,
}

impl Task for Export {
    type Output = Option<String>;
    type JsValue = Option<String>;

    fn compute(&mut self) -> napi::Result<Option<String>> {
        self.editor.export(&self.path).map_err(js_error)
    }

    fn resolve(&mut self, _env: Env, note: Option<String>) -> napi::Result<Option<String>> {
        Ok(note)
    }
}
