//! Which names a file or folder may be given: the ones every system the
//! catalog may be used on can hold, so a catalog stays portable. One rule
//! for folders made and files and folders renamed. A photo's new name is a
//! [`Stem`] and then a [`FileName`], each made only here, by the rule, so a
//! name that wasn't checked can't reach a rename.
use std::fmt;
use std::path::Path;

/// Why a name can't be a file's or folder's on some system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameProblem {
    /// It is empty, or only spaces.
    Empty,
    /// `.` or `..`.
    Dots,
    /// It has `/` or `\` in it.
    Separator,
    /// It has a control character in it.
    Control,
    /// It has a character Windows refuses: `: * ? " < > |`.
    Windows,
    /// It ends in a dot or a space, which Windows drops.
    Trailing,
    /// It is a name Windows keeps for a device (CON, NUL, COM1…), whatever
    /// follows a dot.
    Device,
    /// It is over 255 bytes.
    Long,
}

/// Why `name` can't be a file's or folder's name; `None` when it can.
pub(super) fn problem(name: &str) -> Option<NameProblem> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    if name.trim().is_empty() {
        Some(NameProblem::Empty)
    } else if name == "." || name == ".." {
        Some(NameProblem::Dots)
    } else if name.contains(['/', '\\']) {
        Some(NameProblem::Separator)
    } else if name.chars().any(char::is_control) {
        Some(NameProblem::Control)
    } else if name.contains([':', '*', '?', '"', '<', '>', '|']) {
        Some(NameProblem::Windows)
    } else if name.ends_with(['.', ' ']) {
        Some(NameProblem::Trailing)
    } else if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        Some(NameProblem::Device)
    } else if name.len() > 255 {
        Some(NameProblem::Long)
    } else {
        None
    }
}

/// Said of a photo's file.
impl fmt::Display for NameProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "Give the photo a name.",
            Self::Dots => "That name is taken by the system.",
            Self::Separator => "A file's name can't have / or \\ in it.",
            Self::Control => "A file's name can't have control characters in it.",
            Self::Windows => {
                "A file's name can't have : * ? \" < > or | in it, which Windows refuses."
            }
            Self::Trailing => "A file's name can't end in a dot or a space, which Windows refuses.",
            Self::Device => "Windows keeps that name for a device.",
            Self::Long => "That name is too long.",
        })
    }
}

/// Why `stem` can't be a photo's new name before its extension; `None`
/// when it can (with its extension it may still be too long).
pub fn photo_name_problem(stem: &str) -> Option<NameProblem> {
    Stem::parse(stem).err()
}

/// A photo's new name before its extension, as the rule allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stem(String);

impl Stem {
    pub(super) fn parse(text: &str) -> Result<Self, NameProblem> {
        match problem(text) {
            Some(p) => Err(p),
            None => Ok(Self(text.to_string())),
        }
    }
}

/// A photo's whole new name: a [`Stem`] and the extension it had, as the
/// rule allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileName(String);

impl FileName {
    /// The name of the file now named `current` given `stem`: its
    /// extension is kept.
    pub(super) fn with_stem(current: &str, stem: &Stem) -> Result<Self, NameProblem> {
        let name = match Path::new(current).extension().and_then(|e| e.to_str()) {
            Some(extension) => format!("{}.{extension}", stem.0),
            None => stem.0.clone(),
        };
        match problem(&name) {
            Some(p) => Err(p),
            None => Ok(Self(name)),
        }
    }
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}
