//! Navigable view state: what is expanded, where everything sits (in world units), which
//! relationships are visible and between which visible nodes.
//!
//! Two levels of nodes: *top-level* nodes (groups and ungrouped entities) have absolute
//! positions; grouped entities have positions *relative to their group*, so moving a group
//! moves its members and expanding it reveals them around the group's position.

use std::collections::{BTreeMap, HashMap};

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::diff::{self, DiffMarks, EntityDiff, Status};
use crate::grouping::{self, Grouping, GroupingMode};
use crate::layout::{self, place_new, remove_overlaps};
use crate::model::Diagram;

pub const HEADER_H: f32 = 28.0;
pub const ROW_H: f32 = 18.0;
pub const PAD: f32 = 10.0;
pub const COL_GAP: f32 = 12.0;
pub const GROUP_HEADER_H: f32 = 32.0;
pub const GROUP_PAD: f32 = 18.0;
pub const GROUP_SUB_H: f32 = 18.0;
pub const GROUP_LIST_ROW: f32 = 16.0;
pub const GROUP_LIST_MAX: usize = 8;
/// Tag chips in entity headers: text padding, gap between chips, gap after the name.
pub const TAG_PAD: f32 = 5.0;
pub const TAG_GAP: f32 = 4.0;
pub const TAG_AFTER_NAME: f32 = 8.0;
/// Spacing of the tag dots in collapsed group headers.
pub const TAG_DOT: f32 = 10.0;
/// Width of entity boxes while descriptions are shown.
pub const DESC_W: f32 = 300.0;
pub const DESC_PAD: f32 = 7.0;
const TOP_GAP: f32 = 50.0;
const INNER_GAP: f32 = 46.0;

#[derive(Clone, Copy, Debug)]
pub enum Font {
    /// Entity name (proportional).
    Name,
    /// Attribute rows (monospace).
    Field,
    /// Group title (proportional).
    Title,
    /// Small annotations (proportional).
    Small,
    /// Enum options under an attribute (monospace).
    Options,
}

impl Font {
    pub fn size(self) -> f32 {
        match self {
            Self::Name => 14.0,
            Self::Field => 12.0,
            Self::Title => 16.0,
            Self::Small => 11.5,
            Self::Options => 10.5,
        }
    }

    pub fn monospace(self) -> bool {
        matches!(self, Self::Field | Self::Options)
    }
}

/// Minimum box width when a field lists enum options, so they wrap into a few lines, not many.
const OPTIONS_MIN_W: f32 = 280.0;
/// Space between an attribute's enum options and the next row.
const OPTIONS_GAP: f32 = 4.0;

/// Text width in world units at zoom 1.
pub trait Measure {
    fn width(&self, text: &str, font: Font) -> f32;
    /// Height of a description (see [`crate::richtext`]) in [`Font::Small`], wrapped at `wrap`.
    fn rich_height(&self, text: &str, wrap: f32) -> f32;
    /// Height of plain text in `font`, wrapped at `wrap`.
    fn wrapped_height(&self, text: &str, font: Font, wrap: f32) -> f32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeRef {
    Entity(usize),
    Group(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// Where things are heading (layout decisions).
    Target,
    /// Where things are drawn this frame (animated).
    Shown,
}

#[derive(Clone, Debug, Default)]
pub struct EntityGeom {
    pub collapsed: Vec2,
    pub expanded: Vec2,
    pub type_w: f32,
    pub name_w: f32,
    /// Width of the entity's name in the header.
    pub title_w: f32,
    /// Width of each tag chip in the header.
    pub tag_w: Vec<f32>,
    /// Height of the description block (0 without description).
    pub desc_h: f32,
    /// Height of each attribute row; taller when enum options are listed below it.
    pub rows: Vec<f32>,
    /// Wrap width of enum options.
    pub options_w: f32,
}

#[derive(Clone, Debug)]
pub struct EntState {
    pub show_fields: bool,
    pub pos: Pos2,
    pub shown: Pos2,
    pub geom: EntityGeom,
}

#[derive(Clone, Debug)]
pub struct GrpState {
    pub expanded: bool,
    pub pos: Pos2,
    pub shown: Pos2,
    pub collapsed: Vec2,
    pub title_w: f32,
    pub hue: f32,
    /// Nodes pushed aside when this group was expanded: (node, from, to).
    pushed: Vec<(NodeRef, Pos2, Pos2)>,
}

/// A visible connection between two visible nodes, carrying one or more relationships.
#[derive(Clone, Debug)]
pub struct VisEdge {
    pub a: NodeRef,
    pub b: NodeRef,
    pub rels: Vec<usize>,
}

impl VisEdge {
    pub fn touches_group(&self) -> bool {
        matches!(self.a, NodeRef::Group(_)) || matches!(self.b, NodeRef::Group(_))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub node: NodeRef,
    pub field: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedLayout {
    pub mode: Option<GroupingMode>,
    #[serde(default)]
    pub groups: BTreeMap<String, SavedNode>,
    #[serde(default)]
    pub entities: BTreeMap<String, SavedNode>,
    #[serde(default)]
    pub camera: Option<[f32; 3]>,
    /// The "show descriptions" toggle.
    #[serde(default)]
    pub descriptions: bool,
    /// The "show fields" master switch is off.
    #[serde(default)]
    pub fields_hidden: bool,
    /// Side panels hidden (an app setting, stored here so it is remembered per file).
    #[serde(default)]
    pub panels_hidden: bool,
    /// Highlighted tags (an app setting, like `panels_hidden`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedNode {
    /// Group: expanded. Entity: fields shown.
    pub open: bool,
    pub x: f32,
    pub y: f32,
}

pub struct Scene {
    pub diagram: Diagram,
    pub grouping: Grouping,
    pub mode: GroupingMode,
    pub ents: Vec<EntState>,
    pub grps: Vec<GrpState>,
    /// Descriptions are drawn inside entity boxes (and take up layout space).
    pub show_descriptions: bool,
    /// Master switch for fields. When off, no entity shows fields; each entity's own
    /// `show_fields` is kept and applies again when it is switched back on.
    pub fields_visible: bool,
    /// In diff mode: what changed, by index into `diagram`.
    pub diff: Option<DiffMarks>,
}

impl Scene {
    #[cfg(test)]
    pub fn new(
        diagram: Diagram,
        mode: GroupingMode,
        saved: Option<&SavedLayout>,
        m: &dyn Measure,
    ) -> Self {
        Self::with_diff(diagram, None, mode, saved, m)
    }

    /// A scene showing a merged diagram (see [`crate::diff`]) with its changes marked.
    pub fn with_diff(
        diagram: Diagram,
        diff: Option<DiffMarks>,
        mode: GroupingMode,
        saved: Option<&SavedLayout>,
        m: &dyn Measure,
    ) -> Self {
        let grouping = grouping::compute(&diagram, mode);
        let saved = saved.filter(|s| s.mode == Some(mode));

        let ents = diagram
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let geom = entity_geom(e, diff.as_ref().map(|d| &d.entities[i]), m);
                let s = saved.and_then(|s| s.entities.get(&e.id));
                EntState {
                    show_fields: s.is_some_and(|s| s.open),
                    pos: Pos2::ZERO,
                    shown: Pos2::ZERO,
                    geom,
                }
            })
            .collect();
        let grps = grouping
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let title_w = m.width(&g.name, Font::Title);
                let list_w = g
                    .members
                    .iter()
                    .take(GROUP_LIST_MAX)
                    .map(|&e| m.width(diagram.entities[e].display_name(), Font::Small))
                    .fold(0.0, f32::max);
                let lines = g.members.len().min(GROUP_LIST_MAX)
                    + usize::from(g.members.len() > GROUP_LIST_MAX);
                let badge_w = match diff.as_ref().map(|d| d.counts_of(&g.members)) {
                    Some(c) if !c.is_empty() => m.width(&c.badge(), Font::Small) + 18.0,
                    _ => 0.0,
                };
                let dots_w = match group_tags(&diagram, &g.members).len() {
                    0 => 0.0,
                    n => n as f32 * TAG_DOT + 6.0,
                };
                let collapsed = vec2(
                    (2.0 * PAD + (title_w + 22.0 + dots_w + badge_w).max(list_w + 10.0)).max(170.0),
                    GROUP_HEADER_H + GROUP_SUB_H + lines as f32 * GROUP_LIST_ROW + PAD,
                );
                GrpState {
                    expanded: saved
                        .and_then(|s| s.groups.get(&g.name))
                        .is_some_and(|s| s.open),
                    pos: Pos2::ZERO,
                    shown: Pos2::ZERO,
                    collapsed,
                    title_w,
                    hue: (i as f32 * 0.618_034).fract(),
                    pushed: Vec::new(),
                }
            })
            .collect();

        let show_descriptions = saved.is_some_and(|s| s.descriptions);
        let fields_visible = !saved.is_some_and(|s| s.fields_hidden);
        let mut scene = Self {
            diagram,
            grouping,
            mode,
            ents,
            grps,
            show_descriptions,
            fields_visible,
            diff,
        };
        scene.place(saved);
        scene
    }

    /// Lays out everything without a saved position (or everything, if `saved` is `None`).
    fn place(&mut self, saved: Option<&SavedLayout>) {
        for g in 0..self.grps.len() {
            let members = self.grouping.groups[g].members.clone();
            let known: Vec<Option<Pos2>> = members
                .iter()
                .map(|&e| {
                    saved
                        .and_then(|s| s.entities.get(&self.diagram.entities[e].id))
                        .map(|s| pos2(s.x, s.y))
                })
                .collect();
            let sizes: Vec<Vec2> = members.iter().map(|&e| self.entity_size(e)).collect();
            let local: HashMap<usize, usize> =
                members.iter().enumerate().map(|(i, &e)| (e, i)).collect();
            let edges: Vec<(usize, usize)> = self
                .diagram
                .relationships
                .iter()
                .filter_map(|r| Some((*local.get(&r.a)?, *local.get(&r.b)?)))
                .collect();
            let mut pos = place_new(&known, &sizes, &edges, INNER_GAP);
            let free = vec![false; pos.len()];
            remove_overlaps(&mut pos, &sizes, &free, INNER_GAP);
            for (i, &e) in members.iter().enumerate() {
                self.ents[e].pos = pos[i];
            }
        }

        let nodes = self.top_nodes();
        let index: HashMap<NodeRef, usize> =
            nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let mut known = Vec::with_capacity(nodes.len());
        let mut sizes = Vec::with_capacity(nodes.len());
        let mut offsets = Vec::with_capacity(nodes.len());
        for &n in &nodes {
            let saved_pos = saved.and_then(|s| match n {
                NodeRef::Group(g) => s.groups.get(&self.grouping.groups[g].name),
                NodeRef::Entity(e) => s.entities.get(&self.diagram.entities[e].id),
            });
            self.set_pos(n, Pos2::ZERO);
            let r = self.node_rect(n, Which::Target);
            offsets.push(r.center().to_vec2());
            sizes.push(r.size());
            known.push(saved_pos.map(|s| pos2(s.x, s.y) + r.center().to_vec2()));
        }
        let mut edges: Vec<(usize, usize)> = self
            .diagram
            .relationships
            .iter()
            .map(|r| (index[&self.top_node_of(r.a)], index[&self.top_node_of(r.b)]))
            .filter(|(a, b)| a != b)
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let centers = place_new(&known, &sizes, &edges, TOP_GAP);
        for (i, &n) in nodes.iter().enumerate() {
            self.set_pos(n, centers[i] - offsets[i]);
        }
        self.snap();
    }

    /// Forgets all positions and lays everything out again.
    pub fn relayout(&mut self) {
        for g in &mut self.grps {
            g.pushed.clear();
        }
        self.place(None);
        self.resolve_top(None);
    }

    pub fn snap(&mut self) {
        for e in &mut self.ents {
            e.shown = e.pos;
        }
        for g in &mut self.grps {
            g.shown = g.pos;
        }
    }

    /// Advances animations; returns `true` while something is still moving.
    pub fn animate(&mut self, dt: f32) -> bool {
        let t = 1.0 - (-dt * 14.0).exp();
        let mut moving = false;
        let mut step = |shown: &mut Pos2, pos: Pos2| {
            let d = pos - *shown;
            if d.length_sq() < 0.05 {
                *shown = pos;
            } else {
                *shown += d * t;
                moving = true;
            }
        };
        for e in &mut self.ents {
            step(&mut e.shown, e.pos);
        }
        for g in &mut self.grps {
            step(&mut g.shown, g.pos);
        }
        moving
    }

    // ---- structure ---------------------------------------------------------------------------

    /// Diff status of entity `e` (`Same` outside diff mode).
    pub fn entity_status(&self, e: usize) -> Status {
        self.diff
            .as_ref()
            .map_or(Status::Same, |d| d.entities[e].status)
    }

    /// Diff status of relationship `r` (`Same` outside diff mode).
    pub fn rel_status(&self, r: usize) -> Status {
        self.diff
            .as_ref()
            .map_or(Status::Same, |d| d.relationships[r].status)
    }

    pub fn group_of(&self, e: usize) -> Option<usize> {
        self.grouping.group_of[e]
    }

    pub fn top_nodes(&self) -> Vec<NodeRef> {
        (0..self.grps.len())
            .map(NodeRef::Group)
            .chain(
                (0..self.ents.len())
                    .filter(|&e| self.group_of(e).is_none())
                    .map(NodeRef::Entity),
            )
            .collect()
    }

    pub fn top_node_of(&self, e: usize) -> NodeRef {
        self.group_of(e).map_or(NodeRef::Entity(e), NodeRef::Group)
    }

    /// The node an entity is currently represented by.
    pub fn visible_node(&self, e: usize) -> NodeRef {
        match self.group_of(e) {
            Some(g) if !self.grps[g].expanded => NodeRef::Group(g),
            _ => NodeRef::Entity(e),
        }
    }

    pub fn is_visible(&self, n: NodeRef) -> bool {
        match n {
            NodeRef::Group(_) => true,
            NodeRef::Entity(e) => self.visible_node(e) == n,
        }
    }

    /// Visible nodes in back-to-front paint order.
    pub fn paint_order(&self) -> Vec<NodeRef> {
        let mut v: Vec<NodeRef> = (0..self.grps.len())
            .filter(|&g| self.grps[g].expanded)
            .map(NodeRef::Group)
            .collect();
        v.extend(
            (0..self.grps.len())
                .filter(|&g| !self.grps[g].expanded)
                .map(NodeRef::Group),
        );
        v.extend(
            (0..self.ents.len())
                .filter(|&e| self.is_visible(NodeRef::Entity(e)))
                .map(NodeRef::Entity),
        );
        v
    }

    pub fn visible_edges(&self) -> Vec<VisEdge> {
        let mut out: Vec<VisEdge> = Vec::new();
        let mut index: HashMap<(NodeRef, NodeRef), usize> = HashMap::new();
        for (ri, r) in self.diagram.relationships.iter().enumerate() {
            let (a, b) = (self.visible_node(r.a), self.visible_node(r.b));
            if a == b && matches!(a, NodeRef::Group(_)) {
                continue;
            }
            let key = (a.min(b), a.max(b));
            match index.get(&key) {
                Some(&i) => out[i].rels.push(ri),
                None => {
                    index.insert(key, out.len());
                    out.push(VisEdge {
                        a,
                        b,
                        rels: vec![ri],
                    });
                }
            }
        }
        out
    }

    /// Relationships hidden inside a collapsed group.
    pub fn internal_relationships(&self, g: usize) -> usize {
        self.diagram
            .relationships
            .iter()
            .filter(|r| self.group_of(r.a) == Some(g) && self.group_of(r.b) == Some(g))
            .count()
    }

    /// Entities directly related to `n` (any member, for groups), excluding `n`'s own members.
    pub fn neighbours(&self, n: NodeRef) -> Vec<usize> {
        let inside = |e: usize| match n {
            NodeRef::Entity(x) => x == e,
            NodeRef::Group(g) => self.group_of(e) == Some(g),
        };
        let mut v: Vec<usize> = self
            .diagram
            .relationships
            .iter()
            .filter_map(|r| match (inside(r.a), inside(r.b)) {
                (true, false) => Some(r.b),
                (false, true) => Some(r.a),
                _ => None,
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    // ---- geometry ----------------------------------------------------------------------------

    pub fn entity_size(&self, e: usize) -> Vec2 {
        let s = &self.ents[e];
        let base = if self.fields_shown(e) {
            s.geom.expanded
        } else {
            s.geom.collapsed
        };
        match self.desc_block_h(e) {
            0.0 => base,
            h => vec2(base.x.max(DESC_W), base.y + h),
        }
    }

    /// Whether entity `e` currently shows its fields (its own state and the master switch).
    pub fn fields_shown(&self, e: usize) -> bool {
        self.fields_visible && self.ents[e].show_fields
    }

    /// The "show fields" master switch.
    pub fn set_fields_visible(&mut self, visible: bool) {
        if self.fields_visible == visible {
            return;
        }
        self.fields_visible = visible;
        for g in 0..self.grps.len() {
            self.resolve_inner(g, None);
        }
        self.resolve_top(None);
    }

    /// Height of the description block currently shown in entity `e`.
    pub fn desc_block_h(&self, e: usize) -> f32 {
        if self.show_descriptions {
            self.ents[e].geom.desc_h
        } else {
            0.0
        }
    }

    pub fn has_descriptions(&self) -> bool {
        self.ents.iter().any(|e| e.geom.desc_h > 0.0)
    }

    /// Shows or hides descriptions inside entity boxes, making room as needed.
    pub fn set_descriptions(&mut self, show: bool) {
        if self.show_descriptions == show {
            return;
        }
        self.show_descriptions = show;
        for g in 0..self.grps.len() {
            self.resolve_inner(g, None);
        }
        self.resolve_top(None);
    }

    fn entity_pos(&self, e: usize, w: Which) -> Pos2 {
        let s = &self.ents[e];
        let p = if w == Which::Target { s.pos } else { s.shown };
        match self.group_of(e) {
            Some(g) => self.group_pos(g, w) + p.to_vec2(),
            None => p,
        }
    }

    fn group_pos(&self, g: usize, w: Which) -> Pos2 {
        if w == Which::Target {
            self.grps[g].pos
        } else {
            self.grps[g].shown
        }
    }

    pub fn entity_rect(&self, e: usize, w: Which) -> Rect {
        Rect::from_center_size(self.entity_pos(e, w), self.entity_size(e))
    }

    pub fn group_rect(&self, g: usize, w: Which) -> Rect {
        let st = &self.grps[g];
        if !st.expanded {
            return Rect::from_center_size(self.group_pos(g, w), st.collapsed);
        }
        let mut r = Rect::NOTHING;
        for &e in &self.grouping.groups[g].members {
            r = r.union(self.entity_rect(e, w));
        }
        let mut r = r.expand(GROUP_PAD);
        r.min.y -= GROUP_HEADER_H - GROUP_PAD * 0.5;
        let min_w = st.title_w + 2.0 * PAD + 90.0;
        if r.width() < min_w {
            r = Rect::from_center_size(r.center(), vec2(min_w, r.height()));
        }
        r
    }

    pub fn node_rect(&self, n: NodeRef, w: Which) -> Rect {
        match n {
            NodeRef::Entity(e) => self.entity_rect(e, w),
            NodeRef::Group(g) => self.group_rect(g, w),
        }
    }

    pub fn bounds(&self, w: Which) -> Rect {
        self.top_nodes()
            .into_iter()
            .fold(Rect::NOTHING, |r, n| r.union(self.node_rect(n, w)))
    }

    /// Topmost node under a world-space point.
    pub fn hit_test(&self, p: Pos2) -> Option<Hit> {
        for &n in self.paint_order().iter().rev() {
            let r = self.node_rect(n, Which::Shown);
            if !r.contains(p) {
                continue;
            }
            let field = match n {
                NodeRef::Entity(e) if self.fields_shown(e) => {
                    let mut top = r.min.y + HEADER_H + self.desc_block_h(e) + 3.0;
                    self.ents[e].geom.rows.iter().position(|h| {
                        top += h;
                        p.y >= top - h && p.y < top
                    })
                }
                _ => None,
            };
            return Some(Hit { node: n, field });
        }
        None
    }

    // ---- mutation ----------------------------------------------------------------------------

    fn set_pos(&mut self, n: NodeRef, p: Pos2) {
        match n {
            NodeRef::Entity(e) => self.ents[e].pos = p,
            NodeRef::Group(g) => self.grps[g].pos = p,
        }
    }

    fn pos_of(&self, n: NodeRef) -> Pos2 {
        match n {
            NodeRef::Entity(e) => self.ents[e].pos,
            NodeRef::Group(g) => self.grps[g].pos,
        }
    }

    /// Moves a node immediately (no animation), e.g. while dragging.
    pub fn drag(&mut self, n: NodeRef, delta: Vec2) {
        match n {
            NodeRef::Entity(e) => {
                self.ents[e].pos += delta;
                self.ents[e].shown += delta;
            }
            NodeRef::Group(g) => {
                self.grps[g].pos += delta;
                self.grps[g].shown += delta;
            }
        }
    }

    /// Call after a drag ends: make room around the dragged node.
    pub fn drag_finished(&mut self, n: NodeRef) {
        if let NodeRef::Entity(e) = n
            && let Some(g) = self.group_of(e)
        {
            self.resolve_inner(g, Some(e));
            self.resolve_top(Some(NodeRef::Group(g)));
        } else {
            self.resolve_top(Some(n));
        }
    }

    pub fn set_group_expanded(&mut self, g: usize, expanded: bool) {
        if self.grps[g].expanded == expanded {
            return;
        }
        self.grps[g].expanded = expanded;
        if expanded {
            // Members grow out of the group's center.
            for &e in &self.grouping.groups[g].members {
                self.ents[e].shown = Pos2::ZERO;
            }
            let pushed = self.resolve_top(Some(NodeRef::Group(g)));
            self.grps[g].pushed = pushed;
        } else {
            // Put back whatever was pushed aside and has not been moved since.
            for (n, from, to) in std::mem::take(&mut self.grps[g].pushed) {
                if (self.pos_of(n) - to).length() < 1.0 {
                    self.set_pos(n, from);
                }
            }
            self.resolve_top(Some(NodeRef::Group(g)));
        }
    }

    /// Expands exactly the groups for which `open` is true, collapsing the others.
    pub fn set_groups_expanded(&mut self, open: &[bool]) {
        for (g, &o) in self.grps.iter_mut().zip(open) {
            g.pushed.clear();
            g.expanded = o;
        }
        self.relayout_top_keep_inner();
    }

    pub fn set_all_groups_expanded(&mut self, expanded: bool) {
        for g in &mut self.grps {
            g.pushed.clear();
            g.expanded = expanded;
        }
        self.relayout_top_keep_inner();
    }

    pub fn set_fields(&mut self, entities: &[usize], show: bool) {
        let mut groups: Vec<usize> = Vec::new();
        let mut loose = None;
        for &e in entities {
            self.ents[e].show_fields = show;
            match self.group_of(e) {
                Some(g) => groups.push(g),
                None => loose = Some(e),
            }
        }
        groups.sort_unstable();
        groups.dedup();
        let single = (entities.len() == 1).then(|| entities[0]);
        for &g in &groups {
            self.resolve_inner(g, single);
        }
        let pin = match (single, groups.as_slice()) {
            (Some(_), [g]) => Some(NodeRef::Group(*g)),
            (Some(e), []) if loose.is_some() => Some(NodeRef::Entity(e)),
            _ => None,
        };
        self.resolve_top(pin);
    }

    /// Re-runs the top-level layout using current sizes but keeps group interiors.
    fn relayout_top_keep_inner(&mut self) {
        let nodes = self.top_nodes();
        let index: HashMap<NodeRef, usize> =
            nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let mut sizes = Vec::new();
        let mut offsets = Vec::new();
        for &n in &nodes {
            let r = self.node_rect(n, Which::Target);
            sizes.push(r.size());
            offsets.push(r.center() - self.pos_of(n));
        }
        let mut edges: Vec<(usize, usize)> = self
            .diagram
            .relationships
            .iter()
            .map(|r| (index[&self.top_node_of(r.a)], index[&self.top_node_of(r.b)]))
            .filter(|(a, b)| a != b)
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let centers = layout::force_layout(&sizes, &edges, TOP_GAP);
        for (i, &n) in nodes.iter().enumerate() {
            self.set_pos(n, centers[i] - offsets[i]);
        }
    }

    fn resolve_inner(&mut self, g: usize, pinned: Option<usize>) {
        let members = &self.grouping.groups[g].members;
        let mut pos: Vec<Pos2> = members.iter().map(|&e| self.ents[e].pos).collect();
        let sizes: Vec<Vec2> = members.iter().map(|&e| self.entity_size(e)).collect();
        let pin: Vec<bool> = members.iter().map(|&e| Some(e) == pinned).collect();
        remove_overlaps(&mut pos, &sizes, &pin, INNER_GAP);
        for (i, &e) in members.iter().enumerate() {
            self.ents[e].pos = pos[i];
        }
    }

    /// Removes top-level overlaps, keeping `pinned` in place. Returns the nodes that moved.
    fn resolve_top(&mut self, pinned: Option<NodeRef>) -> Vec<(NodeRef, Pos2, Pos2)> {
        let nodes = self.top_nodes();
        let rects: Vec<Rect> = nodes
            .iter()
            .map(|&n| self.node_rect(n, Which::Target))
            .collect();
        let mut centers: Vec<Pos2> = rects.iter().map(Rect::center).collect();
        let sizes: Vec<Vec2> = rects.iter().map(Rect::size).collect();
        let pin: Vec<bool> = nodes.iter().map(|&n| Some(n) == pinned).collect();
        remove_overlaps(&mut centers, &sizes, &pin, TOP_GAP);
        let mut moved = Vec::new();
        for (i, &n) in nodes.iter().enumerate() {
            let d = centers[i] - rects[i].center();
            if d.length_sq() > 0.01 {
                let from = self.pos_of(n);
                self.set_pos(n, from + d);
                moved.push((n, from, from + d));
            }
        }
        moved
    }

    pub fn to_saved(&self, camera: [f32; 3]) -> SavedLayout {
        SavedLayout {
            mode: Some(self.mode),
            groups: self
                .grouping
                .groups
                .iter()
                .zip(&self.grps)
                .map(|(g, s)| {
                    (
                        g.name.clone(),
                        SavedNode {
                            open: s.expanded,
                            x: s.pos.x,
                            y: s.pos.y,
                        },
                    )
                })
                .collect(),
            entities: self
                .diagram
                .entities
                .iter()
                .zip(&self.ents)
                .map(|(e, s)| {
                    (
                        e.id.clone(),
                        SavedNode {
                            open: s.show_fields,
                            x: s.pos.x,
                            y: s.pos.y,
                        },
                    )
                })
                .collect(),
            camera: Some(camera),
            descriptions: self.show_descriptions,
            fields_hidden: !self.fields_visible,
            panels_hidden: false,
            tags: Vec::new(),
        }
    }
}

/// Tag indices carried by any of `members`, in tag order.
pub fn group_tags(d: &Diagram, members: &[usize]) -> Vec<usize> {
    let mut v: Vec<usize> = members
        .iter()
        .flat_map(|&e| d.entities[e].tags.iter().filter_map(|t| d.tag_index(t)))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn entity_geom(e: &crate::model::Entity, d: Option<&EntityDiff>, m: &dyn Measure) -> EntityGeom {
    let title_w = m.width(e.display_name(), Font::Name);
    let tag_w: Vec<f32> = e
        .tags
        .iter()
        .map(|t| m.width(t, Font::Small) + 2.0 * TAG_PAD)
        .collect();
    let tags_w = if tag_w.is_empty() {
        0.0
    } else {
        TAG_AFTER_NAME + tag_w.iter().map(|w| w + TAG_GAP).sum::<f32>() - TAG_GAP
    };
    let name_w = title_w + tags_w;
    let n = e.attributes.len();
    // Right of the name: the field count, with the diff sign before it (or the sign alone).
    let sign = d.map_or("", |d| d.status.sign());
    let count_w = match (n, sign) {
        (0, "") => 0.0,
        (0, s) => m.width(s, Font::Name) + 14.0,
        (n, "") => m.width(&n.to_string(), Font::Small) + 14.0,
        (n, s) => m.width(&format!("{s} {n}"), Font::Small) + 14.0,
    };
    let collapsed = vec2((2.0 * PAD + name_w + 8.0 + count_w).max(90.0), HEADER_H);
    let desc_h = match &e.description {
        // Measured a little narrower than drawn: text is laid out per zoom level, and glyph
        // widths do not scale exactly, so it must never need more lines than reserved here.
        Some(d) => m.rich_height(d, (DESC_W - 2.0 * PAD) * 0.93) + 2.0 * DESC_PAD,
        None => 0.0,
    };
    if n == 0 {
        return EntityGeom {
            collapsed,
            expanded: collapsed,
            desc_h,
            title_w,
            tag_w,
            ..Default::default()
        };
    }
    let attr_diff = |i: usize| d.map(|d| &d.attrs[i]);
    let max = |f: &dyn Fn(usize, &crate::model::Attribute) -> String| {
        e.attributes
            .iter()
            .enumerate()
            .map(|(i, a)| m.width(&f(i, a), Font::Field))
            .fold(0.0, f32::max)
    };
    let type_w = max(&|i, a| diff::type_text(a, attr_diff(i)));
    let fname_w = max(&|_, a| a.name.clone());
    let keys_w = max(&|i, a| diff::keys_text(a, attr_diff(i)));
    let mut w = 2.0 * PAD + type_w + COL_GAP + fname_w;
    if keys_w > 0.0 {
        w += COL_GAP + keys_w;
    }
    let options: Vec<Option<String>> = e.attributes.iter().map(|a| a.enum_options()).collect();
    if options.iter().any(Option::is_some) {
        w = w.max(OPTIONS_MIN_W);
    }
    let w = w.max(collapsed.x);
    let options_w = w - 2.0 * PAD;
    let rows: Vec<f32> = options
        .iter()
        .map(|o| match o {
            // Measured narrower than drawn, like descriptions.
            Some(o) => ROW_H + m.wrapped_height(o, Font::Options, options_w * 0.93) + OPTIONS_GAP,
            None => ROW_H,
        })
        .collect();
    EntityGeom {
        collapsed,
        expanded: vec2(w, HEADER_H + rows.iter().sum::<f32>() + 6.0),
        type_w,
        name_w: fname_w,
        title_w,
        tag_w,
        desc_h,
        rows,
        options_w,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::mermaid::parse;

    pub struct CharMeasure;
    impl Measure for CharMeasure {
        fn width(&self, text: &str, font: Font) -> f32 {
            text.chars().count() as f32 * font.size() * 0.6
        }
        fn rich_height(&self, text: &str, wrap: f32) -> f32 {
            let w = self.width(&crate::richtext::plain(text), Font::Small);
            (w / wrap).ceil().max(1.0) * Font::Small.size() * 1.3
        }
        fn wrapped_height(&self, text: &str, font: Font, wrap: f32) -> f32 {
            (self.width(text, font) / wrap).ceil().max(1.0) * font.size() * 1.3
        }
    }

    const SRC: &str = "erDiagram
%% group Sales: CUSTOMER, ORDER, LINE
%% group Catalog: PRODUCT, CATEGORY
CUSTOMER ||--o{ ORDER : places
ORDER ||--|{ LINE : contains
LINE }o--|| PRODUCT : refers
PRODUCT }o--|| CATEGORY : in
CUSTOMER ||--o{ REVIEW : writes
REVIEW }o--|| PRODUCT : about
CUSTOMER { string id PK
string name }
";

    fn scene() -> Scene {
        Scene::new(parse(SRC), GroupingMode::File, None, &CharMeasure)
    }

    fn no_top_overlap(s: &Scene) -> bool {
        let rects: Vec<Rect> = s
            .top_nodes()
            .iter()
            .map(|&n| s.node_rect(n, Which::Target))
            .collect();
        rects
            .iter()
            .enumerate()
            .all(|(i, a)| rects[i + 1..].iter().all(|b| !a.intersects(*b)))
    }

    #[test]
    fn collapsed_groups_carry_external_edges() {
        let s = scene();
        let edges = s.visible_edges();
        // Sales–Catalog (LINE→PRODUCT), Sales–REVIEW, REVIEW–Catalog. Internal ones hidden.
        assert_eq!(edges.len(), 3);
        assert!(edges.iter().all(VisEdge::touches_group));
        assert_eq!(s.internal_relationships(0), 2);
        assert!(no_top_overlap(&s));
    }

    #[test]
    fn expanding_attaches_edges_to_entities_and_makes_room() {
        let mut s = scene();
        s.set_group_expanded(0, true);
        let edges = s.visible_edges();
        assert!(
            edges
                .iter()
                .any(|e| e.a == NodeRef::Entity(0) && e.b == NodeRef::Entity(1))
        );
        assert!(
            edges
                .iter()
                .any(|e| (e.a, e.b) == (NodeRef::Entity(2), NodeRef::Group(1)))
        );
        assert!(no_top_overlap(&s));
        s.set_fields(&[0], true);
        assert!(no_top_overlap(&s));
        s.set_group_expanded(0, false);
        assert!(no_top_overlap(&s));
    }

    #[test]
    fn saved_layout_round_trips() {
        let mut s = scene();
        s.set_group_expanded(1, true);
        s.drag(NodeRef::Entity(5), vec2(500.0, 0.0));
        let saved = s.to_saved([0.0, 0.0, 1.0]);
        let json = serde_json::to_string(&saved).unwrap();
        let back: SavedLayout = serde_json::from_str(&json).unwrap();
        let s2 = Scene::new(parse(SRC), GroupingMode::File, Some(&back), &CharMeasure);
        assert!(s2.grps[1].expanded);
        assert_eq!(s2.ents[5].pos, s.ents[5].pos);
        assert_eq!(s2.grps[0].pos, s.grps[0].pos);
    }

    #[test]
    fn hit_test_finds_fields() {
        let mut s = scene();
        s.set_group_expanded(0, true);
        s.set_fields(&[0], true);
        s.snap();
        let r = s.entity_rect(0, Which::Shown);
        let h = s
            .hit_test(pos2(r.center().x, r.min.y + HEADER_H + ROW_H * 1.5))
            .unwrap();
        assert_eq!(h.node, NodeRef::Entity(0));
        assert_eq!(h.field, Some(1));
        let h = s.hit_test(pos2(r.center().x, r.min.y + 2.0)).unwrap();
        assert_eq!(h.field, None);
    }

    #[test]
    fn large_ungrouped_diagram_clusters_and_lays_out() {
        // 400 entities in 25 loosely coupled modules, no group directives.
        let mut src = String::from("erDiagram\n");
        for m in 0..25 {
            for i in 0..16 {
                let e = m * 16 + i;
                if i > 0 {
                    src += &format!("E{} ||--o{{ E{} : r\n", m * 16 + (i - 1) / 2, e);
                }
                src += &format!("E{e} {{\n int id PK\n string name\n}}\n");
            }
            src += &format!("E{} }}o--|| E{} : link\n", m * 16 + 5, ((m + 1) % 25) * 16);
        }
        let d = parse(&src);
        let t = std::time::Instant::now();
        let mode = GroupingMode::default_for(&d);
        let mut s = Scene::new(d, mode, None, &CharMeasure);
        let elapsed = t.elapsed();
        assert_eq!(mode, GroupingMode::Clusters);
        assert!(
            (15..=60).contains(&s.grps.len()),
            "{} clusters",
            s.grps.len()
        );
        assert!(no_top_overlap(&s));
        s.set_all_groups_expanded(true);
        assert!(no_top_overlap(&s));
        eprintln!(
            "400 entities: {} clusters, built in {elapsed:?}",
            s.grps.len()
        );
        assert!(elapsed.as_secs_f32() < 5.0);
    }

    #[test]
    fn example_clusters_are_sensible() {
        let d = parse(include_str!("../examples/commerce.mmd"));
        let g = grouping::compute(&d, GroupingMode::Clusters);
        for grp in &g.groups {
            let names: Vec<&str> = grp
                .members
                .iter()
                .map(|&e| d.entities[e].id.as_str())
                .collect();
            eprintln!("{}: {}", grp.name, names.join(", "));
        }
        assert!(g.groups.len() >= 4);
    }

    #[test]
    fn example_parses_cleanly() {
        let d = parse(include_str!("../examples/commerce.mmd"));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.tags, ["pii", "append-only", "ledger", "derived"]);
        let g = grouping::compute(&d, GroupingMode::File);
        let loose: Vec<&str> = (0..d.entities.len())
            .filter(|&e| g.group_of[e].is_none())
            .map(|e| d.entities[e].id.as_str())
            .collect();
        assert_eq!(loose, ["CURRENCY", "COUNTRY"]);
    }

    #[test]
    fn descriptions_take_layout_space_when_shown() {
        let src = format!(
            "{SRC}%% describe CUSTOMER: A person who buys things, described at some length so it wraps.\n"
        );
        let mut s = Scene::new(parse(&src), GroupingMode::File, None, &CharMeasure);
        s.set_group_expanded(0, true);
        s.set_fields(&[0], true);
        let before = s.entity_size(0);
        s.set_descriptions(true);
        let after = s.entity_size(0);
        assert!(after.y > before.y && after.x >= DESC_W);
        assert_eq!(
            s.entity_size(1),
            s.ents[1].geom.collapsed,
            "no description, no change"
        );
        assert!(no_top_overlap(&s));
        s.snap();
        let r = s.entity_rect(0, Which::Shown);
        let y = r.min.y + HEADER_H + s.desc_block_h(0) + ROW_H * 0.5;
        assert_eq!(s.hit_test(pos2(r.center().x, y)).unwrap().field, Some(0));
    }

    #[test]
    fn enum_options_make_rows_taller_and_hit_test_follows() {
        let src = "erDiagram\nA {\n string role \"viewer | analyst | admin\"\n string note \"free text\"\n string name\n}\n";
        let mut s = Scene::new(parse(src), GroupingMode::None, None, &CharMeasure);
        s.set_fields(&[0], true);
        s.snap();
        let g = &s.ents[0].geom;
        assert!(g.rows[0] > ROW_H && g.rows[1] == ROW_H && g.rows[2] == ROW_H);
        assert!(g.expanded.x >= OPTIONS_MIN_W);
        let r = s.entity_rect(0, Which::Shown);
        let row2 = r.min.y + HEADER_H + 3.0 + g.rows[0] + g.rows[1] + ROW_H * 0.5;
        assert_eq!(s.hit_test(pos2(r.center().x, row2)).unwrap().field, Some(2));
        let options_line = r.min.y + HEADER_H + 3.0 + ROW_H + 2.0;
        assert_eq!(
            s.hit_test(pos2(r.center().x, options_line)).unwrap().field,
            Some(0)
        );
    }

    #[test]
    fn tag_chips_widen_the_header_and_groups_expand_selectively() {
        let src = format!(
            "{SRC}%% describe CUSTOMER: #vip #pii Buys things.\n%% describe PRODUCT: #pii\n"
        );
        let mut s = Scene::new(parse(&src), GroupingMode::File, None, &CharMeasure);
        let plain = Scene::new(parse(SRC), GroupingMode::File, None, &CharMeasure);
        let g = &s.ents[0].geom;
        assert_eq!(g.tag_w.len(), 2);
        assert!(g.collapsed.x >= plain.ents[0].geom.collapsed.x + g.tag_w.iter().sum::<f32>());
        assert_eq!(
            group_tags(&s.diagram, &s.grouping.groups[1].members),
            [1],
            "Catalog carries #pii"
        );
        s.set_groups_expanded(&[false, true]);
        assert!(!s.grps[0].expanded && s.grps[1].expanded);
        assert!(no_top_overlap(&s));
    }

    #[test]
    fn fields_master_switch_hides_and_restores() {
        let mut s = scene();
        s.set_group_expanded(0, true);
        s.set_fields(&[0, 1], true);
        let open = s.entity_size(0);
        s.set_fields_visible(false);
        assert_eq!(s.entity_size(0), s.ents[0].geom.collapsed);
        assert!(s.ents[0].show_fields, "own state is kept");
        assert!(no_top_overlap(&s));
        let saved = s.to_saved([0.0, 0.0, 1.0]);
        assert!(saved.fields_hidden);
        s.set_fields_visible(true);
        assert_eq!(s.entity_size(0), open);
        let s2 = Scene::new(parse(SRC), GroupingMode::File, Some(&saved), &CharMeasure);
        assert!(!s2.fields_visible && s2.ents[0].show_fields);
    }
}
