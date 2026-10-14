//! macOS: the app's menus in the system menu bar (instead of an in-window egui menu bar).
//!
//! Only ⌘-shortcuts are registered as native key equivalents; those are then handled by the
//! menu alone. Bare-key shortcuts (`D`, `G`, …) stay with the canvas and are shown in the item
//! titles, so a menu never swallows keystrokes meant for the search field.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{
    AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};

use crate::app::Action;
use crate::grouping::GroupingMode;

/// App state the menu reflects (checkmarks, enabled items).
#[derive(Clone, Copy, PartialEq)]
pub struct MenuState {
    pub has_scene: bool,
    pub has_file: bool,
    /// The current file is in a git work tree.
    pub has_git: bool,
    pub in_diff: bool,
    pub has_descriptions: bool,
    pub fields: bool,
    pub descriptions: bool,
    pub panels_hidden: bool,
    pub mode: Option<GroupingMode>,
}

#[derive(Clone, Copy)]
enum Check {
    Fields,
    Descriptions,
    PanelsHidden,
    Mode(GroupingMode),
}

#[derive(Clone, Copy)]
enum Needs {
    Always,
    Scene,
    File,
    Git,
    Diff,
    Descriptions,
}

pub struct NativeMenu {
    _menu: Menu,
    actions: HashMap<MenuId, Action>,
    pending: Arc<Mutex<Vec<MenuId>>>,
    items: Vec<(MenuItem, Needs)>,
    checks: Vec<(CheckMenuItem, Check, Needs)>,
    last: Option<MenuState>,
}

impl NativeMenu {
    pub fn install(ctx: &egui::Context) -> Option<Self> {
        let mut m = Self {
            _menu: Menu::new(),
            actions: HashMap::new(),
            pending: Arc::default(),
            items: Vec::new(),
            checks: Vec::new(),
            last: None,
        };
        let cmd = |code| Some(Accelerator::new(Modifiers::META, code));

        let app = Submenu::with_items(
            "ER Nuance",
            true,
            &[
                &PredefinedMenuItem::about(
                    None,
                    Some(AboutMetadata {
                        name: Some("ER Nuance".into()),
                        version: Some(env!("CARGO_PKG_VERSION").into()),
                        ..Default::default()
                    }),
                ),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ],
        )
        .ok()?;

        let file = Submenu::new("File", true);
        file.append(&m.item("Open…", cmd(Code::KeyO), Action::Open, Needs::Always))
            .ok()?;
        file.append(&m.item("Paste Diagram…", None, Action::Paste, Needs::Always))
            .ok()?;
        file.append(&m.item("Reload", cmd(Code::KeyR), Action::Reload, Needs::File))
            .ok()?;
        file.append(&m.item("Load Example", None, Action::LoadExample, Needs::Always))
            .ok()?;
        file.append(&PredefinedMenuItem::separator()).ok()?;
        file.append(&PredefinedMenuItem::close_window(None)).ok()?;

        let view = Submenu::new("View", true);
        view.append(&m.item(
            "Command Palette…",
            cmd(Code::KeyK),
            Action::OpenPalette,
            Needs::Always,
        ))
        .ok()?;
        view.append(&m.item(
            "Search Entities…",
            cmd(Code::KeyF),
            Action::FocusSearch,
            Needs::Scene,
        ))
        .ok()?;
        view.append(&PredefinedMenuItem::separator()).ok()?;
        view.append(&m.check(
            "Show Fields  (⇧F)",
            Check::Fields,
            Action::ToggleFieldsVisible,
            Needs::Scene,
        ))
        .ok()?;
        view.append(&m.check(
            "Show Descriptions  (D)",
            Check::Descriptions,
            Action::ToggleDescriptions,
            Needs::Descriptions,
        ))
        .ok()?;
        view.append(&m.check(
            "Hide Panels  (P)",
            Check::PanelsHidden,
            Action::TogglePanels,
            Needs::Scene,
        ))
        .ok()?;
        view.append(&PredefinedMenuItem::separator()).ok()?;
        for (title, action) in [
            ("Fit All  (0)", Action::Fit),
            ("Zoom In  (+)", Action::ZoomBy(1.25)),
            ("Zoom Out  (−)", Action::ZoomBy(0.8)),
        ] {
            view.append(&m.item(title, None, action, Needs::Scene))
                .ok()?;
        }
        view.append(&PredefinedMenuItem::separator()).ok()?;
        for (title, action) in [
            ("Collapse All Groups  (G)", Action::AllGroups(false)),
            ("Expand All Groups  (⇧G)", Action::AllGroups(true)),
            ("Show Fields of All Entities", Action::AllFields(true)),
            ("Hide Fields of All Entities", Action::AllFields(false)),
            ("Re-layout  (L)", Action::Relayout),
        ] {
            view.append(&m.item(title, None, action, Needs::Scene))
                .ok()?;
        }
        view.append(&PredefinedMenuItem::separator()).ok()?;
        view.append(&PredefinedMenuItem::fullscreen(None)).ok()?;

        let compare = Submenu::new("Compare", true);
        compare
            .append(&m.item("Compare with HEAD", None, Action::CompareHead, Needs::Git))
            .ok()?;
        compare
            .append(&m.item("Compare with Commit…", None, Action::PickCommit, Needs::Git))
            .ok()?;
        compare
            .append(&m.item("Compare with File…", None, Action::CompareFile, Needs::File))
            .ok()?;
        compare.append(&PredefinedMenuItem::separator()).ok()?;
        compare
            .append(&m.item(
                "Next Change  (N)",
                None,
                Action::NextChange(true),
                Needs::Diff,
            ))
            .ok()?;
        compare
            .append(&m.item(
                "Previous Change  (⇧N)",
                None,
                Action::NextChange(false),
                Needs::Diff,
            ))
            .ok()?;
        compare
            .append(&m.item("Exit Diff", None, Action::ExitDiff, Needs::Diff))
            .ok()?;

        let grouping = Submenu::new("Grouping", true);
        for mode in GroupingMode::ALL {
            grouping
                .append(&m.check(
                    mode.label(),
                    Check::Mode(mode),
                    Action::Mode(mode),
                    Needs::Scene,
                ))
                .ok()?;
        }

        let window = Submenu::with_items(
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ],
        )
        .ok()?;

        let help = Submenu::new("Help", true);
        help.append(&m.item(
            "Keyboard Shortcuts  (?)",
            None,
            Action::ShowHelp,
            Needs::Always,
        ))
        .ok()?;
        help.append(&m.item("GitHub Repository", None, Action::OpenGithub, Needs::Always))
            .ok()?;

        m._menu
            .append_items(&[&app, &file, &view, &compare, &grouping, &window, &help])
            .ok()?;
        m._menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        let pending = Arc::clone(&m.pending);
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            pending.lock().unwrap().push(e.id);
            ctx.request_repaint();
        }));
        Some(m)
    }

    fn item(
        &mut self,
        title: &str,
        accel: Option<Accelerator>,
        action: Action,
        needs: Needs,
    ) -> MenuItem {
        let item = MenuItem::new(title, true, accel);
        self.actions.insert(item.id().clone(), action);
        self.items.push((item.clone(), needs));
        item
    }

    fn check(&mut self, title: &str, check: Check, action: Action, needs: Needs) -> CheckMenuItem {
        let item = CheckMenuItem::new(title, true, false, None);
        self.actions.insert(item.id().clone(), action);
        self.checks.push((item.clone(), check, needs));
        item
    }

    /// Updates checkmarks and enabled states (only when something changed).
    pub fn sync(&mut self, st: MenuState) {
        if self.last == Some(st) {
            return;
        }
        self.last = Some(st);
        let ok = |n: Needs| match n {
            Needs::Always => true,
            Needs::Scene => st.has_scene,
            Needs::File => st.has_file,
            Needs::Git => st.has_git,
            Needs::Diff => st.in_diff,
            Needs::Descriptions => st.has_descriptions,
        };
        for (item, needs) in &self.items {
            item.set_enabled(ok(*needs));
        }
        for (item, check, needs) in &self.checks {
            item.set_enabled(ok(*needs));
            item.set_checked(match check {
                Check::Fields => st.fields,
                Check::Descriptions => st.descriptions,
                Check::PanelsHidden => st.panels_hidden,
                Check::Mode(m) => st.mode == Some(*m),
            });
        }
    }

    /// Actions chosen in the menu since the last call.
    pub fn take_actions(&self) -> Vec<Action> {
        let ids = std::mem::take(&mut *self.pending.lock().unwrap());
        ids.iter()
            .filter_map(|id| self.actions.get(id).copied())
            .collect()
    }

    /// Forces checkmarks to be rewritten on the next `sync` (after a click toggled one natively).
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}
