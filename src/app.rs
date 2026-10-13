use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{
    Align, Color32, Context, Event, Key, Layout, Modifiers, PointerButton, RichText, Sense,
    Vec2, vec2,
};

use crate::canvas::{Camera, Canvas, Highlight, Palette, distance_to_segment};
use crate::cli::Launch;
use crate::diff::{self, DiffMarks, Status};
use crate::grouping::GroupingMode;
use crate::mermaid;
use crate::palette::{CommandPalette, Item};
use crate::model::Diagram;
use crate::scene::{Font, Measure, NodeRef, SavedLayout, Scene, Which};
use crate::vcs::{self, Rev, Side};

const EXAMPLE: &str = include_str!("../examples/commerce.mmd");
const SEARCH_ID: &str = "entity-search";

struct EguiMeasure<'a>(&'a Context);

impl Measure for EguiMeasure<'_> {
    fn width(&self, text: &str, font: Font) -> f32 {
        let id = match font {
            f if f.monospace() => egui::FontId::monospace(f.size()),
            f => egui::FontId::proportional(f.size()),
        };
        self.0.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), id, Color32::WHITE).size().x)
    }

    fn rich_height(&self, text: &str, wrap: f32) -> f32 {
        let job = crate::richtext::job(text, Font::Small.size(), Color32::WHITE, Color32::WHITE, wrap);
        self.0.fonts_mut(|f| f.layout_job(job).size().y)
    }

    fn wrapped_height(&self, text: &str, font: Font, wrap: f32) -> f32 {
        let id = if font.monospace() { egui::FontId::monospace(font.size()) } else { egui::FontId::proportional(font.size()) };
        self.0.fonts_mut(|f| f.layout(text.to_owned(), id, Color32::WHITE, wrap).size().y)
    }
}

/// Descriptions appear above this zoom and disappear below the lower one (hysteresis, so a
/// zoom level right at the threshold does not make boxes flicker).
const DESC_ZOOM_ON: f32 = 0.55;
const DESC_ZOOM_OFF: f32 = 0.45;

#[derive(Clone, Copy)]
enum Drag {
    Pan,
    Node(NodeRef),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Select(Option<NodeRef>),
    /// Expand/collapse a group, or show/hide an entity's fields.
    Toggle(NodeRef),
    /// Group: expand and select a member. Entity: toggle fields.
    Dive(NodeRef),
    /// Entity in a group → select the group; otherwise deselect.
    Up,
    CollapseParent(NodeRef),
    ToggleFields(NodeRef),
    AllFields(bool),
    AllGroups(bool),
    /// Expand the entity's group if needed, select it and bring it into view.
    Reveal(usize),
    Center(NodeRef),
    Navigate(Vec2),
    Cycle(bool),
    Fit,
    ZoomBy(f32),
    Relayout,
    Mode(GroupingMode),
    Open,
    Reload,
    LoadExample,
    ToggleDescriptions,
    /// The "show fields" master switch.
    ToggleFieldsVisible,
    FocusSearch,
    ShowHelp,
    /// Open the command palette, or close it if open.
    OpenPalette,
    /// Hide / show both side panels.
    TogglePanels,
    /// Highlight / unhighlight the tag at this index of the diagram's tags.
    ToggleTag(usize),
    /// Expand exactly the groups holding entities with this tag; highlight it.
    FocusTag(usize),
    /// Diff mode: select the next / previous change.
    NextChange(bool),
    /// Compare the current file's working tree with HEAD.
    CompareHead,
    /// List the commits that touched the current file to compare with.
    PickCommit,
    /// Compare with `history[i]`.
    CompareCommit(usize),
    /// Compare with another file picked in a dialog.
    CompareFile,
    ExitDiff,
}

#[derive(Clone)]
enum Source {
    File(PathBuf),
    Example,
    Diff { old: Side, new: Side },
}

impl Source {
    /// Files that may change while shown, polled for live reload.
    fn live_paths(&self) -> Vec<PathBuf> {
        match self {
            Self::File(p) => vec![p.clone()],
            Self::Example => Vec::new(),
            Self::Diff { old, new } => old.live_path().into_iter().chain(new.live_path()).collect(),
        }
    }

    /// The file this view is about (the new side, in diff mode).
    fn path(&self) -> Option<PathBuf> {
        match self {
            Self::File(p) => Some(p.clone()),
            Self::Example => None,
            Self::Diff { new, .. } => Some(new.layout_path()),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PaletteMode {
    Commands,
    /// Choosing a commit from `history`.
    History,
}

pub struct ErApp {
    scene: Option<Scene>,
    source: Option<Source>,
    pending: Option<Source>,
    /// Live files and their modification times when last read.
    watched: Vec<(PathBuf, Option<SystemTime>)>,
    last_poll: f64,
    cam: Camera,
    cam_goal: Option<Camera>,
    canvas: egui::Rect,
    fit_pending: bool,
    selected: Option<NodeRef>,
    search: String,
    focus_search: bool,
    drag: Option<Drag>,
    context_node: Option<NodeRef>,
    hovered_edge: Option<usize>,
    show_help: bool,
    palette: Option<CommandPalette>,
    palette_mode: PaletteMode,
    /// Commits that touched the current file, for "Compare with commit…".
    history: Vec<vcs::Commit>,
    /// The current file's place in a git work tree, if any.
    git: Option<vcs::GitFile>,
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::native_menu::NativeMenu>,
    /// The user's "show descriptions" toggle; whether they are drawn also depends on zoom.
    desc_toggle: bool,
    panels_hidden: bool,
    /// Highlighted tags, by name (so they survive reloads).
    tag_highlight: std::collections::BTreeSet<String>,
    dirty_at: Option<f64>,
    status: String,
}

impl ErApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        install_fonts(&cc.egui_ctx);
        Self {
            scene: None,
            source: None,
            pending: match launch {
                Launch::View(path) => path.map(Source::File),
                Launch::Diff { old, new } => Some(Source::Diff { old, new }),
            },
            watched: Vec::new(),
            last_poll: 0.0,
            cam: Camera::default(),
            cam_goal: None,
            canvas: egui::Rect::NOTHING,
            fit_pending: false,
            selected: None,
            search: String::new(),
            focus_search: false,
            drag: None,
            context_node: None,
            hovered_edge: None,
            show_help: false,
            palette: None,
            palette_mode: PaletteMode::Commands,
            history: Vec::new(),
            git: None,
            // Not in tests: they render headlessly and must not touch the process' menu bar.
            #[cfg(target_os = "macos")]
            native_menu: if cfg!(test) { None } else { crate::native_menu::NativeMenu::install(&cc.egui_ctx) },
            desc_toggle: false,
            panels_hidden: false,
            tag_highlight: Default::default(),
            dirty_at: None,
            status: String::new(),
        }
    }

    // ---- loading & saving --------------------------------------------------------------------

    fn load(&mut self, ctx: &Context, source: Source) {
        let (diagram, marks, title) = match read_source(&source) {
            Ok(r) => r,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let saved = source.path().and_then(|p| read_sidecar(&p));
        let mode = saved.as_ref().and_then(|s| s.mode).unwrap_or_else(|| GroupingMode::default_for(&diagram));
        self.watched = source.live_paths().into_iter().map(|p| (p.clone(), modified(&p))).collect();
        self.git = match &source {
            Source::Diff { new: Side::Git { file, .. }, .. } => Some(file.clone()),
            s => s.path().and_then(|p| vcs::locate(&p)),
        };
        self.source = Some(source);
        self.set_scene(ctx, diagram, marks, mode, saved.as_ref());
        self.desc_toggle = saved.as_ref().is_some_and(|s| s.descriptions);
        self.panels_hidden = saved.as_ref().is_some_and(|s| s.panels_hidden);
        self.tag_highlight = saved.as_ref().map(|s| s.tags.iter().cloned().collect()).unwrap_or_default();
        match saved.and_then(|s| s.camera) {
            Some([x, y, z]) => self.cam = Camera { offset: vec2(x, y), zoom: z },
            None => self.fit_pending = true,
        }
        self.cam_goal = None;
        self.selected = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("ER Navigator — {title}")));
    }

    fn set_scene(&mut self, ctx: &Context, diagram: Diagram, marks: Option<DiffMarks>, mode: GroupingMode, saved: Option<&SavedLayout>) {
        let n = diagram.warnings.len();
        let scene = Scene::with_diff(diagram, marks, mode, saved, &EguiMeasure(ctx));
        let warnings = if n > 0 { format!(" · {n} warnings") } else { String::new() };
        self.status = match (&scene.diff, &self.diff_labels()) {
            (Some(d), Some((old, new))) => {
                let summary = |what: &str, c: diff::Counts| if c.is_empty() { None } else { Some(format!("{what} {}", c.badge())) };
                let parts: Vec<String> = [summary("entities", d.entity_counts()), summary("relationships", d.relationship_counts())]
                    .into_iter()
                    .flatten()
                    .collect();
                let changes = if d.is_identical() { "no differences".to_owned() } else { parts.join(" · ") };
                format!("{old} → {new} · {changes}{warnings}")
            }
            _ => format!(
                "{} entities · {} relationships · {} groups{warnings}",
                scene.diagram.entities.len(),
                scene.diagram.relationships.len(),
                scene.grouping.groups.len(),
            ),
        };
        self.scene = Some(scene);
    }

    /// Labels of the two sides of the diff shown.
    fn diff_labels(&self) -> Option<(String, String)> {
        match self.source.as_ref()? {
            Source::Diff { old, new } => Some((old.label(), new.label())),
            _ => None,
        }
    }

    /// Re-reads the file, keeping positions and open/closed state by name.
    fn reload(&mut self, ctx: &Context) {
        let (Some(source), Some(scene)) = (&self.source, &self.scene) else { return };
        if matches!(source, Source::Example) {
            return;
        }
        let Ok((diagram, marks, _)) = read_source(source) else { return };
        let mut saved = scene.to_saved(self.camera_array());
        saved.descriptions = self.desc_toggle;
        saved.panels_hidden = self.panels_hidden;
        saved.tags = self.tag_highlight.iter().cloned().collect();
        let mode = scene.mode;
        self.watched = source.live_paths().into_iter().map(|p| (p.clone(), modified(&p))).collect();
        let selected_name = self.selected.and_then(|n| match n {
            NodeRef::Entity(e) => Some(scene.diagram.entities[e].id.clone()),
            NodeRef::Group(_) => None,
        });
        self.set_scene(ctx, diagram, marks, mode, Some(&saved));
        self.selected = selected_name
            .and_then(|id| self.scene.as_ref()?.diagram.entity_index(&id))
            .map(NodeRef::Entity);
    }

    fn poll_file(&mut self, ctx: &Context, now: f64) {
        if now - self.last_poll < 1.0 {
            return;
        }
        self.last_poll = now;
        if self.watched.iter().any(|(p, t)| modified(p) != *t) {
            self.reload(ctx);
        }
    }

    fn camera_array(&self) -> [f32; 3] {
        [self.cam.offset.x, self.cam.offset.y, self.cam.zoom]
    }

    fn mark_dirty(&mut self, now: f64) {
        self.dirty_at = Some(now);
    }

    fn save_if_due(&mut self, now: f64) {
        let Some(t) = self.dirty_at else { return };
        if now - t < 1.0 {
            return;
        }
        self.dirty_at = None;
        if let (Some(Source::File(p)), Some(scene)) = (&self.source, &self.scene) {
            let mut saved = scene.to_saved(self.camera_array());
            saved.descriptions = self.desc_toggle;
            saved.panels_hidden = self.panels_hidden;
            saved.tags = self.tag_highlight.iter().cloned().collect();
            if let Ok(json) = serde_json::to_string_pretty(&saved)
                && let Err(e) = std::fs::write(sidecar_path(p), json)
            {
                self.status = format!("Cannot save layout: {e}");
            }
        }
    }

    // ---- camera ------------------------------------------------------------------------------

    fn animate_camera(&mut self, dt: f32) -> bool {
        let Some(goal) = self.cam_goal else { return false };
        let t = 1.0 - (-dt * 10.0).exp();
        self.cam = self.cam.lerp(&goal, t);
        if (self.cam.offset - goal.offset).length() < 0.5 && (self.cam.zoom - goal.zoom).abs() < 0.001 {
            self.cam = goal;
            self.cam_goal = None;
        }
        true
    }

    fn fit(&mut self, animate: bool) {
        let Some(scene) = &self.scene else { return };
        let b = scene.bounds(Which::Target);
        if !b.is_positive() || !self.canvas.is_positive() {
            return;
        }
        let goal = Camera::fitting(self.canvas, b, 1.5);
        if animate { self.cam_goal = Some(goal) } else { self.cam = goal }
    }

    /// Pans (and zooms in, if very far out) so that `n` is comfortably visible.
    fn bring_into_view(&mut self, n: NodeRef, force_center: bool) {
        let Some(scene) = &self.scene else { return };
        let r = scene.node_rect(n, Which::Target);
        let cam = self.cam_goal.unwrap_or(self.cam);
        let on_screen = cam.rect_to_screen(self.canvas, r);
        let readable = cam.zoom >= 0.6;
        if !force_center && readable && self.canvas.shrink(30.0).contains_rect(on_screen) {
            return;
        }
        let mut zoom = cam.zoom.max(0.8);
        // Make sure the whole node fits.
        zoom = zoom.min(self.canvas.width() * 0.9 / r.width()).min(self.canvas.height() * 0.9 / r.height());
        self.cam_goal = Some(Camera::centered_on(self.canvas, r.center(), zoom.max(0.05)));
    }

    // ---- actions -----------------------------------------------------------------------------

    fn apply(&mut self, ctx: &Context, action: Action, now: f64) {
        match action {
            Action::Open => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Mermaid ER", &["mmd", "mermaid", "md", "markdown", "txt"])
                    .pick_file()
                {
                    self.load(ctx, Source::File(p));
                }
                return;
            }
            Action::LoadExample => return self.load(ctx, Source::Example),
            Action::OpenPalette => {
                self.palette = match self.palette {
                    Some(_) => None,
                    None => Some(CommandPalette::new()),
                };
                self.palette_mode = PaletteMode::Commands;
                return;
            }
            Action::CompareHead => {
                let Some(file) = self.current_git_file() else { return };
                self.load(ctx, Source::Diff { old: Side::git(&file, Rev::Commit("HEAD".into())), new: Side::git(&file, Rev::WorkTree) });
                return;
            }
            Action::PickCommit => {
                let Some(file) = self.current_git_file() else { return };
                match vcs::history(&file, 500) {
                    Ok(h) if h.is_empty() => self.status = "No commits touch this file yet".to_owned(),
                    Ok(h) => {
                        self.history = h;
                        self.palette = Some(CommandPalette::new());
                        self.palette_mode = PaletteMode::History;
                    }
                    Err(e) => self.status = e,
                }
                return;
            }
            Action::CompareCommit(i) => {
                let (Some(file), Some(c)) = (self.current_git_file(), self.history.get(i)) else { return };
                let old = Side::git(&file, Rev::Commit(c.id.clone()));
                self.load(ctx, Source::Diff { old, new: Side::git(&file, Rev::WorkTree) });
                return;
            }
            Action::CompareFile => {
                let Some(current) = self.source.as_ref().and_then(Source::path) else { return };
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Mermaid ER", &["mmd", "mermaid", "md", "markdown", "txt"])
                    .set_title("Compare with…")
                    .pick_file()
                {
                    self.load(ctx, Source::Diff { old: Side::File(p), new: Side::File(current) });
                }
                return;
            }
            Action::ExitDiff => {
                if let Some(p) = self.source.as_ref().and_then(Source::path).filter(|p| p.is_file()) {
                    self.load(ctx, Source::File(p));
                }
                return;
            }
            Action::ShowHelp => {
                self.show_help = true;
                return;
            }
            Action::TogglePanels => {
                self.panels_hidden = !self.panels_hidden;
                self.mark_dirty(now);
                return;
            }
            Action::Reload => return self.reload(ctx),
            _ => {}
        }
        let Some(scene) = &mut self.scene else { return };
        match action {
            Action::Select(n) => self.selected = n,
            Action::Toggle(NodeRef::Group(g)) => {
                let open = !scene.grps[g].expanded;
                scene.set_group_expanded(g, open);
                self.mark_dirty(now);
                if open {
                    self.bring_into_view(NodeRef::Group(g), false);
                }
            }
            Action::Toggle(NodeRef::Entity(e)) => {
                let show = !scene.fields_shown(e);
                scene.set_fields(&[e], show);
                if show {
                    scene.set_fields_visible(true);
                }
                self.mark_dirty(now);
                if show {
                    self.bring_into_view(NodeRef::Entity(e), false);
                }
            }
            Action::Dive(NodeRef::Group(g)) => {
                scene.set_group_expanded(g, true);
                let first = scene.grouping.groups[g]
                    .members
                    .iter()
                    .copied()
                    .max_by_key(|&e| (scene.diagram.relationships_of(e).count(), std::cmp::Reverse(e)));
                self.selected = first.map(NodeRef::Entity);
                self.mark_dirty(now);
                self.bring_into_view(NodeRef::Group(g), false);
            }
            Action::Dive(n @ NodeRef::Entity(_)) => self.apply(ctx, Action::Toggle(n), now),
            Action::Up => {
                self.selected = match self.selected {
                    Some(NodeRef::Entity(e)) => scene.group_of(e).map(NodeRef::Group),
                    _ => None,
                };
            }
            Action::CollapseParent(n) => {
                let g = match n {
                    NodeRef::Group(g) => Some(g),
                    NodeRef::Entity(e) => scene.group_of(e),
                };
                if let Some(g) = g {
                    scene.set_group_expanded(g, false);
                    self.selected = Some(NodeRef::Group(g));
                    self.mark_dirty(now);
                }
            }
            Action::ToggleFields(NodeRef::Entity(e)) => self.apply(ctx, Action::Toggle(NodeRef::Entity(e)), now),
            Action::ToggleFields(NodeRef::Group(g)) => {
                let members = scene.grouping.groups[g].members.clone();
                let show = !members.iter().any(|&e| scene.fields_shown(e));
                scene.set_fields(&members, show);
                if show {
                    scene.set_fields_visible(true);
                    scene.set_group_expanded(g, true);
                }
                self.mark_dirty(now);
                self.bring_into_view(NodeRef::Group(g), false);
            }
            Action::AllFields(show) => {
                let all: Vec<usize> = (0..scene.ents.len()).collect();
                scene.set_fields(&all, show);
                if show {
                    scene.set_fields_visible(true);
                }
                self.mark_dirty(now);
            }
            Action::AllGroups(open) => {
                scene.set_all_groups_expanded(open);
                self.mark_dirty(now);
                self.fit(true);
            }
            Action::Reveal(e) => {
                if let Some(g) = scene.group_of(e) {
                    scene.set_group_expanded(g, true);
                }
                self.selected = Some(NodeRef::Entity(e));
                self.mark_dirty(now);
                self.bring_into_view(NodeRef::Entity(e), true);
            }
            Action::Center(n) => {
                self.selected = Some(n);
                self.bring_into_view(n, true);
            }
            Action::Navigate(dir) => {
                if let Some(n) = self.spatial_neighbour(dir) {
                    self.selected = Some(n);
                    self.bring_into_view(n, false);
                }
            }
            Action::Cycle(forward) => {
                let order = self.navigable();
                if order.is_empty() {
                    return;
                }
                let i = self.selected.and_then(|s| order.iter().position(|&n| n == s));
                let next = match (i, forward) {
                    (None, _) => 0,
                    (Some(i), true) => (i + 1) % order.len(),
                    (Some(i), false) => (i + order.len() - 1) % order.len(),
                };
                self.selected = Some(order[next]);
                self.bring_into_view(order[next], false);
            }
            Action::ToggleFieldsVisible => {
                let visible = !scene.fields_visible;
                scene.set_fields_visible(visible);
                self.mark_dirty(now);
            }
            Action::FocusSearch => self.focus_search = true,
            Action::ToggleDescriptions => {
                self.desc_toggle = !self.desc_toggle;
                self.mark_dirty(now);
            }
            Action::Fit => self.fit(true),
            Action::ZoomBy(f) => {
                let mut goal = self.cam_goal.unwrap_or(self.cam);
                goal.zoom_around(self.canvas, self.canvas.center(), f);
                self.cam_goal = Some(goal);
            }
            Action::Relayout => {
                scene.relayout();
                self.mark_dirty(now);
                self.fit(true);
            }
            Action::ToggleTag(t) => {
                let tag = scene.diagram.tags[t].clone();
                if !self.tag_highlight.remove(&tag) {
                    self.tag_highlight.insert(tag);
                }
                self.mark_dirty(now);
            }
            Action::FocusTag(t) => {
                let tag = scene.diagram.tags[t].clone();
                let open: Vec<bool> = scene
                    .grouping
                    .groups
                    .iter()
                    .map(|g| g.members.iter().any(|&e| scene.diagram.entities[e].tags.contains(&tag)))
                    .collect();
                scene.set_groups_expanded(&open);
                self.tag_highlight.insert(tag);
                self.mark_dirty(now);
                self.fit(true);
            }
            Action::NextChange(forward) => {
                let targets = change_targets(scene);
                if targets.is_empty() {
                    return;
                }
                let at = match self.selected {
                    Some(NodeRef::Entity(e)) => targets.iter().position(|&t| t == e),
                    _ => None,
                };
                let next = match (at, forward) {
                    (None, true) => 0,
                    (None, false) => targets.len() - 1,
                    (Some(i), true) => (i + 1) % targets.len(),
                    (Some(i), false) => (i + targets.len() - 1) % targets.len(),
                };
                self.apply(ctx, Action::Reveal(targets[next]), now);
            }
            Action::Mode(mode) => {
                if mode != scene.mode {
                    let (diagram, marks) = (scene.diagram.clone(), scene.diff.clone());
                    self.set_scene(ctx, diagram, marks, mode, None);
                    self.selected = None;
                    self.mark_dirty(now);
                    self.fit_pending = true;
                }
            }
            Action::Open
            | Action::LoadExample
            | Action::Reload
            | Action::OpenPalette
            | Action::ShowHelp
            | Action::TogglePanels
            | Action::CompareHead
            | Action::PickCommit
            | Action::CompareCommit(_)
            | Action::CompareFile
            | Action::ExitDiff => unreachable!(),
        }
    }

    /// The current file, when it is in a git work tree.
    fn current_git_file(&self) -> Option<vcs::GitFile> {
        self.git.clone()
    }

    fn in_diff(&self) -> bool {
        matches!(self.source, Some(Source::Diff { .. }))
    }

    fn has_native_menu(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.native_menu.is_some();
        #[cfg(not(target_os = "macos"))]
        false
    }

    #[cfg(target_os = "macos")]
    fn native_menu_actions(&mut self, actions: &mut Vec<Action>) {
        let Some(menu) = &mut self.native_menu else { return };
        let chosen = menu.take_actions();
        if !chosen.is_empty() {
            // A click toggles a check item natively; rewrite all marks from the app's state.
            menu.invalidate();
        }
        actions.extend(chosen);
    }

    /// Checkmarks and enabled items follow the app state (call after applying actions).
    #[cfg(target_os = "macos")]
    fn sync_native_menu(&mut self) {
        let state = crate::native_menu::MenuState {
            has_scene: self.scene.is_some(),
            has_file: matches!(self.source, Some(Source::File(_) | Source::Diff { .. })),
            has_git: self.git.is_some(),
            in_diff: self.in_diff(),
            has_descriptions: self.scene.as_ref().is_some_and(Scene::has_descriptions),
            fields: self.scene.as_ref().is_some_and(|s| s.fields_visible),
            descriptions: self.desc_toggle,
            panels_hidden: self.panels_hidden,
            mode: self.scene.as_ref().map(|s| s.mode),
        };
        if let Some(menu) = &mut self.native_menu {
            menu.sync(state);
        }
    }

    /// Everything the command palette offers, in the order shown before typing.
    fn palette_items(&self) -> Vec<Item<Action>> {
        let sc = |m: Modifiers, k: Key| Some(egui::KeyboardShortcut::new(m, k));
        let none = Modifiers::NONE;
        let mut v = Vec::new();
        if self.palette_mode == PaletteMode::History {
            return self
                .history
                .iter()
                .enumerate()
                .map(|(i, c)| Item {
                    detail: Some(format!("{} · {}", c.short_id(), c.date)),
                    ..Item::command(c.summary.clone(), None, Action::CompareCommit(i))
                })
                .collect();
        }
        let Some(scene) = &self.scene else {
            v.push(Item::command("Open file…", sc(Modifiers::COMMAND, Key::O), Action::Open));
            v.push(Item::command("Load example", None, Action::LoadExample));
            return v;
        };

        v.push(Item::flag("Show fields", scene.fields_visible, sc(Modifiers::SHIFT, Key::F), Action::ToggleFieldsVisible));
        if scene.has_descriptions() {
            v.push(Item::flag("Show descriptions", self.desc_toggle, sc(none, Key::D), Action::ToggleDescriptions));
        }
        v.push(Item::flag("Hide panels", self.panels_hidden, sc(none, Key::P), Action::TogglePanels));
        match self.selected {
            Some(n @ NodeRef::Group(g)) => {
                let name = &scene.grouping.groups[g].name;
                v.push(Item::flag(format!("Expanded: {name}"), scene.grps[g].expanded, sc(none, Key::Space), Action::Toggle(n)));
                let any = scene.grouping.groups[g].members.iter().any(|&e| scene.fields_shown(e));
                v.push(Item::flag(format!("Fields of all members: {name}"), any, sc(none, Key::F), Action::ToggleFields(n)));
                v.push(Item::command(format!("Center: {name}"), sc(none, Key::Period), Action::Center(n)));
            }
            Some(n @ NodeRef::Entity(e)) => {
                let name = scene.diagram.entities[e].display_name();
                v.push(Item::flag(format!("Fields: {name}"), scene.fields_shown(e), sc(none, Key::Space), Action::Toggle(n)));
                v.push(Item::command(format!("Center: {name}"), sc(none, Key::Period), Action::Center(n)));
                if let Some(g) = scene.group_of(e) {
                    let group = &scene.grouping.groups[g].name;
                    v.push(Item::command(format!("Collapse group: {group}"), sc(none, Key::C), Action::CollapseParent(n)));
                }
            }
            None => {}
        }
        v.extend([
            Item::command("Fit all", sc(none, Key::Num0), Action::Fit),
            Item::command("Zoom in", sc(none, Key::Plus), Action::ZoomBy(1.25)),
            Item::command("Zoom out", sc(none, Key::Minus), Action::ZoomBy(0.8)),
            Item::command("Collapse all groups", sc(none, Key::G), Action::AllGroups(false)),
            Item::command("Expand all groups", sc(Modifiers::SHIFT, Key::G), Action::AllGroups(true)),
            Item::command("Show fields of all entities", None, Action::AllFields(true)),
            Item::command("Hide fields of all entities", None, Action::AllFields(false)),
            Item::command("Re-layout", sc(none, Key::L), Action::Relayout),
            Item::command("Search entities and fields", sc(none, Key::Slash), Action::FocusSearch),
        ]);
        for m in GroupingMode::ALL {
            v.push(Item::flag(format!("Grouping: {}", m.label()), scene.mode == m, None, Action::Mode(m)));
        }
        for (t, tag) in scene.diagram.tags.iter().enumerate() {
            v.push(Item { needs_query: true, ..Item::flag(format!("Highlight tag: #{tag}"), self.tag_highlight.contains(tag), None, Action::ToggleTag(t)) });
            v.push(Item { needs_query: true, ..Item::command(format!("Focus tag: #{tag} (expand only its groups)"), None, Action::FocusTag(t)) });
        }
        if scene.diff.is_some() {
            v.push(Item::command("Next change", sc(none, Key::N), Action::NextChange(true)));
            v.push(Item::command("Previous change", sc(Modifiers::SHIFT, Key::N), Action::NextChange(false)));
            v.push(Item::command("Exit diff", None, Action::ExitDiff));
        }
        if self.source.as_ref().and_then(Source::path).is_some() {
            if self.current_git_file().is_some() {
                v.push(Item::command("Compare with HEAD", None, Action::CompareHead));
                v.push(Item::command("Compare with commit…", None, Action::PickCommit));
            }
            v.push(Item::command("Compare with file…", None, Action::CompareFile));
        }
        v.extend([
            Item::command("Open file…", sc(Modifiers::COMMAND, Key::O), Action::Open),
            Item::command("Reload file", sc(Modifiers::COMMAND, Key::R), Action::Reload),
            Item::command("Load example", None, Action::LoadExample),
            Item::command("Keyboard shortcuts", sc(none, Key::F1), Action::ShowHelp),
        ]);
        // Navigation targets, listed once something is typed.
        for (g, group) in scene.grouping.groups.iter().enumerate() {
            v.push(Item {
                detail: Some(format!("group · {} entities", group.members.len())),
                needs_query: true,
                ..Item::command(group.name.clone(), None, Action::Center(NodeRef::Group(g)))
            });
        }
        for (e, ent) in scene.diagram.entities.iter().enumerate() {
            let detail = match scene.group_of(e) {
                Some(g) => format!("entity in {}", scene.grouping.groups[g].name),
                None => "entity".to_owned(),
            };
            v.push(Item { detail: Some(detail), needs_query: true, ..Item::command(ent.display_name(), None, Action::Reveal(e)) });
        }
        v
    }

    /// Nodes reachable by keyboard navigation, in reading order.
    fn navigable(&self) -> Vec<NodeRef> {
        let Some(scene) = &self.scene else { return Vec::new() };
        let mut v: Vec<NodeRef> = scene
            .paint_order()
            .into_iter()
            .filter(|n| !matches!(n, NodeRef::Group(g) if scene.grps[*g].expanded))
            .collect();
        v.sort_by(|a, b| {
            let (pa, pb) = (scene.node_rect(*a, Which::Target).center(), scene.node_rect(*b, Which::Target).center());
            ((pa.y / 60.0).round(), pa.x).partial_cmp(&((pb.y / 60.0).round(), pb.x)).unwrap()
        });
        v
    }

    fn spatial_neighbour(&self, dir: Vec2) -> Option<NodeRef> {
        let scene = self.scene.as_ref()?;
        let candidates = self.navigable();
        let from = match self.selected {
            Some(n) => scene.node_rect(n, Which::Target).center(),
            None => return candidates.first().copied(),
        };
        candidates
            .into_iter()
            .filter(|&n| Some(n) != self.selected)
            .filter_map(|n| {
                let d = scene.node_rect(n, Which::Target).center() - from;
                let along = d.dot(dir);
                (along > 1.0).then(|| (n, along + 2.5 * d.dot(dir.rot90()).abs()))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(n, _)| n)
    }

    // ---- input -------------------------------------------------------------------------------

    fn keyboard(&mut self, ctx: &Context, actions: &mut Vec<Action>) {
        let typing = ctx.egui_wants_keyboard_input();
        let sel = self.selected;
        let native_menu = self.has_native_menu();
        ctx.input_mut(|i| {
            let mut key = |m: Modifiers, k: Key| i.consume_key(m, k);
            // With a native menu bar these are its key equivalents, handled there.
            if !native_menu {
                for (k, a) in [(Key::O, Action::Open), (Key::R, Action::Reload), (Key::F, Action::FocusSearch), (Key::K, Action::OpenPalette)] {
                    if key(Modifiers::COMMAND, k) {
                        actions.push(a);
                    }
                }
            }
            // The palette handles its own keys while open.
            if typing || self.palette.is_some() {
                return;
            }
            if key(Modifiers::NONE, Key::Slash) {
                self.focus_search = true;
            }
            if key(Modifiers::NONE, Key::F1) || key(Modifiers::SHIFT, Key::Questionmark) || key(Modifiers::NONE, Key::Questionmark) {
                self.show_help = !self.show_help;
            }
            if key(Modifiers::SHIFT, Key::F) {
                actions.push(Action::ToggleFieldsVisible);
            }
            if key(Modifiers::SHIFT, Key::G) {
                actions.push(Action::AllGroups(true));
            }
            if key(Modifiers::NONE, Key::G) {
                actions.push(Action::AllGroups(false));
            }
            if key(Modifiers::SHIFT, Key::Tab) {
                actions.push(Action::Cycle(false));
            }
            if key(Modifiers::NONE, Key::Tab) {
                actions.push(Action::Cycle(true));
            }
            for (k, d) in [
                (Key::ArrowLeft, vec2(-1.0, 0.0)),
                (Key::ArrowRight, vec2(1.0, 0.0)),
                (Key::ArrowUp, vec2(0.0, -1.0)),
                (Key::ArrowDown, vec2(0.0, 1.0)),
            ] {
                if key(Modifiers::NONE, k) {
                    actions.push(Action::Navigate(d));
                }
            }
            if key(Modifiers::NONE, Key::Num0) || key(Modifiers::NONE, Key::Home) {
                actions.push(Action::Fit);
            }
            if key(Modifiers::NONE, Key::Plus) || key(Modifiers::NONE, Key::Equals) {
                actions.push(Action::ZoomBy(1.25));
            }
            if key(Modifiers::NONE, Key::Minus) {
                actions.push(Action::ZoomBy(0.8));
            }
            if key(Modifiers::NONE, Key::P) {
                actions.push(Action::TogglePanels);
            }
            if key(Modifiers::NONE, Key::D) {
                actions.push(Action::ToggleDescriptions);
            }
            if key(Modifiers::NONE, Key::L) {
                actions.push(Action::Relayout);
            }
            if key(Modifiers::SHIFT, Key::N) {
                actions.push(Action::NextChange(false));
            }
            if key(Modifiers::NONE, Key::N) {
                actions.push(Action::NextChange(true));
            }
            if key(Modifiers::NONE, Key::Escape) {
                actions.push(Action::Up);
            }
            if let Some(n) = sel {
                if key(Modifiers::NONE, Key::Space) {
                    actions.push(Action::Toggle(n));
                }
                if key(Modifiers::NONE, Key::Enter) {
                    actions.push(Action::Dive(n));
                }
                if key(Modifiers::NONE, Key::F) {
                    actions.push(Action::ToggleFields(n));
                }
                if key(Modifiers::NONE, Key::C) || key(Modifiers::NONE, Key::Backspace) {
                    actions.push(Action::CollapseParent(n));
                }
                if key(Modifiers::NONE, Key::Period) || key(Modifiers::NONE, Key::Z) {
                    actions.push(Action::Center(n));
                }
            }
            // `?` often arrives only as text.
            if i.events.iter().any(|e| matches!(e, Event::Text(t) if t == "?")) {
                self.show_help = !self.show_help;
            }
        });
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>, now: f64) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        // When the canvas changes size (panels toggled, window resized), keep the same world point
        // in its center instead of letting the diagram jump sideways.
        if self.canvas.is_positive() && rect.size() != self.canvas.size() {
            let shift = (rect.size() - self.canvas.size()) * 0.5;
            self.cam.offset += shift;
            if let Some(goal) = &mut self.cam_goal {
                goal.offset += shift;
            }
        }
        self.canvas = rect;
        if self.fit_pending && self.scene.is_some() {
            self.fit_pending = false;
            self.fit(false);
        }
        let Some(scene) = &mut self.scene else { return };

        // Camera: scroll pans, pinch / ctrl+scroll zooms.
        if resp.hovered() {
            let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            if let Some(p) = resp.hover_pos()
                && zoom != 1.0
            {
                self.cam_goal = None;
                self.cam.zoom_around(rect, p, zoom);
            }
            if scroll != Vec2::ZERO {
                self.cam_goal = None;
                self.cam.offset += scroll;
            }
        }

        let pointer = resp.hover_pos().or_else(|| resp.interact_pointer_pos());
        let hit = pointer.and_then(|p| scene.hit_test(self.cam.to_world(rect, p)));

        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            let target = scene.hit_test(self.cam.to_world(rect, origin));
            self.drag = match target {
                Some(h) if resp.dragged_by(PointerButton::Primary) => {
                    self.selected = Some(h.node);
                    Some(Drag::Node(h.node))
                }
                _ => Some(Drag::Pan),
            };
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match self.drag {
                Some(Drag::Pan) => {
                    self.cam_goal = None;
                    self.cam.offset += d;
                }
                Some(Drag::Node(n)) => scene.drag(n, d / self.cam.zoom),
                None => {}
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Node(n)) = self.drag {
                scene.drag_finished(n);
                self.dirty_at = Some(now);
            }
            self.drag = None;
        }
        if resp.clicked() {
            actions.push(Action::Select(hit.map(|h| h.node)));
        }
        if resp.double_clicked()
            && let Some(h) = hit
        {
            actions.push(Action::Toggle(h.node));
        }
        if resp.secondary_clicked() {
            self.context_node = hit.map(|h| h.node);
            if self.context_node.is_some() {
                self.selected = self.context_node;
            }
        }

        // Highlights.
        let neighbours: HashSet<NodeRef> = self
            .selected
            .map(|s| scene.neighbours(s).into_iter().map(|e| scene.visible_node(e)).collect())
            .unwrap_or_default();
        let query = self.search.trim().to_lowercase();
        let search: HashSet<usize> = if query.is_empty() {
            HashSet::new()
        } else {
            (0..scene.ents.len()).filter(|&e| matches(scene, e, &query)).collect()
        };

        let pal = Palette::new(ui.visuals().dark_mode);
        let painter = ui.painter_at(rect);
        let edges = scene.visible_edges();
        let hovered_edge = self.hovered_edge.filter(|&i| i < edges.len());
        let tags = self.tag_highlight.iter().filter_map(|t| scene.diagram.tag_index(t)).collect();
        let hl = Highlight { selected: self.selected, neighbours, search, hovered_edge, tags };
        let c = Canvas { painter: &painter, rect, cam: self.cam, pal: &pal, hl: &hl };
        c.background();
        let order = scene.paint_order();
        for &n in &order {
            if let NodeRef::Group(g) = n
                && scene.grps[g].expanded
            {
                c.group_frame(scene, g);
            }
        }
        let edge_hits = c.edges(scene, &edges);
        for &n in &order {
            match n {
                NodeRef::Group(g) if !scene.grps[g].expanded => c.collapsed_group(scene, g),
                NodeRef::Entity(e) => c.entity(scene, e),
                NodeRef::Group(_) => {}
            }
        }

        // Hover: edges take precedence over the frame of an expanded group, not over boxes.
        let over_box = hit.is_some_and(|h| !matches!(h.node, NodeRef::Group(g) if scene.grps[g].expanded));
        self.hovered_edge = None;
        if let Some(p) = resp.hover_pos()
            && !over_box
            && self.drag.is_none()
        {
            self.hovered_edge = edge_hits
                .iter()
                .map(|h| (h.edge, distance_to_segment(p, h.a, h.b)))
                .filter(|(_, d)| *d < 6.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
        }

        // Each hovered item gets its own tooltip id: a tooltip keeps the size it opened with, so
        // sharing one between items would squeeze longer texts into the previous item's width.
        if self.drag.is_none() && resp.hovered() {
            let tip = |id: egui::Id, add: &mut dyn FnMut(&mut egui::Ui)| {
                egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), id, egui::PopupAnchor::Pointer)
                    .gap(12.0)
                    .show(|ui| add(ui));
            };
            if let Some(i) = self.hovered_edge {
                let edge = &edges[i];
                tip(egui::Id::new(("edge-tip", edge.a, edge.b)), &mut |ui| {
                    for &ri in &edge.rels {
                        relationship_line(ui, scene, ri);
                    }
                });
            } else if let Some(h) = hit {
                tip(egui::Id::new(("node-tip", h.node, h.field)), &mut |ui| node_tooltip(ui, scene, h.node, h.field));
            }
        }

        let ctx_node = self.context_node;
        resp.context_menu(|ui| match ctx_node {
            Some(n @ NodeRef::Group(g)) => {
                let open = scene.grps[g].expanded;
                if ui.button(if open { "Collapse group  (Space)" } else { "Expand group  (Space)" }).clicked() {
                    actions.push(Action::Toggle(n));
                }
                if ui.button("Toggle fields of all members  (F)").clicked() {
                    actions.push(Action::ToggleFields(n));
                }
                if ui.button("Center  (.)").clicked() {
                    actions.push(Action::Center(n));
                }
            }
            Some(n @ NodeRef::Entity(e)) => {
                let label = if scene.fields_shown(e) { "Hide fields  (Space)" } else { "Show fields  (Space)" };
                if ui.button(label).clicked() {
                    actions.push(Action::Toggle(n));
                }
                if scene.group_of(e).is_some() && ui.button("Collapse group  (C)").clicked() {
                    actions.push(Action::CollapseParent(n));
                }
                if ui.button("Center  (.)").clicked() {
                    actions.push(Action::Center(n));
                }
            }
            None => {
                if ui.button("Fit all  (0)").clicked() {
                    actions.push(Action::Fit);
                }
                if ui.button("Collapse all groups  (G)").clicked() {
                    actions.push(Action::AllGroups(false));
                }
                if ui.button("Expand all groups  (Shift+G)").clicked() {
                    actions.push(Action::AllGroups(true));
                }
                if ui.button("Re-layout  (L)").clicked() {
                    actions.push(Action::Relayout);
                }
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .id(egui::Id::new(SEARCH_ID))
                .hint_text("Search entities / fields  ( / )")
                .desired_width(f32::INFINITY),
        );
        if self.focus_search {
            self.focus_search = false;
            resp.request_focus();
        }
        let Some(scene) = &self.scene else { return };
        let query = self.search.trim().to_lowercase();
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.search.clear();
        }
        ui.add_space(4.0);

        // Diff mode: the changes get their own resizable, separately scrolling part on top.
        if let Some(d) = &scene.diff {
            let rows = d.entities.iter().filter(|e| e.status.differs()).count()
                + d.relationships.iter().filter(|r| r.status.differs()).count();
            let row_h = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            let avail = ui.available_height();
            let fit = (rows.max(1) + 1) as f32 * row_h + 12.0;
            // Keyed by the compared pair: a new diff starts at its fitted size.
            egui::Panel::top(egui::Id::new(("changes", self.diff_labels())))
                .resizable(true)
                .frame(egui::Frame::NONE)
                .default_size(fit.min(avail * 0.4))
                .size_range(row_h * 2.0..=(avail * 0.85).max(row_h * 2.0))
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("Changes ({rows})  —  N / Shift+N")).strong());
                    egui::ScrollArea::vertical().id_salt("changes").auto_shrink(false).show(ui, |ui| {
                        changes_list(ui, scene, d, self.selected, actions);
                    });
                });
            ui.add_space(4.0);
        }

        egui::ScrollArea::vertical().id_salt("tree").auto_shrink(false).show(ui, |ui| {
            if !query.is_empty() {
                let hits: Vec<usize> = (0..scene.ents.len()).filter(|&e| matches(scene, e, &query)).collect();
                if enter && let Some(&e) = hits.first() {
                    actions.push(Action::Reveal(e));
                }
                ui.label(RichText::new(format!("{} matches — Enter jumps to the first", hits.len())).weak());
                for e in hits {
                    let group = scene.group_of(e).map(|g| scene.grouping.groups[g].name.as_str()).unwrap_or("—");
                    let sel = self.selected == Some(NodeRef::Entity(e));
                    let r = ui.selectable_label(sel, format!("{}   ", scene.diagram.entities[e].display_name()));
                    let r = r.on_hover_text(format!("group: {group}"));
                    if r.clicked() {
                        actions.push(Action::Reveal(e));
                    }
                }
                return;
            }
            for (g, group) in scene.grouping.groups.iter().enumerate() {
                let open = scene.grps[g].expanded;
                ui.horizontal(|ui| {
                    if ui.small_button(if open { "⏷" } else { "⏵" }).on_hover_text("Expand / collapse on canvas").clicked() {
                        actions.push(Action::Toggle(NodeRef::Group(g)));
                    }
                    let sel = self.selected == Some(NodeRef::Group(g));
                    let text = RichText::new(format!("{}  ({})", group.name, group.members.len()))
                        .color(Palette::new(ui.visuals().dark_mode).group(scene.grps[g].hue))
                        .strong();
                    if ui.selectable_label(sel, text).clicked() {
                        actions.push(Action::Center(NodeRef::Group(g)));
                    }
                });
                if open {
                    ui.indent(("members", g), |ui| {
                        for &e in &group.members {
                            entity_row(ui, scene, e, self.selected, actions);
                        }
                    });
                }
            }
            let loose: Vec<usize> = (0..scene.ents.len()).filter(|&e| scene.group_of(e).is_none()).collect();
            if !loose.is_empty() {
                if !scene.grouping.groups.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Ungrouped").weak());
                }
                for e in loose {
                    entity_row(ui, scene, e, self.selected, actions);
                }
            }
        });
    }

    /// Every tag with its entity count. Click: highlight; double-click: expand only its groups.
    fn tags_panel(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(scene) = &self.scene else { return };
        ui.add_space(4.0);
        ui.label(RichText::new("Tags").strong());
        ui.horizontal_wrapped(|ui| {
            for (t, tag) in scene.diagram.tags.iter().enumerate() {
                let n = scene.diagram.entities.iter().filter(|e| e.tags.contains(tag)).count();
                let on = self.tag_highlight.contains(tag);
                let r = ui.add(tag_chip(ui, t, &format!("{tag}  {n}"), on)).on_hover_text(
                    "click: highlight on the canvas\ndouble-click: expand only the groups with this tag",
                );
                if r.clicked() {
                    actions.push(Action::ToggleTag(t));
                }
                if r.double_clicked() {
                    actions.push(Action::FocusTag(t));
                }
            }
        });
        ui.add_space(4.0);
    }

    fn details(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let (Some(scene), Some(sel)) = (&self.scene, self.selected) else {
            ui.label(RichText::new("Select an entity or group to see its fields and relationships.").weak());
            return;
        };
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match sel {
            NodeRef::Entity(e) => {
                let ent = &scene.diagram.entities[e];
                ui.heading(status_text(ui, ent.display_name(), scene.entity_status(e)));
                if let Some(d) = &scene.diff {
                    let ed = &d.entities[e];
                    match ed.status {
                        Status::Same => {}
                        Status::Added => { ui.label(status_text(ui, "added", Status::Added)); }
                        Status::Removed => { ui.label(status_text(ui, "removed", Status::Removed)); }
                        Status::Changed => {
                            for n in &ed.notes {
                                ui.label(status_text(ui, n.as_str(), Status::Changed));
                            }
                        }
                    }
                }
                if ent.label.is_some() {
                    ui.label(RichText::new(format!("id: {}", ent.id)).weak());
                }
                if !ent.tags.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for tag in &ent.tags {
                            let Some(t) = scene.diagram.tag_index(tag) else { continue };
                            if ui.add(tag_chip(ui, t, tag, self.tag_highlight.contains(tag))).clicked() {
                                actions.push(Action::ToggleTag(t));
                            }
                        }
                    });
                }
                if let Some(d) = &ent.description {
                    ui.add_space(4.0);
                    ui.label(description_job(ui, d, ui.available_width()));
                    ui.add_space(4.0);
                }
                if let Some(g) = scene.group_of(e)
                    && ui.link(format!("group: {}", scene.grouping.groups[g].name)).clicked()
                {
                    actions.push(Action::Center(NodeRef::Group(g)));
                }
                ui.horizontal(|ui| {
                    let label = if scene.fields_shown(e) { "Hide fields on canvas" } else { "Show fields on canvas" };
                    if ui.button(label).clicked() {
                        actions.push(Action::Toggle(sel));
                    }
                });
                ui.separator();
                ui.label(RichText::new(format!("Fields ({})", ent.attributes.len())).strong());
                let attr_diffs = scene.diff.as_ref().map(|d| &d.entities[e].attrs);
                egui::Grid::new("fields").striped(true).num_columns(3).show(ui, |ui| {
                    for (i, a) in ent.attributes.iter().enumerate() {
                        let ad = attr_diffs.map(|d| &d[i]);
                        let st = ad.map_or(Status::Same, |d| d.status);
                        ui.label(RichText::new(diff::keys_text(a, ad)).monospace().color(ui.visuals().warn_fg_color));
                        let name = ui.label(status_text(ui, a.name.as_str(), st).monospace());
                        if let Some(c) = &a.comment {
                            match ad.and_then(|d| d.old.as_ref()).and_then(|o| o.comment.as_ref()).filter(|o| *o != c) {
                                Some(old) => name.on_hover_text(format!("{old}\n→ {c}")),
                                None => name.on_hover_text(c),
                            };
                        }
                        ui.label(RichText::new(diff::type_text(a, ad)).monospace().weak());
                        ui.end_row();
                    }
                });
                ui.separator();
                let rels: Vec<usize> = scene.diagram.relationships_of(e).collect();
                ui.label(RichText::new(format!("Relationships ({})", rels.len())).strong());
                for ri in rels {
                    let r = &scene.diagram.relationships[ri];
                    let (other, mine, theirs) = if r.a == e { (r.b, r.card_a, r.card_b) } else { (r.a, r.card_b, r.card_a) };
                    let st = scene.rel_status(ri);
                    ui.horizontal_wrapped(|ui| {
                        if st.differs() {
                            ui.label(status_text(ui, st.sign(), st).strong());
                        }
                        ui.label(RichText::new(format!("{} → {}", mine.short(), theirs.short())).monospace().weak());
                        if ui.link(scene.diagram.entities[other].display_name()).clicked() {
                            actions.push(Action::Reveal(other));
                        }
                        if !r.label.is_empty() {
                            ui.label(RichText::new(&r.label).italics());
                        }
                    });
                }
            }
            NodeRef::Group(g) => {
                let group = &scene.grouping.groups[g];
                ui.heading(&group.name);
                ui.horizontal(|ui| {
                    if ui.button(if scene.grps[g].expanded { "Collapse" } else { "Expand" }).clicked() {
                        actions.push(Action::Toggle(sel));
                    }
                    if ui.button("Toggle all fields").clicked() {
                        actions.push(Action::ToggleFields(sel));
                    }
                });
                ui.separator();
                ui.label(RichText::new(format!("Members ({})", group.members.len())).strong());
                for &e in &group.members {
                    if ui.link(scene.diagram.entities[e].display_name()).clicked() {
                        actions.push(Action::Reveal(e));
                    }
                }
                ui.separator();
                let outside = scene.neighbours(sel);
                ui.label(RichText::new(format!("Related outside the group ({})", outside.len())).strong());
                for e in outside {
                    ui.horizontal(|ui| {
                        if ui.link(scene.diagram.entities[e].display_name()).clicked() {
                            actions.push(Action::Reveal(e));
                        }
                        if let Some(og) = scene.group_of(e) {
                            ui.label(RichText::new(format!("in {}", scene.grouping.groups[og].name)).weak());
                        }
                    });
                }
            }
        });
    }

    fn menu(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open…  (⌘O)").clicked() {
                    actions.push(Action::Open);
                }
                if ui.button("Reload  (⌘R)").clicked() {
                    actions.push(Action::Reload);
                }
                if ui.button("Load example").clicked() {
                    actions.push(Action::LoadExample);
                }
            });
            ui.menu_button("Compare", |ui| {
                let has_file = self.source.as_ref().and_then(Source::path).is_some();
                let has_git = self.git.is_some();
                let in_diff = self.in_diff();
                for (label, enabled, a) in [
                    ("Compare with HEAD", has_git, Action::CompareHead),
                    ("Compare with commit…", has_git, Action::PickCommit),
                    ("Compare with file…", has_file, Action::CompareFile),
                    ("Next change  (N)", in_diff, Action::NextChange(true)),
                    ("Previous change  (Shift+N)", in_diff, Action::NextChange(false)),
                    ("Exit diff", in_diff, Action::ExitDiff),
                ] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        actions.push(a);
                    }
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Command palette  (⌘K / Ctrl+K)").clicked() {
                    self.palette = Some(CommandPalette::new());
                    self.palette_mode = PaletteMode::Commands;
                    ui.close();
                }
                let mut fields = self.scene.as_ref().is_none_or(|s| s.fields_visible);
                if ui.checkbox(&mut fields, "Show fields  (Shift+F)").clicked() {
                    actions.push(Action::ToggleFieldsVisible);
                }
                let mut hidden = self.panels_hidden;
                if ui.checkbox(&mut hidden, "Hide panels  (P)").clicked() {
                    actions.push(Action::TogglePanels);
                }
                ui.separator();
                for (label, a) in [
                    ("Fit all  (0)", Action::Fit),
                    ("Collapse all groups  (G)", Action::AllGroups(false)),
                    ("Expand all groups  (Shift+G)", Action::AllGroups(true)),
                    ("Show all fields", Action::AllFields(true)),
                    ("Hide all fields", Action::AllFields(false)),
                    ("Re-layout  (L)", Action::Relayout),
                ] {
                    if ui.button(label).clicked() {
                        actions.push(a);
                    }
                }
            });
            ui.menu_button("Descriptions", |ui| {
                let has = self.scene.as_ref().is_some_and(Scene::has_descriptions);
                let mut on = self.desc_toggle;
                let r = ui.add_enabled(has, egui::Checkbox::new(&mut on, "Show descriptions in boxes  (D)"));
                if r.clicked() {
                    actions.push(Action::ToggleDescriptions);
                }
                ui.label(RichText::new(if has {
                    "Hidden automatically when zoomed out."
                } else {
                    "Add `%% describe ENTITY: text` lines to the file."
                }).weak());
            });
            ui.menu_button("Grouping", |ui| {
                let current = self.scene.as_ref().map(|s| s.mode);
                for m in GroupingMode::ALL {
                    if ui.radio(current == Some(m), m.label()).clicked() {
                        actions.push(Action::Mode(m));
                    }
                }
            });
            if ui.button("Shortcuts  (?)").clicked() {
                self.show_help = !self.show_help;
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.status);
            if let Some(scene) = &self.scene
                && !scene.diagram.warnings.is_empty()
            {
                ui.label(RichText::new("⚠").color(ui.visuals().warn_fg_color))
                    .on_hover_text(scene.diagram.warnings.join("\n"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!("{:.0}%", self.cam.zoom * 100.0));
                if self.desc_toggle && self.scene.as_ref().is_some_and(|s| !s.show_descriptions) {
                    ui.label(RichText::new("descriptions hidden at this zoom").color(ui.visuals().warn_fg_color));
                }
                ui.label(RichText::new("scroll: pan · pinch / ⌘-scroll: zoom · ? shortcuts").weak());
            });
        });
    }

    fn welcome(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("ER Navigator");
            ui.label("Open a Mermaid erDiagram (.mmd, or Markdown with ```mermaid blocks), or drop a file here.");
            ui.add_space(12.0);
            if ui.button("Open file…").clicked() {
                actions.push(Action::Open);
            }
            if ui.button("Load example").clicked() {
                actions.push(Action::LoadExample);
            }
            if !self.status.is_empty() {
                ui.add_space(12.0);
                ui.colored_label(ui.visuals().warn_fg_color, &self.status);
            }
        });
    }
}

impl eframe::App for ErApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);

        if let Some(src) = self.pending.take() {
            self.load(&ctx, src);
        }
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| (!f.path().as_os_str().is_empty()).then(|| f.path().to_path_buf()))) {
            self.load(&ctx, Source::File(path));
        }
        self.poll_file(&ctx, now);

        if let Some(scene) = &mut self.scene {
            let zoom = self.cam.zoom;
            let show = self.desc_toggle
                && if scene.show_descriptions { zoom >= DESC_ZOOM_OFF } else { zoom >= DESC_ZOOM_ON };
            scene.set_descriptions(show);
        }

        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);

        #[cfg(target_os = "macos")]
        self.native_menu_actions(&mut actions);
        if !self.has_native_menu() {
            egui::Panel::top("menu").show(ui, |ui| self.menu(ui, &mut actions));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.scene.is_some() && !self.panels_hidden {
            egui::Panel::left("tree").default_size(250.0).show(ui, |ui| {
                if self.scene.as_ref().is_some_and(|s| !s.diagram.tags.is_empty()) {
                    egui::Panel::bottom("tags").show(ui, |ui| self.tags_panel(ui, &mut actions));
                }
                self.sidebar(ui, &mut actions);
            });
            egui::Panel::right("details").default_size(290.0).show(ui, |ui| self.details(ui, &mut actions));
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            if self.scene.is_some() {
                self.canvas_ui(ui, &mut actions, now);
            } else {
                self.welcome(ui, &mut actions);
            }
        });

        if let Some(mut palette) = self.palette.take() {
            let items = self.palette_items();
            let out = palette.show(&ctx, &items);
            actions.extend(out.run);
            if out.close {
                self.palette_mode = PaletteMode::Commands;
            } else {
                self.palette = Some(palette);
            }
        }

        for a in actions {
            self.apply(&ctx, a, now);
        }
        #[cfg(target_os = "macos")]
        self.sync_native_menu();

        if self.show_help {
            let mut open = true;
            egui::Window::new("Shortcuts").open(&mut open).collapsible(false).resizable(false).show(&ctx, help_ui);
            self.show_help = open;
        }

        let mut moving = self.scene.as_mut().is_some_and(|s| s.animate(dt));
        moving |= self.animate_camera(dt);
        if moving {
            ctx.request_repaint();
        }
        self.save_if_due(now);
        if self.dirty_at.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(1100));
        }
        if !self.watched.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// Reads and parses a source: the diagram (merged, in diff mode), the diff marks, a title.
fn read_source(source: &Source) -> Result<(Diagram, Option<DiffMarks>, String), String> {
    let parse = |text: &str| mermaid::parse(&mermaid::extract_er_source(text));
    match source {
        Source::Example => Ok((parse(EXAMPLE), None, "example: commerce.mmd".to_owned())),
        Source::File(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("Cannot read {}: {e}", p.display()))?;
            Ok((parse(&text), None, p.display().to_string()))
        }
        Source::Diff { old, new } => {
            // A side without the file (not yet committed, deleted) is an empty diagram.
            let side = |s: &Side| vcs::read(s).map(|t| t.as_deref().map(parse).unwrap_or_default());
            let r = diff::diff(&side(old)?, &side(new)?);
            let title = format!("diff: {} → {} — {}", old.label(), new.label(), new.layout_path().display());
            Ok((r.merged, Some(r.marks), title))
        }
    }
}

/// Entities to visit with N / Shift+N: changed entities, and one end of each changed
/// relationship between unchanged entities, in list order.
fn change_targets(scene: &Scene) -> Vec<usize> {
    let Some(d) = &scene.diff else { return Vec::new() };
    let mut v: Vec<usize> = (0..d.entities.len()).filter(|&e| d.entities[e].status.differs()).collect();
    for (ri, rd) in d.relationships.iter().enumerate() {
        let r = &scene.diagram.relationships[ri];
        if rd.status.differs() && !v.contains(&r.a) && !v.contains(&r.b) {
            v.push(r.a);
        }
    }
    v
}

fn status_text(ui: &egui::Ui, text: impl Into<String>, s: Status) -> RichText {
    let t = RichText::new(text.into());
    match Palette::new(ui.visuals().dark_mode).status(s) {
        Some(c) if s == Status::Removed => t.color(c).strikethrough(),
        Some(c) => t.color(c),
        None => t,
    }
}

fn matches(scene: &Scene, e: usize, query: &str) -> bool {
    let ent = &scene.diagram.entities[e];
    ent.display_name().to_lowercase().contains(query)
        || ent.id.to_lowercase().contains(query)
        || ent.attributes.iter().any(|a| a.name.to_lowercase().contains(query))
        || ent.tags.iter().any(|t| format!("#{}", t.to_lowercase()).contains(query))
}

/// Diff mode: every change, entities first; clicking one reveals it.
fn changes_list(ui: &mut egui::Ui, scene: &Scene, d: &DiffMarks, selected: Option<NodeRef>, actions: &mut Vec<Action>) {
    let ents: Vec<usize> = (0..d.entities.len()).filter(|&e| d.entities[e].status.differs()).collect();
    let rels: Vec<usize> = (0..d.relationships.len()).filter(|&r| d.relationships[r].status.differs()).collect();
    if ents.is_empty() && rels.is_empty() {
        ui.label(RichText::new("No differences.").weak());
    }
    for e in ents {
        let st = d.entities[e].status;
        let text = status_text(ui, format!("{} {}", st.sign(), scene.diagram.entities[e].display_name()), st);
        let mut r = ui.selectable_label(selected == Some(NodeRef::Entity(e)), text);
        if !d.entities[e].notes.is_empty() {
            r = r.on_hover_text(d.entities[e].notes.join("\n"));
        }
        if r.clicked() {
            actions.push(Action::Reveal(e));
        }
    }
    for ri in rels {
        let rel = &scene.diagram.relationships[ri];
        let st = d.relationships[ri].status;
        let name = |e: usize| scene.diagram.entities[e].display_name();
        let text = status_text(ui, format!("{} {} — {} — {}", st.sign(), name(rel.a), rel.label, name(rel.b)), st);
        let mut r = ui.selectable_label(false, text);
        if let Some(n) = &d.relationships[ri].note {
            r = r.on_hover_text(n);
        }
        if r.clicked() {
            actions.push(Action::Reveal(rel.a));
        }
    }
}

/// A tag as a rounded button in its color: outlined, or filled when `on`.
fn tag_chip(ui: &egui::Ui, t: usize, text: &str, on: bool) -> egui::Button<'static> {
    let pal = Palette::new(ui.visuals().dark_mode);
    let c = pal.tag(t);
    let (fill, fg) = if on { (c, pal.tag_text_on_fill()) } else { (Color32::TRANSPARENT, c) };
    egui::Button::new(RichText::new(text.to_owned()).color(fg).small())
        .fill(fill)
        .stroke(egui::Stroke::new(1.2, c))
        .corner_radius(8)
}

fn entity_row(ui: &mut egui::Ui, scene: &Scene, e: usize, selected: Option<NodeRef>, actions: &mut Vec<Action>) {
    let ent = &scene.diagram.entities[e];
    ui.horizontal(|ui| {
        let open = scene.fields_shown(e);
        let toggle = ui.add_enabled(!ent.attributes.is_empty(), egui::Button::new(if open { "−" } else { "+" }).small());
        if toggle.on_hover_text("Show / hide fields").clicked() {
            actions.push(Action::Toggle(NodeRef::Entity(e)));
        }
        if ui.selectable_label(selected == Some(NodeRef::Entity(e)), ent.display_name()).clicked() {
            actions.push(Action::Reveal(e));
        }
    });
}

fn description_job(ui: &egui::Ui, text: &str, wrap: f32) -> egui::text::LayoutJob {
    let v = ui.visuals();
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    crate::richtext::job(text, size, v.text_color(), v.strong_text_color(), wrap)
}

fn relationship_line(ui: &mut egui::Ui, scene: &Scene, ri: usize) {
    let r = &scene.diagram.relationships[ri];
    let a = scene.diagram.entities[r.a].display_name();
    let b = scene.diagram.entities[r.b].display_name();
    let dash = if r.identifying { "──" } else { "┄┄" };
    let st = scene.rel_status(ri);
    let line = format!("{a} ({}) {dash} {} {dash} ({}) {b}", r.card_a.short(), r.label, r.card_b.short());
    match scene.diff.as_ref().and_then(|d| d.relationships[ri].note.as_ref()) {
        Some(note) => ui.label(status_text(ui, format!("{line}   was {note}"), st)),
        None if st.differs() => ui.label(status_text(ui, format!("{} {line}", st.sign()), st)),
        None => ui.label(line),
    };
}

fn node_tooltip(ui: &mut egui::Ui, scene: &Scene, n: NodeRef, field: Option<usize>) {
    match (n, field) {
        (NodeRef::Entity(e), Some(f)) => {
            let a = &scene.diagram.entities[e].attributes[f];
            ui.label(RichText::new(format!("{} {}", a.ty, a.name)).monospace().strong());
            if let Some(ad) = scene.diff.as_ref().map(|d| &d.entities[e].attrs[f]) {
                match &ad.old {
                    Some(old) => {
                        let keys = if old.keys.is_empty() { String::new() } else { format!(" {}", old.keys.join(",")) };
                        let comment = old.comment.as_ref().map(|c| format!("  \"{c}\"")).unwrap_or_default();
                        ui.label(status_text(ui, format!("was: {} {}{keys}{comment}", old.ty, old.name), Status::Changed).monospace());
                    }
                    None if ad.status.differs() && ad.status != scene.entity_status(e) => {
                        let word = if ad.status == Status::Added { "added" } else { "removed" };
                        ui.label(status_text(ui, word, ad.status));
                    }
                    None => {}
                }
            }
            if !a.keys.is_empty() {
                ui.label(a.keys.join(", "));
            }
            if let Some(c) = &a.comment {
                ui.label(c);
            }
        }
        (NodeRef::Entity(e), None) => {
            let ent = &scene.diagram.entities[e];
            ui.label(RichText::new(ent.display_name()).strong());
            if let Some(d) = &scene.diff {
                let ed = &d.entities[e];
                match ed.status {
                    Status::Same => {}
                    Status::Added => { ui.label(status_text(ui, "added", Status::Added)); }
                    Status::Removed => { ui.label(status_text(ui, "removed", Status::Removed)); }
                    Status::Changed => {
                        for n in &ed.notes {
                            ui.label(status_text(ui, n.as_str(), Status::Changed));
                        }
                    }
                }
            }
            if !ent.tags.is_empty() {
                let tags: Vec<String> = ent.tags.iter().map(|t| format!("#{t}")).collect();
                ui.label(RichText::new(tags.join(" ")).weak());
            }
            if let Some(d) = &ent.description {
                ui.set_max_width(380.0);
                ui.label(description_job(ui, d, 380.0));
            }
            ui.label(format!(
                "{} fields · {} relationships",
                ent.attributes.len(),
                scene.diagram.relationships_of(e).count()
            ));
            ui.label(RichText::new("double-click / Space: toggle fields").weak());
        }
        (NodeRef::Group(g), _) => {
            ui.label(RichText::new(&scene.grouping.groups[g].name).strong());
            if let Some(d) = &scene.diff {
                let c = d.counts_of(&scene.grouping.groups[g].members);
                if !c.is_empty() {
                    ui.label(format!("changes: {} (added, removed, changed entities)", c.badge()));
                }
            }
            ui.label(format!(
                "{} entities · {} related outside",
                scene.grouping.groups[g].members.len(),
                scene.neighbours(n).len()
            ));
            let hint = if scene.grps[g].expanded { "double-click / Space: collapse" } else { "double-click / Space: expand · Enter: expand & dive in" };
            ui.label(RichText::new(hint).weak());
        }
    }
}

fn help_ui(ui: &mut egui::Ui) {
    egui::Grid::new("help").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
        for (k, v) in [
            ("click / Tab / arrows", "select (arrows move to the nearest box in that direction)"),
            ("Space, double-click", "expand / collapse group · show / hide entity fields"),
            ("Enter", "group: expand and select its main entity"),
            ("Esc", "go up: entity → its group → nothing"),
            ("C / Backspace", "collapse the selected (or enclosing) group"),
            ("F", "toggle fields of entity / all group members"),
            ("Shift+F", "show / hide fields everywhere (each box keeps its own state)"),
            ("G / Shift+G", "collapse / expand all groups"),
            (". or Z", "center the selection"),
            ("0 / Home", "fit everything"),
            ("+ / −", "zoom"),
            ("D", "show / hide descriptions in boxes (only when zoomed in)"),
            ("P", "hide / show both side panels"),
            ("L", "re-layout from scratch"),
            ("N / Shift+N", "diff: next / previous change"),
            ("⌘K / Ctrl+K", "command palette: type ahead, Space toggles settings"),
            ("/ or ⌘F", "search (Enter jumps to first match)"),
            ("⌘O / ⌘R", "open / reload (files also reload on change)"),
            ("drag box", "move it (others make room)"),
            ("drag background, scroll", "pan"),
            ("pinch, ⌘/Ctrl+scroll", "zoom"),
            ("right-click", "context menu"),
        ] {
            ui.label(RichText::new(k).monospace().strong());
            ui.label(v);
            ui.end_row();
        }
    });
    ui.separator();
    ui.label(RichText::new("Groups come from `%% group Name: A, B, C` lines in the Mermaid source,").weak());
    ui.label(RichText::new("or automatically (Grouping menu). Layout is saved next to the file as *.layout.json.").weak());
    ui.label(RichText::new("Tags: `#TAG` in `%% describe` lines. Tags panel: click highlights, double-click expands only its groups.").weak());
    ui.label(RichText::new("Diff: `ernuance diff FILE [REV | A..B | OTHER_FILE] [--staged]`, or Compare menu.").weak());
}

/// egui's default proportional font (Ubuntu Light) lacks arrows and box-drawing characters such
/// as `→` `─` `┄`; fall back to the bundled monospace font (Hack), which has them.
fn install_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn sidecar_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".layout.json");
    PathBuf::from(s)
}

fn read_sidecar(p: &Path) -> Option<SavedLayout> {
    serde_json::from_str(&std::fs::read_to_string(sidecar_path(p)).ok()?).ok()
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}


#[cfg(test)]
mod screenshots {
    use super::*;
    use egui::pos2;
    use egui_kittest::Harness;

    /// Renders the example in several navigation states to `target/screenshots/`.
    /// Run with `cargo test screenshots -- --ignored`.
    #[test]
    #[ignore = "needs a GPU; writes PNGs for manual inspection"]
    fn render_states() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = Harness::builder()
            .with_size(vec2(1480.0, 920.0))
            .wgpu()
            .build_eframe(|cc| {
                let mut app = ErApp::new(cc, Launch::View(None));
                app.pending = Some(Source::Example);
                app
            });
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(90);
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        shot(&mut h, "1-overview");

        let orders = h.state().scene.as_ref().unwrap().grouping.groups.iter().position(|g| g.name == "Orders").unwrap();
        h.state_mut().selected = Some(NodeRef::Group(orders));
        h.key_press(Key::Enter);
        shot(&mut h, "2-orders-expanded");

        h.key_press(Key::F);
        h.key_press(Key::ArrowRight);
        shot(&mut h, "3-fields-and-nav");

        h.key_press(Key::C);
        shot(&mut h, "4-collapsed-again");

        h.key_press_modifiers(Modifiers::SHIFT, Key::G);
        shot(&mut h, "5-all-expanded");
    }

    #[test]
    #[ignore = "needs a GPU; writes PNGs for manual inspection"]
    fn render_descriptions() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        // Work on a copy so the layout sidecar does not land in examples/.
        let file = dir.join("commerce.mmd");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/commerce.mmd"), &file).unwrap();
        let _ = std::fs::remove_file(sidecar_path(&file));
        let mut h = Harness::builder().with_size(vec2(1480.0, 920.0)).wgpu().build_eframe(|cc| {
            let mut app = ErApp::new(cc, Launch::View(None));
            app.pending = Some(Source::File(file.clone()));
            app
        });
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(90);
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        let g = h.state().scene.as_ref().unwrap().grouping.groups.iter().position(|g| g.name == "Orders").unwrap();
        h.state_mut().selected = Some(NodeRef::Group(g));
        h.key_press(Key::Enter);
        h.key_press(Key::D);
        shot(&mut h, "6-descriptions");
        h.key_press(Key::Num0);
        shot(&mut h, "7-descriptions-fit");
        // ORDER's description contains `→`, which the default proportional font lacks.
        let e = h.state().scene.as_ref().unwrap().diagram.entity_index("ORDER").unwrap();
        let ctx = h.ctx.clone();
        h.state_mut().apply(&ctx, Action::Reveal(e), 0.0);
        shot(&mut h, "9-arrows-in-description");
        let e = h.state().scene.as_ref().unwrap().diagram.entity_index("PAYMENT").unwrap();
        h.state_mut().apply(&ctx, Action::Reveal(e), 0.0);
        h.state_mut().apply(&ctx, Action::Toggle(NodeRef::Entity(e)), 0.0);
        shot(&mut h, "10-enum-options");
        // Hover a short field first, then a long one: the second tooltip must not keep the
        // first one's width.
        let field_pos = |h: &Harness<'_, ErApp>, i: usize| {
            let app = h.state();
            let scene = app.scene.as_ref().unwrap();
            let r = scene.entity_rect(e, Which::Shown);
            let rows: f32 = scene.ents[e].geom.rows[..i].iter().sum();
            let y = r.min.y + crate::scene::HEADER_H + scene.desc_block_h(e) + 3.0 + rows + 6.0;
            app.cam.to_screen(app.canvas, pos2(r.center().x, y))
        };
        let p = field_pos(&h, 0);
        h.hover_at(p);
        h.run_steps(5);
        let p = field_pos(&h, 5);
        h.hover_at(p);
        shot(&mut h, "11-tooltip-after-short-one");
        for _ in 0..3 {
            h.key_press(Key::Minus);
        }
        shot(&mut h, "8-descriptions-hidden-when-far");
        assert!(!h.state().scene.as_ref().unwrap().show_descriptions);
    }

    /// Writes `old.mmd` (the example) and `new.mmd` (a few changes) into `dir`.
    fn diff_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let old = EXAMPLE.to_owned();
    // New version: CART_ITEM dropped, GIFT_CARD added, ORDER's fields and CART–ORDER changed.
    let start = old.find("    CART_ITEM {").unwrap();
    let end = start + old[start..].find("    }\n").unwrap() + "    }\n".len();
    let mut new = format!("{}{}", &old[..start], &old[end..]);
    new = new.replace("CART, CART_ITEM, ORDER", "CART, ORDER, GIFT_CARD");
    new = new.lines().filter(|l| !l.contains("CART_ITEM")).collect::<Vec<_>>().join("\n");
    new = new
        .replace("        char(3) currency\n        decimal total\n", "        string currency FK\n")
        .replacen("        timestamp placed_at\n    }", "        timestamp placed_at\n        string notes\n    }", 1)
        .replace("CART |o..o| ORDER", "CART |o--|| ORDER")
        .replace("ORDER ||--o{ PAYMENT : \"paid by\"", "ORDER ||--o{ PAYMENT : \"paid by\"\n    ORDER ||--o{ GIFT_CARD : \"paid with\"");
    let (old_p, new_p) = (dir.join("old.mmd"), dir.join("new.mmd"));
    std::fs::write(&old_p, old).unwrap();
    std::fs::write(&new_p, new).unwrap();
    let _ = std::fs::remove_file(sidecar_path(&new_p));
        (old_p, new_p)
    }

    /// Renders the README screenshots to `docs/screenshots/`.
    /// Run with `cargo test readme_screenshots -- --ignored`.
    #[test]
    #[ignore = "needs a GPU; writes the README screenshots"]
    fn readme_screenshots() {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots/readme");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let size = vec2(1480.0, 920.0);
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(120);
            h.render().unwrap().save(out.join(format!("{name}.png"))).unwrap();
        };
        let entity = |h: &Harness<'_, ErApp>, id: &str| h.state().scene.as_ref().unwrap().diagram.entity_index(id).unwrap();

        // Navigator: Orders expanded, ORDER's fields and descriptions shown, a tag highlighted.
        let file = dir.join("commerce.mmd");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/commerce.mmd"), &file).unwrap();
        let _ = std::fs::remove_file(sidecar_path(&file));
        let mut h = Harness::builder().with_size(size).wgpu().build_eframe(|cc| ErApp::new(cc, Launch::View(Some(file.clone()))));
        h.run_steps(30);
        let ctx = h.ctx.clone();
        let order = entity(&h, "ORDER");
        h.state_mut().apply(&ctx, Action::Reveal(order), 0.0);
        h.state_mut().apply(&ctx, Action::Toggle(NodeRef::Entity(order)), 0.0);
        h.state_mut().apply(&ctx, Action::ToggleDescriptions, 0.0);
        let t = h.state().scene.as_ref().unwrap().diagram.tag_index("append-only").unwrap();
        h.state_mut().apply(&ctx, Action::ToggleTag(t), 0.0);
        h.state_mut().apply(&ctx, Action::Center(NodeRef::Entity(order)), 0.0);
        shot(&mut h, "navigator");

        // Diff: the first change (ORDER) with its fields.
        let (old_p, new_p) = diff_fixture(&dir);
        let mut h = Harness::builder().with_size(size).wgpu().build_eframe(|cc| {
            ErApp::new(cc, Launch::Diff { old: Side::File(old_p.clone()), new: Side::File(new_p.clone()) })
        });
        h.run_steps(30);
        h.key_press(Key::N);
        h.key_press(Key::F);
        shot(&mut h, "diff");
    }

    #[test]
    #[ignore = "needs a GPU; writes PNGs for manual inspection"]
    fn render_diff() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots/diff");
        std::fs::create_dir_all(&dir).unwrap();
        let (old_p, new_p) = diff_fixture(&dir);

        let mut h = Harness::builder().with_size(vec2(1480.0, 920.0)).wgpu().build_eframe(|cc| {
            ErApp::new(cc, Launch::Diff { old: Side::File(old_p.clone()), new: Side::File(new_p.clone()) })
        });
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(90);
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        shot(&mut h, "1-overview");
        let d = h.state().scene.as_ref().unwrap().diff.clone().unwrap();
        assert_eq!(d.entity_counts(), diff::Counts { added: 1, removed: 1, changed: 1 });
        assert!(h.state().status.contains("old.mmd → new.mmd"), "{}", h.state().status);

        h.key_press(Key::N);
        shot(&mut h, "2-first-change");
        h.key_press(Key::F);
        shot(&mut h, "3-first-change-fields");
        h.key_press(Key::N);
        h.key_press(Key::F);
        shot(&mut h, "4-next-change-fields");
        assert!(!sidecar_path(&new_p).exists(), "diff mode never writes a layout");

        // A long diff: the main entities renamed. Changes and tree scroll separately.
        let ctx = h.ctx.clone();
        let renamed = ["ORDER", "PRODUCT", "CUSTOMER", "PAYMENT", "STOCK"].iter().fold(EXAMPLE.to_owned(), |s, w| s.replace(w, &format!("V2_{w}")));
        std::fs::write(dir.join("renamed.mmd"), renamed).unwrap();
        let long = Source::Diff { old: Side::File(old_p.clone()), new: Side::File(dir.join("renamed.mmd")) };
        h.state_mut().load(&ctx, long);
        h.state_mut().apply(&ctx, Action::AllGroups(true), 0.0);
        shot(&mut h, "5-long-diff");
        h.hover_at(egui::pos2(120.0, 150.0));
        for _ in 0..10 {
            h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -3.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move });
            h.run_steps(2);
        }
        shot(&mut h, "6-long-diff-changes-scrolled");
    }

    #[test]
    #[ignore = "needs a GPU; writes PNGs for manual inspection"]
    fn render_tags() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots/tags");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("commerce.mmd");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/commerce.mmd"), &file).unwrap();
        let _ = std::fs::remove_file(sidecar_path(&file));
        let mut h = Harness::builder().with_size(vec2(1480.0, 920.0)).wgpu().build_eframe(|cc| {
            ErApp::new(cc, Launch::View(Some(file.clone())))
        });
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(90);
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        shot(&mut h, "1-overview-dots");
        let ctx = h.ctx.clone();
        let tag = |h: &Harness<'_, ErApp>, name: &str| h.state().scene.as_ref().unwrap().diagram.tag_index(name).unwrap();
        let derived = tag(&h, "derived");
        h.state_mut().apply(&ctx, Action::FocusTag(derived), 0.0);
        shot(&mut h, "2-focus-derived");
        let app = h.state();
        let scene = app.scene.as_ref().unwrap();
        for (g, group) in scene.grouping.groups.iter().enumerate() {
            let has = group.members.iter().any(|&e| scene.diagram.entities[e].tags.iter().any(|t| t == "derived"));
            assert_eq!(scene.grps[g].expanded, has, "{}", group.name);
        }
        assert!(app.tag_highlight.contains("derived"));
        let other = tag(&h, "append-only");
        h.state_mut().apply(&ctx, Action::ToggleTag(other), 0.0);
        h.state_mut().apply(&ctx, Action::ToggleTag(derived), 0.0);
        shot(&mut h, "3-highlight-append-only");
        assert_eq!(h.state().tag_highlight.iter().collect::<Vec<_>>(), ["append-only"]);
    }

    #[test]
    fn git_diff_source_reads_both_sides() {
        let f = crate::vcs::tests::Fixture::new();
        f.commit("s.mmd", "erDiagram\nA ||--o{ B : x\n", "one");
        f.write("s.mmd", "erDiagram\nA ||--o{ B : x\nB ||--o{ C : y\n");
        f.write("new.mmd", "erDiagram\nN { int id }\n");
        let side = |rel: &str, rev: Rev| Side::git(&f.file(rel), rev);

        let src = Source::Diff { old: side("s.mmd", Rev::Commit("HEAD".into())), new: side("s.mmd", Rev::WorkTree) };
        let (merged, marks, title) = read_source(&src).unwrap();
        assert_eq!(merged.entities.len(), 3);
        assert_eq!(marks.unwrap().entity_counts(), diff::Counts { added: 1, removed: 0, changed: 0 });
        assert!(title.starts_with("diff: HEAD → working tree"), "{title}");
        assert_eq!(src.live_paths(), [f.file("s.mmd").disk_path()], "only the work tree side is polled");

        // Not committed yet: the old side is empty.
        let src = Source::Diff { old: side("new.mmd", Rev::Commit("HEAD".into())), new: side("new.mmd", Rev::WorkTree) };
        let (_, marks, _) = read_source(&src).unwrap();
        assert_eq!(marks.unwrap().entity_counts(), diff::Counts { added: 1, removed: 0, changed: 0 });
    }

    #[test]
    fn proportional_font_covers_used_symbols() {
        let ctx = Context::default();
        install_fonts(&ctx);
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let used: String = [include_str!("../examples/commerce.mmd"), include_str!("app.rs"), include_str!("canvas.rs"), include_str!("diff.rs")]
            .concat()
            .chars()
            .filter(|c| !c.is_ascii())
            // Drawn from egui's emoji font, which `has_glyph` does not report.
            .filter(|&c| c != '⚠')
            .collect();
        let id = egui::FontId::proportional(12.0);
        let missing: String = ctx.fonts_mut(|f| used.chars().filter(|&c| !f.has_glyph(&id, c)).collect());
        assert!(missing.is_empty(), "missing glyphs: {missing:?}");
    }

    #[test]
    #[ignore = "needs a GPU; writes PNGs for manual inspection"]
    fn command_palette() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = Harness::builder().with_size(vec2(1480.0, 920.0)).wgpu().build_eframe(|cc| {
            let mut app = ErApp::new(cc, Launch::View(None));
            app.pending = Some(Source::Example);
            app
        });
        let shot = |h: &mut Harness<'_, ErApp>, name: &str| {
            h.run_steps(30);
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        };
        let type_text = |h: &mut Harness<'_, ErApp>, t: &str| {
            for c in t.chars() {
                h.event(Event::Text(c.to_string()));
                h.run_steps(1);
            }
        };
        h.run_steps(30);
        let order = h.state().scene.as_ref().unwrap().grouping.groups.iter().position(|g| g.name == "Orders").unwrap();
        h.state_mut().selected = Some(NodeRef::Group(order));

        h.key_press_modifiers(Modifiers::COMMAND, Key::K);
        shot(&mut h, "12-palette");
        assert!(h.state().palette.is_some());

        type_text(&mut h, "sf");
        shot(&mut h, "13-palette-filtered");
        h.key_press(Key::Space);
        h.run_steps(3);
        assert!(!h.state().scene.as_ref().unwrap().fields_visible, "Space toggles the flag");
        assert!(h.state().palette.is_some(), "and keeps the palette open");
        shot(&mut h, "14-palette-flag-toggled");

        h.key_press(Key::Escape);
        h.run_steps(3);
        assert!(h.state().palette.is_none());

        h.key_press_modifiers(Modifiers::COMMAND, Key::K);
        h.run_steps(2);
        type_text(&mut h, "order line");
        shot(&mut h, "15-palette-goto");
        h.key_press(Key::Enter);
        shot(&mut h, "16-after-goto");
        let app = h.state();
        let scene = app.scene.as_ref().unwrap();
        assert_eq!(app.selected, scene.diagram.entity_index("ORDER_LINE").map(NodeRef::Entity));
        assert!(app.palette.is_none());

        let center_before = app.cam.to_world(app.canvas, app.canvas.center());
        let width_before = app.canvas.width();
        h.key_press(Key::P);
        shot(&mut h, "17-panels-hidden");
        let app = h.state();
        assert!(app.panels_hidden);
        assert!(app.canvas.width() > width_before + 400.0, "canvas takes the panels' space");
        let center_after = app.cam.to_world(app.canvas, app.canvas.center());
        assert!((center_after - center_before).length() < 1.0, "view stays centered");
    }
}
