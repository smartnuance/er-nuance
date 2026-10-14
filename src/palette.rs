//! ⌘K / Ctrl+K command palette: type-ahead over commands, flags and entities.

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, Context, Event, FontId, Id, Key, KeyboardShortcut, Modifiers, Order,
    Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

pub struct Item<A> {
    pub title: String,
    /// Shown dimmed after the title.
    pub detail: Option<String>,
    pub shortcut: Option<KeyboardShortcut>,
    /// `Some(state)` for on/off settings: drawn as a checkbox, toggled with Space.
    pub flag: Option<bool>,
    /// Only listed once something has been typed (e.g. "go to entity" entries).
    pub needs_query: bool,
    pub action: A,
}

impl<A> Item<A> {
    pub fn command(
        title: impl Into<String>,
        shortcut: Option<KeyboardShortcut>,
        action: A,
    ) -> Self {
        Self {
            title: title.into(),
            detail: None,
            shortcut,
            flag: None,
            needs_query: false,
            action,
        }
    }

    pub fn flag(
        title: impl Into<String>,
        on: bool,
        shortcut: Option<KeyboardShortcut>,
        action: A,
    ) -> Self {
        Self {
            flag: Some(on),
            ..Self::command(title, shortcut, action)
        }
    }
}

pub struct Outcome<A> {
    pub run: Option<A>,
    pub close: bool,
}

pub struct CommandPalette {
    query: String,
    selected: usize,
    /// Scroll the selection into view on the next frame (after keyboard navigation).
    reveal: bool,
}

const ROW_H: f32 = 26.0;
const WIDTH: f32 = 580.0;

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            reveal: false,
        }
    }

    pub fn show<A: Copy>(&mut self, ctx: &Context, items: &[Item<A>]) -> Outcome<A> {
        // Keys are taken before the text field sees them. Space never types: it toggles flags,
        // and matching ignores spaces anyway.
        let (up, down, enter, esc, space) = ctx.input_mut(|i| {
            let space = i.consume_key(Modifiers::NONE, Key::Space);
            i.events
                .retain(|e| !matches!(e, Event::Text(t) if t == " "));
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                space,
            )
        });

        let mut out = Outcome {
            run: None,
            close: esc,
        };
        let matches = filter(items, &self.query);
        if !matches.is_empty() {
            let n = matches.len();
            if up {
                self.selected = (self.selected + n - 1) % n;
                self.reveal = true;
            }
            if down {
                self.selected = (self.selected + 1) % n;
                self.reveal = true;
            }
            self.selected = self.selected.min(n - 1);
            let current = &items[matches[self.selected].0];
            if enter {
                out = Outcome {
                    run: Some(current.action),
                    close: true,
                };
            } else if space && current.flag.is_some() {
                out.run = Some(current.action);
            }
        }

        let area = egui::Area::new(Id::new("command-palette"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 64.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .id(Id::new("command-palette-query"))
                                .hint_text(
                                    "Type a command, setting or entity…   (Space toggles settings)",
                                )
                                .desired_width(f32::INFINITY),
                        );
                        edit.request_focus();
                        if edit.changed() {
                            self.selected = 0;
                        }
                        ui.add_space(4.0);
                        if matches.is_empty() {
                            ui.weak("No matching commands");
                            return;
                        }
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| {
                                for (row, (idx, hits)) in matches.iter().enumerate() {
                                    let item = &items[*idx];
                                    let resp = self.row(ui, item, hits, row == self.selected);
                                    if row == self.selected && std::mem::take(&mut self.reveal) {
                                        resp.scroll_to_me(Some(Align::Center));
                                    }
                                    if resp.hovered()
                                        && ui.input(|i| i.pointer.delta() != Vec2::ZERO)
                                    {
                                        self.selected = row;
                                    }
                                    if resp.clicked() {
                                        // Settings stay open so several can be flipped in a row.
                                        out = Outcome {
                                            run: Some(item.action),
                                            close: item.flag.is_none(),
                                        };
                                    }
                                }
                            });
                    });
            });

        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p))
        });
        out.close |= clicked_outside;
        out
    }

    fn row<A>(
        &self,
        ui: &mut egui::Ui,
        item: &Item<A>,
        hits: &[usize],
        selected: bool,
    ) -> egui::Response {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let v = ui.visuals();
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.6));
        }
        let text_color = v.text_color();
        let mut x = rect.min.x + 8.0;
        if let Some(on) = item.flag {
            let b = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            painter.rect(
                b,
                3.0,
                if on {
                    v.selection.bg_fill
                } else {
                    Color32::TRANSPARENT
                },
                Stroke::new(1.0, v.weak_text_color()),
                StrokeKind::Inside,
            );
            if on {
                let s = Stroke::new(2.0, v.strong_text_color());
                painter.line_segment(
                    [
                        b.left_center() + vec2(3.0, 0.0),
                        b.center_bottom() + vec2(-1.0, -3.5),
                    ],
                    s,
                );
                painter.line_segment(
                    [
                        b.center_bottom() + vec2(-1.0, -3.5),
                        b.right_top() + vec2(-3.0, 3.5),
                    ],
                    s,
                );
            }
        }
        x += 24.0;

        let font = FontId::proportional(14.0);
        let mut job = LayoutJob::default();
        for (i, c) in item.title.chars().enumerate() {
            let hit = hits.contains(&i);
            let color = if hit { v.hyperlink_color } else { text_color };
            job.append(
                &c.to_string(),
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color,
                    underline: if hit {
                        Stroke::new(1.0, color)
                    } else {
                        Stroke::NONE
                    },
                    ..Default::default()
                },
            );
        }
        if let Some(d) = &item.detail {
            job.append(
                d,
                10.0,
                TextFormat {
                    font_id: FontId::proportional(12.5),
                    color: v.weak_text_color(),
                    ..Default::default()
                },
            );
        }
        let galley = painter.layout_job(job);
        painter.galley(
            pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            text_color,
        );

        if let Some(sc) = &item.shortcut {
            let text = format_shortcut(ui.ctx(), sc);
            let g = painter.layout_no_wrap(text, FontId::monospace(12.0), v.weak_text_color());
            let key = Rect::from_min_size(
                pos2(rect.max.x - 10.0 - g.size().x - 8.0, rect.center().y - 10.0),
                vec2(g.size().x + 8.0, 20.0),
            );
            painter.rect(
                key,
                4.0,
                v.extreme_bg_color,
                Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
                StrokeKind::Inside,
            );
            painter.galley(
                key.min + vec2(4.0, (20.0 - g.size().y) * 0.5),
                g,
                v.weak_text_color(),
            );
        }
        resp
    }
}

/// `⇧F` / `⌘K` on macOS; `Shift+F` / `Ctrl+K` elsewhere, with keys as symbols (`.` not `Period`).
fn format_shortcut(ctx: &Context, sc: &KeyboardShortcut) -> String {
    if ctx.os().is_mac() {
        ctx.format_shortcut(sc)
    } else {
        sc.format(
            &egui::ModifierNames {
                is_short: true,
                ..egui::ModifierNames::NAMES
            },
            false,
        )
    }
}

/// Matching items as (index, matched title char positions), best first.
fn filter<A>(items: &[Item<A>], query: &str) -> Vec<(usize, Vec<usize>)> {
    let q: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return items
            .iter()
            .enumerate()
            .filter(|(_, it)| !it.needs_query)
            .map(|(i, _)| (i, Vec::new()))
            .collect();
    }
    let mut scored: Vec<(i32, usize, Vec<usize>)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy(&q, &it.title).map(|(s, hits)| (s, i, hits)))
        .collect();
    // Stable: equal scores keep the listed order (commands before entities).
    scored.sort_by_key(|(s, _, _)| -s);
    scored.into_iter().map(|(_, i, hits)| (i, hits)).collect()
}

/// Subsequence match, preferring word starts and consecutive runs. Returns the score and the
/// matched character positions in `text`.
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let t: Vec<char> = text.chars().collect();
    let lower: Vec<char> = t.iter().map(|c| c.to_ascii_lowercase()).collect();
    let word_start = |i: usize| {
        i == 0
            || matches!(t[i - 1], ' ' | '_' | '-' | ':' | '(' | '/')
            || (t[i].is_uppercase() && t[i - 1].is_lowercase())
    };
    let mut hits = Vec::new();
    let mut score = 0;
    let mut from = 0;
    for qc in query.chars().map(|c| c.to_ascii_lowercase()) {
        // Prefer the next word start with this letter; otherwise the next occurrence.
        let next = (from..t.len())
            .find(|&i| lower[i] == qc && word_start(i))
            .filter(|&i| {
                // ...unless the plain next occurrence continues the current run.
                !(hits
                    .last()
                    .is_some_and(|&l: &usize| lower.get(l + 1) == Some(&qc) && l + 1 < i))
            });
        let i = next.or_else(|| (from..t.len()).find(|&i| lower[i] == qc))?;
        score += 1;
        if word_start(i) {
            score += 6;
        }
        if hits.last().is_some_and(|&l| l + 1 == i) {
            score += 4;
        }
        hits.push(i);
        from = i + 1;
    }
    score -= hits.first().copied().unwrap_or(0) as i32 / 4;
    score -= (t.len() as i32 - query.len() as i32) / 10;
    Some((score, hits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts() {
        let (_, hits) = fuzzy("sf", "Show fields").unwrap();
        assert_eq!(hits, [0, 5]);
        let (_, hits) = fuzzy("fit", "Fit all").unwrap();
        assert_eq!(hits, [0, 1, 2]);
        assert!(fuzzy("xyz", "Show fields").is_none());
        let a = fuzzy("sd", "Show descriptions").unwrap().0;
        let b = fuzzy("sd", "Search entities and fields").unwrap().0;
        assert!(a > b);
    }

    #[test]
    fn filter_hides_goto_entries_until_typing() {
        let items = vec![
            Item::command("Fit all", None, 0),
            Item {
                needs_query: true,
                ..Item::command("ORDER", None, 1)
            },
        ];
        assert_eq!(filter(&items, "").len(), 1);
        assert_eq!(filter(&items, "ord")[0].0, 1);
        assert_eq!(filter(&items, "  f a ")[0].0, 0, "spaces are ignored");
    }
}
