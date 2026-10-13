//! Painting the scene: group frames, collapsed groups, entities and relationship edges.

use std::collections::HashSet;

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind, Vec2, ecolor::Hsva, epaint::CubicBezierShape, pos2, vec2,
};

use crate::diff::{self, Status};
use crate::model::Cardinality;
use crate::scene::{
    Font, GROUP_HEADER_H, GROUP_LIST_MAX, GROUP_LIST_ROW, GROUP_SUB_H, HEADER_H, NodeRef, PAD,
    ROW_H, Scene, VisEdge, Which, COL_GAP, DESC_PAD, DESC_W, TAG_AFTER_NAME, TAG_DOT, TAG_GAP,
    group_tags,
};

/// Tag colors, far apart in hue, handed out in order of first appearance (dark, light theme).
const TAG_COLORS: [([u8; 3], [u8; 3]); 10] = [
    ([0x4f, 0xc3, 0xf7], [0x02, 0x77, 0xbd]),
    ([0xff, 0xb7, 0x4d], [0xe6, 0x51, 0x00]),
    ([0x81, 0xc7, 0x84], [0x2e, 0x7d, 0x32]),
    ([0xf0, 0x62, 0x92], [0xc2, 0x18, 0x5b]),
    ([0xba, 0x68, 0xc8], [0x7b, 0x1f, 0xa2]),
    ([0xff, 0xf1, 0x76], [0x9e, 0x86, 0x00]),
    ([0x4d, 0xb6, 0xac], [0x00, 0x79, 0x6b]),
    ([0xe5, 0x73, 0x73], [0xc6, 0x28, 0x28]),
    ([0x9f, 0xa8, 0xda], [0x39, 0x49, 0xab]),
    ([0xa1, 0x88, 0x7f], [0x6d, 0x4c, 0x41]),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Screen position of the world origin, relative to the canvas' top-left corner.
    pub offset: Vec2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { offset: Vec2::ZERO, zoom: 1.0 }
    }
}

impl Camera {
    pub fn to_screen(self, canvas: Rect, p: Pos2) -> Pos2 {
        canvas.min + self.offset + p.to_vec2() * self.zoom
    }

    pub fn to_world(self, canvas: Rect, p: Pos2) -> Pos2 {
        ((p - canvas.min - self.offset) / self.zoom).to_pos2()
    }

    pub fn rect_to_screen(&self, canvas: Rect, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(canvas, r.min), self.to_screen(canvas, r.max))
    }

    /// Camera showing `world` centered in `canvas`.
    pub fn fitting(canvas: Rect, world: Rect, max_zoom: f32) -> Self {
        let zoom = (canvas.width() / world.width().max(1.0))
            .min(canvas.height() / world.height().max(1.0))
            * 0.92;
        let zoom = zoom.clamp(0.05, max_zoom);
        Self { zoom, offset: canvas.size() * 0.5 - world.center().to_vec2() * zoom }
    }

    pub fn centered_on(canvas: Rect, p: Pos2, zoom: f32) -> Self {
        Self { zoom, offset: canvas.size() * 0.5 - p.to_vec2() * zoom }
    }

    pub fn zoom_around(&mut self, canvas: Rect, screen: Pos2, factor: f32) {
        let world = self.to_world(canvas, screen);
        self.zoom = (self.zoom * factor).clamp(0.05, 4.0);
        self.offset = screen - canvas.min - world.to_vec2() * self.zoom;
    }

    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        Self { offset: self.offset + (to.offset - self.offset) * t, zoom: self.zoom + (to.zoom - self.zoom) * t }
    }
}

pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub dot: Color32,
    pub ent_fill: Color32,
    pub ent_stroke: Color32,
    pub row_alt: Color32,
    pub desc_fill: Color32,
    pub options: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub edge: Color32,
    pub edge_dim: Color32,
    pub accent: Color32,
    pub neighbour: Color32,
    pub search: Color32,
    pub pk: Color32,
    pub fk: Color32,
    pub added: Color32,
    pub removed: Color32,
    pub changed: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(0x1c, 0x1d, 0x21),
                dot: Color32::from_rgb(0x2c, 0x2e, 0x34),
                ent_fill: Color32::from_rgb(0x2a, 0x2c, 0x32),
                ent_stroke: Color32::from_rgb(0x4a, 0x4d, 0x56),
                row_alt: Color32::from_rgba_unmultiplied(255, 255, 255, 6),
                desc_fill: Color32::from_rgb(0x25, 0x27, 0x2c),
                options: Color32::from_rgb(0x8f, 0xb8, 0x96),
                text: Color32::from_rgb(0xe6, 0xe7, 0xea),
                muted: Color32::from_rgb(0x95, 0x99, 0xa3),
                edge: Color32::from_rgb(0x8a, 0x8f, 0x9a),
                edge_dim: Color32::from_rgb(0x46, 0x49, 0x50),
                accent: Color32::from_rgb(0x5a, 0xa9, 0xff),
                neighbour: Color32::from_rgb(0x9a, 0xc8, 0xff),
                search: Color32::from_rgb(0xff, 0xc8, 0x3d),
                pk: Color32::from_rgb(0xe8, 0xb8, 0x4a),
                fk: Color32::from_rgb(0x7f, 0xb6, 0xff),
                added: Color32::from_rgb(0x5c, 0xc9, 0x8a),
                removed: Color32::from_rgb(0xef, 0x6b, 0x6b),
                changed: Color32::from_rgb(0xf0, 0xa6, 0x4a),
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(0xf6, 0xf6, 0xf4),
                dot: Color32::from_rgb(0xdd, 0xdd, 0xd8),
                ent_fill: Color32::WHITE,
                ent_stroke: Color32::from_rgb(0xb8, 0xbb, 0xc2),
                row_alt: Color32::from_rgba_unmultiplied(0, 0, 0, 8),
                desc_fill: Color32::from_rgb(0xfa, 0xfa, 0xf8),
                options: Color32::from_rgb(0x3d, 0x7a, 0x4a),
                text: Color32::from_rgb(0x1d, 0x1f, 0x24),
                muted: Color32::from_rgb(0x6b, 0x70, 0x7a),
                edge: Color32::from_rgb(0x6a, 0x6f, 0x7a),
                edge_dim: Color32::from_rgb(0xc4, 0xc6, 0xcc),
                accent: Color32::from_rgb(0x1f, 0x6f, 0xeb),
                neighbour: Color32::from_rgb(0x5d, 0x9b, 0xf0),
                search: Color32::from_rgb(0xe0, 0x9a, 0x00),
                pk: Color32::from_rgb(0xb0, 0x7d, 0x00),
                fk: Color32::from_rgb(0x1f, 0x6f, 0xeb),
                added: Color32::from_rgb(0x1f, 0x9d, 0x55),
                removed: Color32::from_rgb(0xd6, 0x45, 0x45),
                changed: Color32::from_rgb(0xc7, 0x7d, 0x00),
            }
        }
    }

    pub fn group(&self, hue: f32) -> Color32 {
        if self.dark { Hsva::new(hue, 0.45, 0.62, 1.0).into() } else { Hsva::new(hue, 0.45, 0.72, 1.0).into() }
    }

    pub fn group_header(&self, hue: f32) -> Color32 {
        if self.dark { Hsva::new(hue, 0.5, 0.42, 1.0).into() } else { Hsva::new(hue, 0.22, 0.93, 1.0).into() }
    }

    pub fn group_tint(&self, hue: f32) -> Color32 {
        let c: Color32 = Hsva::new(hue, 0.5, 0.7, 1.0).into();
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), if self.dark { 22 } else { 34 })
    }

    /// Color of a diff status (`None` for unchanged).
    pub fn status(&self, s: Status) -> Option<Color32> {
        match s {
            Status::Same => None,
            Status::Added => Some(self.added),
            Status::Removed => Some(self.removed),
            Status::Changed => Some(self.changed),
        }
    }

    /// Color of the tag at `index` in [`crate::model::Diagram::tags`].
    pub fn tag(&self, index: usize) -> Color32 {
        match TAG_COLORS.get(index) {
            Some((d, l)) => {
                let [r, g, b] = if self.dark { *d } else { *l };
                Color32::from_rgb(r, g, b)
            }
            None => Hsva::new((index as f32 * 0.618_034 + 0.1).fract(), 0.6, if self.dark { 0.9 } else { 0.6 }, 1.0).into(),
        }
    }

    /// Text on a filled (highlighted) tag chip.
    pub fn tag_text_on_fill(&self) -> Color32 {
        if self.dark { self.bg } else { Color32::WHITE }
    }

    pub fn ungrouped_header(&self) -> Color32 {
        if self.dark { Color32::from_rgb(0x3a, 0x3d, 0x45) } else { Color32::from_rgb(0xe6, 0xe7, 0xea) }
    }
}

/// Per-frame highlight state.
pub struct Highlight {
    pub selected: Option<NodeRef>,
    pub neighbours: HashSet<NodeRef>,
    /// Entities matching the search.
    pub search: HashSet<usize>,
    pub hovered_edge: Option<usize>,
    /// Highlighted tags, by index into the diagram's tags.
    pub tags: HashSet<usize>,
}

/// One painted edge segment, for hover detection.
pub struct EdgeHit {
    pub a: Pos2,
    pub b: Pos2,
    pub edge: usize,
}

pub struct Canvas<'a> {
    pub painter: &'a Painter,
    pub rect: Rect,
    pub cam: Camera,
    pub pal: &'a Palette,
    pub hl: &'a Highlight,
}

impl Canvas<'_> {
    fn z(&self) -> f32 {
        self.cam.zoom
    }

    fn screen(&self, r: Rect) -> Rect {
        self.cam.rect_to_screen(self.rect, r)
    }

    fn text(&self, pos: Pos2, anchor: Align2, text: &str, font: Font, color: Color32) {
        let size = font.size() * self.z();
        if size < 4.0 {
            return;
        }
        let family = if font.monospace() { FontFamily::Monospace } else { FontFamily::Proportional };
        self.painter.text(pos, anchor, text, FontId::new(size, family), color);
    }

    fn radius(&self, r: f32) -> CornerRadius {
        CornerRadius::same((r * self.z()).clamp(0.0, 255.0) as u8)
    }

    pub fn background(&self) {
        self.painter.rect_filled(self.rect, 0.0, self.pal.bg);
        let spacing = 32.0 * self.z();
        if spacing < 14.0 {
            return;
        }
        let o = self.rect.min + self.cam.offset;
        let x0 = o.x + ((self.rect.min.x - o.x) / spacing).ceil() * spacing;
        let y0 = o.y + ((self.rect.min.y - o.y) / spacing).ceil() * spacing;
        let mut y = y0;
        while y < self.rect.max.y {
            let mut x = x0;
            while x < self.rect.max.x {
                self.painter.circle_filled(pos2(x, y), 1.0, self.pal.dot);
                x += spacing;
            }
            y += spacing;
        }
    }

    fn visible(&self, r: Rect) -> bool {
        self.rect.intersects(r)
    }

    fn outline(&self, n: NodeRef, base: Stroke, search_hit: bool) -> Stroke {
        if self.hl.selected == Some(n) {
            Stroke::new(2.5, self.pal.accent)
        } else if search_hit {
            Stroke::new(2.5, self.pal.search)
        } else if self.hl.neighbours.contains(&n) {
            Stroke::new(1.8, self.pal.neighbour)
        } else {
            base
        }
    }

    pub fn group_frame(&self, scene: &Scene, g: usize) {
        let r = self.screen(scene.group_rect(g, Which::Shown));
        if !self.visible(r) {
            return;
        }
        let st = &scene.grps[g];
        let color = self.pal.group(st.hue);
        let stroke = self.outline(NodeRef::Group(g), Stroke::new(1.5, color), false);
        self.painter.rect(r, self.radius(10.0), self.pal.group_tint(st.hue), stroke, StrokeKind::Inside);
        let z = self.z();
        let cy = r.min.y + GROUP_HEADER_H * 0.5 * z;
        self.triangle(pos2(r.min.x + (PAD + 5.0) * z, cy), true, color);
        let name = &scene.grouping.groups[g].name;
        self.text(pos2(r.min.x + (PAD + 16.0) * z, cy), Align2::LEFT_CENTER, name, Font::Title, color);
        let members = &scene.grouping.groups[g].members;
        let count = format!("{} entities", members.len());
        let right = pos2(r.max.x - PAD * z, cy);
        let count_w = self.text_width(&count, Font::Small);
        self.text(right, Align2::RIGHT_CENTER, &count, Font::Small, self.pal.muted);
        self.diff_badge(scene, members, right - vec2(count_w + 8.0 * z, 0.0));
    }

    fn text_width(&self, text: &str, font: Font) -> f32 {
        let size = font.size() * self.z();
        if size < 4.0 {
            return 0.0;
        }
        let family = if font.monospace() { FontFamily::Monospace } else { FontFamily::Proportional };
        self.painter.layout_no_wrap(text.to_owned(), FontId::new(size, family), Color32::WHITE).size().x
    }

    /// `+a −r ~c` for the given entities on a dark pill, right-aligned at `right`.
    fn diff_badge(&self, scene: &Scene, entities: &[usize], right: Pos2) {
        let Some(d) = &scene.diff else { return };
        let c = d.counts_of(entities);
        if c.is_empty() || Font::Small.size() * self.z() < 4.0 {
            return;
        }
        let z = self.z();
        let w = self.text_width(&c.badge(), Font::Small) + 5.0 * z;
        let pill = Rect::from_min_max(pos2(right.x - w - 2.0 * z, right.y - 9.0 * z), pos2(right.x + 5.0 * z, right.y + 9.0 * z));
        self.painter.rect_filled(pill, self.radius(9.0), self.pal.bg);
        let mut x = right.x;
        for (sign, n, color) in [("~", c.changed, self.pal.changed), ("−", c.removed, self.pal.removed), ("+", c.added, self.pal.added)] {
            if n == 0 {
                continue;
            }
            let t = format!("{sign}{n}");
            self.text(pos2(x, right.y), Align2::RIGHT_CENTER, &t, Font::Small, color);
            x -= self.text_width(&t, Font::Small) + 5.0 * self.z();
        }
    }

    pub fn collapsed_group(&self, scene: &Scene, g: usize) {
        let r = self.screen(scene.group_rect(g, Which::Shown));
        if !self.visible(r) {
            return;
        }
        let z = self.z();
        let st = &scene.grps[g];
        let color = self.pal.group(st.hue);
        let members = &scene.grouping.groups[g].members;
        let hits = members.iter().filter(|e| self.hl.search.contains(e)).count();

        // Stacked cards hint that this box contains several entities.
        for k in [2.0, 1.0] {
            let back = r.translate(vec2(5.0, 5.0) * k * z);
            self.painter.rect(back, self.radius(8.0), self.pal.ent_fill, Stroke::new(1.0, color.gamma_multiply(0.6)), StrokeKind::Inside);
        }
        let stroke = self.outline(NodeRef::Group(g), Stroke::new(1.8, color), hits > 0);
        self.painter.rect(r, self.radius(8.0), self.pal.ent_fill, Stroke::NONE, StrokeKind::Inside);
        let header = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + GROUP_HEADER_H * z));
        let rr = (8.0 * z).clamp(0.0, 255.0) as u8;
        self.painter.rect_filled(header, CornerRadius { nw: rr, ne: rr, sw: 0, se: 0 }, self.pal.group_header(st.hue));
        self.painter.rect_stroke(r, self.radius(8.0), stroke, StrokeKind::Inside);

        let cy = header.center().y;
        self.triangle(pos2(r.min.x + (PAD + 5.0) * z, cy), false, self.pal.text);
        self.text(pos2(r.min.x + (PAD + 16.0) * z, cy), Align2::LEFT_CENTER, &scene.grouping.groups[g].name, Font::Title, self.pal.text);
        // One dot per tag among the members, filled when highlighted.
        let mut x = r.min.x + (PAD + 16.0 + st.title_w + 6.0 + TAG_DOT * 0.5) * z;
        for t in group_tags(&scene.diagram, members) {
            let c = self.pal.tag(t);
            if self.hl.tags.contains(&t) {
                self.painter.circle_filled(pos2(x, cy), 4.0 * z, c);
            } else {
                self.painter.circle(pos2(x, cy), 3.2 * z, self.pal.ent_fill, Stroke::new(1.5 * z, c));
            }
            x += TAG_DOT * z;
        }
        self.diff_badge(scene, members, pos2(r.max.x - PAD * z, cy));

        let internal = scene.internal_relationships(g);
        let mut sub = format!("{} entities · {} internal", members.len(), internal);
        if hits > 0 {
            sub = format!("{sub} · {hits} match");
        }
        let sub_y = header.max.y + GROUP_SUB_H * 0.5 * z + 2.0 * z;
        self.text(pos2(r.min.x + PAD * z, sub_y), Align2::LEFT_CENTER, &sub, Font::Small, self.pal.muted);

        let mut y = header.max.y + (GROUP_SUB_H + 4.0 + GROUP_LIST_ROW * 0.5) * z;
        // Show search matches, then changes, first so they are never cut off.
        let rank = |e: &usize| (!self.hl.search.contains(e), !scene.entity_status(*e).differs());
        let mut list: Vec<usize> = members.clone();
        list.sort_by_key(rank);
        for &e in list.iter().take(GROUP_LIST_MAX) {
            let c = if self.hl.search.contains(&e) {
                self.pal.search
            } else {
                self.pal.status(scene.entity_status(e)).unwrap_or(self.pal.text)
            };
            self.text(pos2(r.min.x + (PAD + 6.0) * z, y), Align2::LEFT_CENTER, scene.diagram.entities[e].display_name(), Font::Small, c);
            y += GROUP_LIST_ROW * z;
        }
        if members.len() > GROUP_LIST_MAX {
            let more = format!("+ {} more", members.len() - GROUP_LIST_MAX);
            self.text(pos2(r.min.x + (PAD + 6.0) * z, y), Align2::LEFT_CENTER, &more, Font::Small, self.pal.muted);
        }
    }

    pub fn entity(&self, scene: &Scene, e: usize) {
        let r = self.screen(scene.entity_rect(e, Which::Shown));
        if !self.visible(r) {
            return;
        }
        let status = scene.entity_status(e);
        if status == Status::Removed {
            let mut faded = self.painter.clone();
            faded.multiply_opacity(0.55);
            return Canvas { painter: &faded, ..*self }.entity_box(scene, e, r, status);
        }
        self.entity_box(scene, e, r, status);
    }

    fn entity_box(&self, scene: &Scene, e: usize, r: Rect, status: Status) {
        let z = self.z();
        let header_color = match (self.pal.status(status), scene.group_of(e)) {
            (Some(c), _) => self.pal.ent_fill.lerp_to_gamma(c, if self.pal.dark { 0.42 } else { 0.3 }),
            (None, Some(g)) => self.pal.group_header(scene.grps[g].hue),
            (None, None) => self.pal.ungrouped_header(),
        };
        let base = match self.pal.status(status) {
            Some(c) => Stroke::new(2.0, c),
            None => Stroke::new(1.0, self.pal.ent_stroke),
        };
        let stroke = self.outline(NodeRef::Entity(e), base, self.hl.search.contains(&e));
        self.painter.rect_filled(r, self.radius(5.0), self.pal.ent_fill);
        let rr = (5.0 * z).clamp(0.0, 255.0) as u8;
        let header = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + HEADER_H * z));
        let desc_h = scene.desc_block_h(e);
        let bottom = if scene.fields_shown(e) || desc_h > 0.0 { 0 } else { rr };
        self.painter.rect_filled(header, CornerRadius { nw: rr, ne: rr, sw: bottom, se: bottom }, header_color);
        // Collapsed boxes with fields show the sign next to the field count instead.
        if status.differs() && (scene.fields_shown(e) || scene.diagram.entities[e].attributes.is_empty()) {
            self.text(pos2(r.max.x - PAD * z, header.center().y), Align2::RIGHT_CENTER, status.sign(), Font::Name, self.pal.text);
        }
        self.tag_chips(scene, e, r, header);
        self.entity_body(scene, e, r, header, desc_h);
        if status == Status::Removed {
            let w = self.text_width(scene.diagram.entities[e].display_name(), Font::Name);
            let y = header.center().y;
            let x = r.min.x + PAD * z;
            self.painter.hline(x..=x + w, y, Stroke::new(1.2, self.pal.text));
        }
        // The outline goes on top, so fills inside the box cannot cover part of it.
        self.painter.rect_stroke(r, self.radius(5.0), stroke, StrokeKind::Inside);
    }

    /// The entity's tags as pills after its name: outlined, or filled when highlighted.
    fn tag_chips(&self, scene: &Scene, e: usize, r: Rect, header: Rect) {
        let z = self.z();
        if Font::Small.size() * z < 5.0 {
            return;
        }
        let geom = &scene.ents[e].geom;
        let cy = header.center().y;
        let mut x = r.min.x + (PAD + geom.title_w + TAG_AFTER_NAME) * z;
        for (tag, w) in scene.diagram.entities[e].tags.iter().zip(&geom.tag_w) {
            let Some(t) = scene.diagram.tag_index(tag) else { continue };
            let c = self.pal.tag(t);
            let chip = Rect::from_min_max(pos2(x, cy - 8.0 * z), pos2(x + w * z, cy + 8.0 * z));
            if self.hl.tags.contains(&t) {
                self.painter.rect_filled(chip, self.radius(8.0), c);
                self.text(chip.center(), Align2::CENTER_CENTER, tag, Font::Small, self.pal.tag_text_on_fill());
            } else {
                self.painter.rect(chip, self.radius(8.0), self.pal.ent_fill.gamma_multiply(0.85), Stroke::new(1.2, c), StrokeKind::Inside);
                self.text(chip.center(), Align2::CENTER_CENTER, tag, Font::Small, c);
            }
            x += (w + TAG_GAP) * z;
        }
    }

    fn entity_body(&self, scene: &Scene, e: usize, r: Rect, header: Rect, desc_h: f32) {
        let z = self.z();
        let ent = &scene.diagram.entities[e];
        let st = &scene.ents[e];
        let fields = scene.fields_shown(e);
        let cy = header.center().y;
        self.text(pos2(r.min.x + PAD * z, cy), Align2::LEFT_CENTER, ent.display_name(), Font::Name, self.pal.text);
        if !fields && !ent.attributes.is_empty() {
            let status = scene.entity_status(e);
            let count = match status.sign() {
                "" => ent.attributes.len().to_string(),
                sign => format!("{sign} {}", ent.attributes.len()),
            };
            let color = if status.differs() { self.pal.text } else { self.pal.muted };
            self.text(pos2(r.max.x - PAD * z, cy), Align2::RIGHT_CENTER, &count, Font::Small, color);
        }
        if desc_h > 0.0
            && let Some(text) = &ent.description
        {
            let block = Rect::from_min_max(header.left_bottom(), pos2(r.max.x, header.max.y + desc_h * z));
            // Last section of the box when fields are hidden: round its bottom like the box.
            let rr = if fields { 0 } else { (5.0 * z).clamp(0.0, 255.0) as u8 };
            self.painter.rect_filled(block, CornerRadius { nw: 0, ne: 0, sw: rr, se: rr }, self.pal.desc_fill);
            if fields {
                self.painter.hline(block.x_range(), block.max.y, Stroke::new(1.0, self.pal.ent_stroke));
            }
            if Font::Small.size() * z >= 4.0 {
                let job = crate::richtext::job(text, Font::Small.size() * z, self.pal.muted, self.pal.text, (DESC_W - 2.0 * PAD) * z);
                let galley = self.painter.layout_job(job);
                let clip = block.intersect(self.painter.clip_rect());
                self.painter.with_clip_rect(clip).galley(pos2(r.min.x + PAD * z, block.min.y + DESC_PAD * z), galley, self.pal.muted);
            }
        }
        if !fields || Font::Field.size() * z < 4.0 {
            return;
        }
        let x_type = r.min.x + PAD * z;
        let x_name = x_type + (st.geom.type_w + COL_GAP) * z;
        let x_keys = x_name + (st.geom.name_w + COL_GAP) * z;
        let mut top = header.max.y + (desc_h + 3.0) * z;
        let diffs = scene.diff.as_ref().map(|d| &d.entities[e].attrs);
        for (i, a) in ent.attributes.iter().enumerate() {
            let row_h = st.geom.rows[i] * z;
            let row = Rect::from_min_max(pos2(r.min.x + 1.0, top), pos2(r.max.x - 1.0, top + row_h));
            if i % 2 == 1 {
                self.painter.rect_filled(row, 0.0, self.pal.row_alt);
            }
            let ad = diffs.map(|d| &d[i]);
            let row_status = ad.map_or(Status::Same, |d| d.status);
            // Inside an added/removed entity every row has that status; the box says so already.
            let own = row_status.differs() && row_status != scene.entity_status(e);
            if own && let Some(c) = self.pal.status(row_status) {
                self.painter.rect_filled(row, 0.0, c.gamma_multiply(0.16));
                self.painter.rect_filled(Rect::from_min_max(row.min, pos2(row.min.x + 3.0 * z, row.max.y)), 0.0, c);
            }
            if let Some(options) = a.enum_options()
                && Font::Options.size() * z >= 4.0
            {
                let font = FontId::monospace(Font::Options.size() * z);
                let galley = self.painter.layout(options, font, self.pal.options, st.geom.options_w * z);
                let area = Rect::from_min_max(pos2(r.min.x, top + ROW_H * z), pos2(r.max.x, top + row_h));
                self.painter.with_clip_rect(area.intersect(self.painter.clip_rect())).galley(pos2(x_type, area.min.y), galley, self.pal.options);
            }
            let y = top + ROW_H * 0.5 * z;
            let changed = |differs: bool, normal: Color32| if differs && row_status == Status::Changed { self.pal.changed } else { normal };
            let old = ad.and_then(|d| d.old.as_ref());
            self.text(pos2(x_type, y), Align2::LEFT_CENTER, &diff::type_text(a, ad), Font::Field, changed(old.is_some_and(|o| o.ty != a.ty), self.pal.muted));
            let is_pk = a.keys.iter().any(|k| k == "PK");
            self.text(pos2(x_name, y), Align2::LEFT_CENTER, &a.name, Font::Field, if is_pk { self.pal.pk } else { self.pal.text });
            let keys = diff::keys_text(a, ad);
            if !keys.is_empty() {
                let key_color = if is_pk { self.pal.pk } else { self.pal.fk };
                self.text(pos2(x_keys, y), Align2::LEFT_CENTER, &keys, Font::Field, changed(old.is_some_and(|o| o.keys != a.keys), key_color));
            }
            if own && row_status == Status::Removed {
                let w = self.text_width(&a.name, Font::Field);
                self.painter.hline(x_name..=x_name + w, y, Stroke::new(1.0, self.pal.removed));
            }
            top += row_h;
        }
    }

    fn triangle(&self, c: Pos2, down: bool, color: Color32) {
        let s = 4.5 * self.z();
        let pts = if down {
            vec![c + vec2(-s, -s * 0.6), c + vec2(s, -s * 0.6), c + vec2(0.0, s * 0.7)]
        } else {
            vec![c + vec2(-s * 0.6, -s), c + vec2(-s * 0.6, s), c + vec2(s * 0.7, 0.0)]
        };
        self.painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    }

    /// Paints all edges; returns segments for hover detection.
    pub fn edges(&self, scene: &Scene, edges: &[VisEdge]) -> Vec<EdgeHit> {
        let mut hits = Vec::new();
        for (i, edge) in edges.iter().enumerate() {
            let ra = self.screen(scene.node_rect(edge.a, Which::Shown));
            let rb = self.screen(scene.node_rect(edge.b, Which::Shown));
            if !self.visible(ra.union(rb)) {
                continue;
            }
            let lit = self.hl.selected.is_some_and(|s| s == edge.a || s == edge.b) || self.hl.hovered_edge == Some(i);
            let dim = self.hl.selected.is_some() && !lit;
            let shade = |status: Status| match (lit, self.pal.status(status)) {
                (true, _) => self.pal.accent,
                (false, Some(c)) => if dim { c.gamma_multiply(0.45) } else { c },
                (false, None) => if dim { self.pal.edge_dim } else { self.pal.edge },
            };
            let color = shade(bundle_status(scene, &edge.rels));

            if edge.a == edge.b {
                self.self_loop(scene, ra, edge, color, &mut hits, i);
                continue;
            }
            if edge.touches_group() && edge.rels.len() > 1 {
                self.bundle(scene, ra, rb, edge, color, lit, &mut hits, i);
                continue;
            }
            let n = edge.rels.len() as f32;
            for (k, &ri) in edge.rels.iter().enumerate() {
                let rel = &scene.diagram.relationships[ri];
                let color = shade(scene.rel_status(ri));
                let (card_a, card_b) = if scene.visible_node(rel.a) == edge.a { (rel.card_a, rel.card_b) } else { (rel.card_b, rel.card_a) };
                let dir = (rb.center() - ra.center()).normalized();
                let off = dir.rot90() * (k as f32 - (n - 1.0) * 0.5) * 14.0 * self.z();
                let (Some(pa), Some(pb)) = (exit_point(ra, ra.center() + off, dir), exit_point(rb, rb.center() + off, -dir)) else {
                    continue;
                };
                let width = if lit { 2.0 } else { 1.3 };
                self.line(pa, pb, Stroke::new(width, color), rel.identifying);
                self.cardinality(pa, dir, card_a, Stroke::new(width, color));
                self.cardinality(pb, -dir, card_b, Stroke::new(width, color));
                if self.z() > 0.55 && !rel.label.is_empty() {
                    self.label(pa + (pb - pa) * 0.5, &rel.label, color, lit);
                }
                hits.push(EdgeHit { a: pa, b: pb, edge: i });
            }
        }
        hits
    }

    #[allow(clippy::too_many_arguments)]
    fn bundle(&self, scene: &Scene, ra: Rect, rb: Rect, edge: &VisEdge, color: Color32, lit: bool, hits: &mut Vec<EdgeHit>, i: usize) {
        let dir = (rb.center() - ra.center()).normalized();
        let (Some(pa), Some(pb)) = (exit_point(ra, ra.center(), dir), exit_point(rb, rb.center(), -dir)) else {
            return;
        };
        let n = edge.rels.len();
        let width = (1.3 + (n as f32).ln() * 1.2) * if lit { 1.4 } else { 1.0 };
        let identifying = edge.rels.iter().any(|&r| scene.diagram.relationships[r].identifying);
        self.line(pa, pb, Stroke::new(width, color), identifying);
        for p in [pa, pb] {
            self.painter.circle_filled(p, 3.5 * self.z().max(0.5), color);
        }
        if self.z() > 0.25 {
            self.label(pa + (pb - pa) * 0.5, &format!("{n} relations"), color, lit);
        }
        hits.push(EdgeHit { a: pa, b: pb, edge: i });
    }

    fn self_loop(&self, scene: &Scene, r: Rect, edge: &VisEdge, color: Color32, hits: &mut Vec<EdgeHit>, i: usize) {
        let z = self.z();
        for (k, &ri) in edge.rels.iter().enumerate() {
            let s = (26.0 + k as f32 * 14.0) * z;
            let a = pos2(r.max.x, r.min.y + 10.0 * z);
            let b = pos2(r.max.x - 20.0 * z, r.min.y);
            let pts = [a, a + vec2(s, 0.0), b + vec2(0.0, -s), b];
            self.painter.add(CubicBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, Stroke::new(1.3, color)));
            let rel = &scene.diagram.relationships[ri];
            if z > 0.55 && !rel.label.is_empty() {
                self.label(a + vec2(s * 0.8, -s * 0.8), &rel.label, color, false);
            }
            hits.push(EdgeHit { a: a + vec2(s * 0.6, 0.0), b: b + vec2(0.0, -s * 0.6), edge: i });
        }
    }

    fn line(&self, a: Pos2, b: Pos2, stroke: Stroke, solid: bool) {
        if solid {
            self.painter.line_segment([a, b], stroke);
        } else {
            let z = self.z().max(0.3);
            self.painter.extend(Shape::dashed_line(&[a, b], stroke, 6.0 * z, 4.0 * z));
        }
    }

    fn label(&self, at: Pos2, text: &str, color: Color32, strong: bool) {
        let size = Font::Small.size() * self.z().clamp(0.6, 1.4);
        let galley = self.painter.layout_no_wrap(text.to_owned(), FontId::proportional(size), if strong { color } else { self.pal.muted });
        let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 2.0));
        self.painter.rect(r, CornerRadius::same(4), self.pal.bg, Stroke::new(0.8, color.gamma_multiply(0.5)), StrokeKind::Inside);
        self.painter.galley(r.min + vec2(4.0, 1.0), galley, color);
    }

    /// Crow's-foot notation at `p` (on the box border); `dir` points away from the box.
    fn cardinality(&self, p: Pos2, dir: Vec2, card: Cardinality, stroke: Stroke) {
        let z = self.z();
        if z < 0.2 {
            return;
        }
        let perp = dir.rot90();
        let bar = |d: f32| {
            let c = p + dir * d * z;
            self.painter.line_segment([c + perp * 6.0 * z, c - perp * 6.0 * z], stroke);
        };
        let circle = |d: f32| {
            self.painter.circle(p + dir * d * z, 4.0 * z, self.pal.bg, stroke);
        };
        if card.is_many() {
            let apex = p + dir * 13.0 * z;
            for k in [-1.0, 0.0, 1.0] {
                self.painter.line_segment([apex, p + perp * 7.0 * z * k], stroke);
            }
            if card.is_optional() { circle(19.0) } else { bar(17.0) }
        } else {
            bar(8.0);
            if card.is_optional() { circle(17.0) } else { bar(13.0) }
        }
    }
}

/// One status for several relationships drawn as one line: theirs if they agree, else changed.
fn bundle_status(scene: &Scene, rels: &[usize]) -> Status {
    let mut it = rels.iter().map(|&r| scene.rel_status(r)).filter(|s| s.differs());
    match it.next() {
        None => Status::Same,
        Some(first) if it.all(|s| s == first) && rels.iter().all(|&r| scene.rel_status(r) == first) => first,
        Some(_) => Status::Changed,
    }
}

/// Where a ray from `from` (inside `r`) in direction `dir` leaves `r`.
fn exit_point(r: Rect, from: Pos2, dir: Vec2) -> Option<Pos2> {
    if !dir.is_finite() || dir == Vec2::ZERO {
        return None;
    }
    let from = r.clamp(from);
    let t = |p: f32, d: f32, lo: f32, hi: f32| {
        if d > 0.0 {
            (hi - p) / d
        } else if d < 0.0 {
            (lo - p) / d
        } else {
            f32::INFINITY
        }
    };
    let tx = t(from.x, dir.x, r.min.x, r.max.x);
    let ty = t(from.y, dir.y, r.min.y, r.max.y);
    Some(from + dir * tx.min(ty))
}

pub fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_point_hits_border() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        assert_eq!(exit_point(r, r.center(), vec2(1.0, 0.0)), Some(pos2(100.0, 25.0)));
        assert_eq!(exit_point(r, r.center(), vec2(0.0, -1.0)), Some(pos2(50.0, 0.0)));
    }

    #[test]
    fn camera_round_trip() {
        let canvas = Rect::from_min_max(pos2(10.0, 20.0), pos2(810.0, 620.0));
        let mut cam = Camera::fitting(canvas, Rect::from_min_max(pos2(-500.0, -100.0), pos2(900.0, 700.0)), 2.0);
        let p = pos2(123.0, -45.0);
        assert!((cam.to_world(canvas, cam.to_screen(canvas, p)) - p).length() < 1e-3);
        let s = pos2(300.0, 300.0);
        let before = cam.to_world(canvas, s);
        cam.zoom_around(canvas, s, 1.7);
        assert!((cam.to_world(canvas, s) - before).length() < 1e-3);
    }
}
