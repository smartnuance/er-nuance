//! Minimal inline markup for descriptions: `*italic*`, `**strong**` and `` `code` ``.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

pub fn job(text: &str, size: f32, color: Color32, accent: Color32, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let (mut italic, mut strong, mut code) = (false, false, false);
    let mut buf = String::new();
    let flush = |job: &mut LayoutJob, buf: &mut String, italic: bool, strong: bool, code: bool| {
        if buf.is_empty() {
            return;
        }
        let font_id = if code { FontId::monospace(size * 0.92) } else { FontId::proportional(size) };
        let color = if code || strong { accent } else { color };
        job.append(buf, 0.0, TextFormat { font_id, color, italics: italic, ..Default::default() });
        buf.clear();
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                flush(&mut job, &mut buf, italic, strong, code);
                code = !code;
            }
            '*' if !code && chars.peek() == Some(&'*') => {
                chars.next();
                flush(&mut job, &mut buf, italic, strong, code);
                strong = !strong;
            }
            '*' if !code => {
                flush(&mut job, &mut buf, italic, strong, code);
                italic = !italic;
            }
            _ => buf.push(c),
        }
    }
    flush(&mut job, &mut buf, italic, strong, code);
    job
}

/// The text without markup characters, e.g. for width estimates.
#[cfg(test)]
pub fn plain(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '*' | '`')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_markup_into_sections() {
        let j = job("a *b* `c` **d**", 12.0, Color32::WHITE, Color32::RED, 100.0);
        assert_eq!(j.text, "a b c d");
        assert_eq!(j.sections.len(), 6);
        assert!(j.sections[1].format.italics);
        assert_eq!(j.sections[3].format.font_id.family, egui::FontFamily::Monospace);
        assert_eq!(j.sections[5].format.color, Color32::RED);
    }
}
