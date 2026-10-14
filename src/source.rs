//! What is shown: one diagram (a side) or the difference of two, and how to read them.
//!
//! A side is in memory (picked or dropped in the browser, pasted, the built-in example), a file
//! on disk, or a file at a git revision. The browser only ever has in-memory sides.

use std::path::PathBuf;

use crate::diff::{self, DiffMarks};
use crate::mermaid;
use crate::model::Diagram;
use crate::vcs::{self, GitFile, Rev};

pub const EXAMPLE: &str = include_str!("../examples/commerce.mmd");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Text { name: String, text: String },
    File(PathBuf),
    Git { file: GitFile, rev: Rev },
}

impl Side {
    pub fn git(file: &GitFile, rev: Rev) -> Self {
        Self::Git {
            file: file.clone(),
            rev,
        }
    }

    pub fn example() -> Self {
        Self::Text {
            name: "commerce.mmd".into(),
            text: EXAMPLE.into(),
        }
    }

    /// Short description for the status bar and window title.
    pub fn label(&self) -> String {
        match self {
            Self::Text { name, .. } => name.clone(),
            Self::File(p) => p
                .file_name()
                .unwrap_or(p.as_os_str())
                .to_string_lossy()
                .into_owned(),
            Self::Git { rev, .. } => rev.label(),
        }
    }

    /// The text. `Ok(None)` when the file does not exist there (e.g. not yet committed).
    pub fn read(&self) -> Result<Option<String>, String> {
        match self {
            Self::Text { text, .. } => Ok(Some(text.clone())),
            Self::File(p) => std::fs::read(p)
                .map(|b| Some(String::from_utf8_lossy(&b).into_owned()))
                .map_err(|e| format!("cannot read {}: {e}", p.display())),
            Self::Git { file, rev } => vcs::read(file, rev),
        }
    }

    /// The file on disk that can change while we look at it (polled for live reload).
    pub fn live_path(&self) -> Option<PathBuf> {
        match self {
            Self::File(p) => Some(p.clone()),
            Self::Git {
                file,
                rev: Rev::WorkTree,
            } => Some(file.disk_path()),
            _ => None,
        }
    }

    /// The file on disk this side is about (for layouts, "exit diff", git lookup).
    pub fn path(&self) -> Option<PathBuf> {
        match self {
            Self::Text { .. } => None,
            Self::File(p) => Some(p.clone()),
            Self::Git { file, .. } => Some(file.disk_path()),
        }
    }

    /// Where this side's layout is remembered.
    pub fn layout_key(&self) -> crate::persist::LayoutKey {
        use crate::persist::LayoutKey;
        match self {
            Self::Text { name, .. } => LayoutKey::Name(name.clone()),
            Self::File(p) => LayoutKey::Path(p.clone()),
            Self::Git { file, .. } => LayoutKey::Path(file.disk_path()),
        }
    }

    pub fn git_file(&self) -> Option<&GitFile> {
        match self {
            Self::Git { file, .. } => Some(file),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    View(Side),
    Diff { old: Side, new: Side },
}

impl Source {
    /// The side this view is about (the new one, in diff mode).
    pub fn current(&self) -> &Side {
        match self {
            Self::View(s) | Self::Diff { new: s, .. } => s,
        }
    }

    pub fn is_diff(&self) -> bool {
        matches!(self, Self::Diff { .. })
    }

    pub fn live_paths(&self) -> Vec<PathBuf> {
        match self {
            Self::View(s) => s.live_path().into_iter().collect(),
            Self::Diff { old, new } => old.live_path().into_iter().chain(new.live_path()).collect(),
        }
    }

    /// Reads and parses: the diagram (merged, in diff mode), the diff marks, a title.
    pub fn read(&self) -> Result<(Diagram, Option<DiffMarks>, String), String> {
        let parse = |text: &str| mermaid::parse(&mermaid::extract_er_source(text));
        match self {
            Self::View(s) => {
                let text = s
                    .read()?
                    .ok_or_else(|| format!("{} does not exist", s.label()))?;
                let title = s
                    .path()
                    .map_or_else(|| s.label(), |p| p.display().to_string());
                Ok((parse(&text), None, title))
            }
            Self::Diff { old, new } => {
                // A side without the file (not yet committed, deleted) is an empty diagram.
                let side = |s: &Side| {
                    s.read()
                        .map(|t| t.as_deref().map(parse).unwrap_or_default())
                };
                let r = diff::diff(&side(old)?, &side(new)?);
                let place = new
                    .path()
                    .map_or_else(|| new.label(), |p| p.display().to_string());
                let title = format!("diff: {} → {} — {place}", old.label(), new.label());
                Ok((r.merged, Some(r.marks), title))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_side_reads_its_text_and_is_labelled_by_name() {
        let s = Side::Text {
            name: "a.mmd".into(),
            text: "erDiagram\nA ||--o{ B : x\n".into(),
        };
        assert_eq!(
            s.read().unwrap().as_deref(),
            Some("erDiagram\nA ||--o{ B : x\n")
        );
        assert_eq!(s.label(), "a.mmd");
        assert_eq!(s.live_path(), None);
        assert_eq!(s.path(), None);
    }

    #[test]
    fn view_of_text_parses_and_diff_of_texts_marks_changes() {
        let a = Side::Text {
            name: "a.mmd".into(),
            text: "erDiagram\nA ||--o{ B : x\n".into(),
        };
        let b = Side::Text {
            name: "b.mmd".into(),
            text: "erDiagram\nA ||--o{ B : x\nB ||--o{ C : y\n".into(),
        };
        let (d, marks, title) = Source::View(a.clone()).read().unwrap();
        assert_eq!(d.entities.len(), 2);
        assert!(marks.is_none());
        assert_eq!(title, "a.mmd");
        let (d, marks, title) = Source::Diff { old: a, new: b }.read().unwrap();
        assert_eq!(d.entities.len(), 3);
        assert_eq!(marks.unwrap().entity_counts().added, 1);
        assert!(title.starts_with("diff: a.mmd → b.mmd"), "{title}");
    }

    #[test]
    fn example_is_an_in_memory_side() {
        let s = Side::example();
        assert_eq!(s.label(), "commerce.mmd");
        assert!(Source::View(s).read().unwrap().0.entities.len() > 40);
    }

    #[test]
    fn labels() {
        let file = crate::vcs::GitFile {
            root: "/r".into(),
            path: "s.mmd".into(),
        };
        assert_eq!(Side::git(&file, Rev::WorkTree).label(), "working tree");
        assert_eq!(
            Side::git(&file, Rev::Commit("a".repeat(40))).label(),
            "aaaaaaaa"
        );
        assert_eq!(Side::git(&file, Rev::Commit("main".into())).label(), "main");
        assert_eq!(Side::File("/tmp/x/old.mmd".into()).label(), "old.mmd");
    }
}
