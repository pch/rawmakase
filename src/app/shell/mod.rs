//! The window around the modules: the workspace, its chrome and menus, messages, undo and quitting.
pub(in crate::app) mod automation;
pub(in crate::app) mod chrome;
pub(in crate::app) mod commands;
pub(in crate::app) mod crash_log;
pub(in crate::app) mod dialogs;
pub(in crate::app) mod display;
pub(in crate::app) mod exit;
pub(in crate::app) mod messages;
#[cfg(any(target_os = "macos", test))]
pub(in crate::app) mod native_menu;
pub(in crate::app) mod panels;
pub(in crate::app) mod report_issue;
pub(in crate::app) mod shortcuts;
pub(in crate::app) mod undo;
pub(in crate::app) mod updates;
pub(in crate::app) mod workspace;
