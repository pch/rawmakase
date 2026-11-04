//! Routes native menu choices through the same editor operations as the UI.
use crate::app::{
    Editor, Module,
    shell::dialogs::{CatalogDialog, FolderAction},
    shell::panels::{PanelChange, WorkspacePanel},
};
use crate::platform::menu::Action;
use eframe::egui;

impl Editor {
    pub(in crate::app) fn menu_state(&self, action: Action, ctx: &egui::Context) -> (bool, bool) {
        let free = !self.activity.is_busy() && !self.workspace_modal() && !self.close_confirm;
        let text = ctx.text_edit_focused();
        let library = self.library.is_some();
        let selected = self.library.as_ref().and_then(|l| l.selected()).is_some();
        let workspace = free && library && !self.onboarding.visible;
        let enabled = match action {
            Action::Close | Action::Fullscreen => true,
            // Like the sidebar's button, while a background task runs too.
            Action::ReportIssue => !self.workspace_modal() && !self.close_confirm,
            Action::Cut | Action::Copy | Action::Paste => text && !self.activity.is_busy(),
            Action::Undo | Action::Redo if text => !self.activity.is_busy(),
            Action::SelectAll if text => !self.activity.is_busy(),
            Action::Undo => free && self.undo_log.can_undo(),
            Action::Redo => free && self.undo_log.can_redo(),
            Action::Preferences | Action::NewCatalog | Action::OpenCatalog | Action::Shortcuts => {
                free
            }
            Action::AddFolder => free && library,
            Action::Export => workspace && selected,
            Action::Develop => workspace && selected,
            Action::SelectAll => workspace,
            // As the thumbnail menu's: in the Library, where stacks show.
            Action::AutoStack => {
                workspace
                    && self.module == Module::Library
                    && self.library.as_ref().is_some_and(|l| l.shows_stacks())
            }
            Action::Library | Action::Sidebar | Action::Inspector | Action::Filmstrip => workspace,
        };
        let checked = match action {
            Action::Library => self.module == Module::Library && !self.onboarding.visible,
            Action::Develop => self.module == Module::Develop && !self.onboarding.visible,
            Action::Sidebar => self.panel_shown(WorkspacePanel::Left),
            Action::Inspector => self.panel_shown(WorkspacePanel::Right),
            Action::Filmstrip => self.panel_shown(WorkspacePanel::Filmstrip),
            Action::Fullscreen => ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)),
            _ => false,
        };
        (enabled, checked)
    }

    pub(in crate::app) fn menu_action(&mut self, action: Action, ctx: &egui::Context) {
        // Revalidate when dispatched: a queued click may precede a modal or job.
        if !self.menu_state(action, ctx).0 {
            return;
        }
        if ctx.text_edit_focused()
            && matches!(action, Action::Undo | Action::Redo | Action::SelectAll)
        {
            let key = if action == Action::SelectAll {
                egui::Key::A
            } else {
                egui::Key::Z
            };
            ctx.input_mut(|i| {
                i.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        shift: action == Action::Redo,
                        ..egui::Modifiers::COMMAND
                    },
                })
            });
            return;
        }
        match action {
            Action::Preferences => self.open_preferences(crate::app::preferences::Tab::General),
            Action::NewCatalog => self.catalog_dialog(CatalogDialog::Create, ctx),
            Action::OpenCatalog => self.catalog_dialog(CatalogDialog::Open, ctx),
            Action::AddFolder => self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), ctx),
            Action::Export => self.open_export_dialog(),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Cut => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut),
            Action::Copy => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
            Action::Paste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            Action::SelectAll => {
                if let Some(library) = &mut self.library {
                    if self.module == Module::Develop {
                        library.develop_select_all(self.document.catalog_photo);
                    } else {
                        library.select_all();
                    }
                }
            }
            Action::Library => {
                if self.flush() {
                    self.module = Module::Library;
                    if let Some(library) = &mut self.library {
                        library.show_grid();
                    }
                }
            }
            Action::AutoStack => {
                if let Some(library) = &mut self.library {
                    library.stack_action(crate::app::library::StackAction::AutoStack, Vec::new());
                }
            }
            Action::Develop => {
                if let Some(id) = self.library.as_ref().and_then(|l| l.selected()) {
                    self.develop_catalog_photo(id);
                }
            }
            Action::Sidebar | Action::Inspector | Action::Filmstrip => {
                let panel = match action {
                    Action::Sidebar => WorkspacePanel::Left,
                    Action::Inspector => WorkspacePanel::Right,
                    _ => WorkspacePanel::Filmstrip,
                };
                self.request_panels(PanelChange::Toggle(panel));
            }
            Action::Fullscreen => ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                !self.menu_state(action, ctx).1,
            )),
            Action::Shortcuts => self.view.shortcuts = true,
            Action::ReportIssue => self.open_report_issue(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::app) fn native_menu_actions(&mut self, ctx: &egui::Context) {
        if !self.native_menu {
            return;
        }
        let actions = crate::platform::menu::take_actions();
        for action in actions {
            self.menu_action(action, ctx);
        }
    }

    #[cfg(target_os = "macos")]
    pub(in crate::app) fn update_native_menu(&mut self, ctx: &egui::Context) {
        if self.native_menu {
            self.sync_undo();
            crate::platform::menu::update(|action| self.menu_state(action, ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::editor_with_catalog;

    #[test]
    fn menu_commands_recheck_busy_and_modal_state() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        assert!(editor.activity.begin_folder_change());
        for action in [
            Action::NewCatalog,
            Action::OpenCatalog,
            Action::AddFolder,
            Action::Export,
            Action::Undo,
            Action::Redo,
            Action::Cut,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
            Action::AutoStack,
            Action::Preferences,
            Action::Library,
            Action::Develop,
            Action::Sidebar,
            Action::Inspector,
            Action::Filmstrip,
            Action::Shortcuts,
        ] {
            assert!(!editor.menu_state(action, &ctx).0, "{action:?}");
            editor.menu_action(action, &ctx);
        }
        assert!(editor.menu_state(Action::Fullscreen, &ctx).0);
        // Report an Issue stays available while a task runs, as the sidebar's
        // button does, and then makes the dialog modal.
        assert!(editor.menu_state(Action::ReportIssue, &ctx).0);
        editor.report_issue_test_probe();
        editor.menu_action(Action::ReportIssue, &ctx);
        assert!(editor.report_issue.is_open());
        assert!(!editor.menu_state(Action::ReportIssue, &ctx).0);
        assert!(!editor.menu_state(Action::Shortcuts, &ctx).0);
        editor.close_report_issue();
        assert!(!editor.preferences.open);
        assert!(!editor.view.shortcuts);
        assert!(editor.activity.is_changing_folder());
        editor.activity.finish_dialog();
        editor.view.shortcuts = true;
        assert!(!editor.menu_state(Action::OpenCatalog, &ctx).0);
        assert!(
            editor.menu_state(Action::Close, &ctx).0,
            "close still uses the save guard"
        );
        Ok(())
    }

    #[test]
    fn menu_undo_and_redo_use_the_shared_history() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor.library.as_mut().unwrap().select(Some(ids[0]));
        editor
            .command_metadata(
                crate::app::library::photo_metadata::Edit::Rating(4),
                None,
                false,
            )
            .unwrap();
        editor.menu_action(Action::Undo, &ctx);
        assert_eq!(
            editor
                .library
                .as_ref()
                .unwrap()
                .photo(ids[0])
                .unwrap()
                .rating,
            0
        );
        editor.menu_action(Action::Redo, &ctx);
        assert_eq!(
            editor
                .library
                .as_ref()
                .unwrap()
                .photo(ids[0])
                .unwrap()
                .rating,
            4
        );
        Ok(())
    }

    #[test]
    fn menu_library_cannot_leave_a_draft_that_failed_to_save() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW"])?;
        editor.onboarding.visible = false;
        let library = editor.library.as_mut().unwrap();
        let copy = library.create_virtual_copy(ids[0])?.value;
        library
            .session
            .catalog_mut_for_tests()
            .remove_virtual_copy(copy)?;
        library.set_copy_name_draft(copy, "Unsaved name");
        editor.module = Module::Develop;
        let ctx = editor.context.clone();
        editor.menu_action(Action::Library, &ctx);
        assert!(editor.module == Module::Develop);
        assert!(!editor.flush());
        editor.library.as_mut().unwrap().discard_drafts();
        Ok(())
    }

    #[test]
    fn menu_auto_stack_opens_the_window_in_the_library_where_stacks_show() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        let ctx = editor.context.clone();
        editor.onboarding.visible = false;
        editor.module = Module::Develop;
        assert!(!editor.menu_state(Action::AutoStack, &ctx).0);
        editor.menu_action(Action::AutoStack, &ctx);
        assert!(!editor.auto_stack_open());
        editor.module = Module::Library;
        assert!(editor.menu_state(Action::AutoStack, &ctx).0);
        editor.menu_action(Action::AutoStack, &ctx);
        assert!(editor.auto_stack_open());
        // Open, the window is modal: the menu waits for it.
        assert!(!editor.menu_state(Action::AutoStack, &ctx).0);
        Ok(())
    }

    #[test]
    fn menu_select_all_in_develop_preserves_the_open_photo() -> anyhow::Result<()> {
        let (_dir, mut editor, ids) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        editor.onboarding.visible = false;
        editor.module = Module::Develop;
        editor.document.catalog_photo = Some(ids[1]);
        editor.library.as_mut().unwrap().select(Some(ids[1]));
        let ctx = editor.context.clone();
        editor.menu_action(Action::SelectAll, &ctx);
        let library = editor.library.as_ref().unwrap();
        assert_eq!(library.selected_photos().len(), 2);
        assert_eq!(library.selected(), Some(ids[1]));
        Ok(())
    }

    #[test]
    fn menu_close_requests_the_normal_window_close() -> anyhow::Result<()> {
        let (_dir, mut editor, _) = editor_with_catalog(&[])?;
        let ctx = editor.context.clone();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            editor.menu_action(Action::Close, ui.ctx())
        });
        output.textures_delta.clear();
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close))
        );
        Ok(())
    }
}
