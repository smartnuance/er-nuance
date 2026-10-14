# Web Version Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run ER Nuance in the browser (local-only: open, drop or paste diagrams, diff two files) and
deploy it as a static nginx image to `er.smartnuance.com` on the `contentnuance` cluster.

**Architecture:** One crate, two targets. A portable source model (`Side::Text` in memory, next to
the existing `File` / `Git` sides) replaces direct file-path handling in the app; layout persistence
and file picking get small platform modules (sidecar + blocking dialog natively, localStorage +
async dialog on the web). git and the CLI stay native-only. trunk builds the wasm bundle; a
multi-stage Docker image serves it with nginx-unprivileged; kustomize manifests live in `k8s/`.

**Tech Stack:** Rust 2024, eframe/egui 0.36 (wgpu), trunk 0.21, wasm-bindgen, web-sys, rfd 0.17,
nginx-unprivileged, kustomize, cargo-make.

**Spec:** `docs/superpowers/specs/2026-10-14-web-version-design.md`

## Global Constraints

- Native behaviour must not change: `ernuance [FILE]`, `ernuance diff …`, git compare, live reload,
  `.layout.json` sidecars, macOS native menu. All existing tests keep passing.
- Web target: `wasm32-unknown-unknown`; `gix` and `clap` are native-only dependencies.
- Layout key on the web: `localStorage` item `ernuance.layout.<file name>`, value = `SavedLayout` JSON.
- Diff mode never writes a layout (native or web).
- Web bundle output: `target/web/`; Rust profile `web-release` (`opt-level = "s"`, `lto = true`,
  `codegen-units = 1`); trunk runs `wasm-opt`.
- Image: `ghcr.io/smartnuance/er-nuance:<version>` and `:latest`, `--platform linux/amd64`, public
  package, nginx-unprivileged on port 8080, non-root, read-only root FS with `emptyDir` on `/tmp`.
- k8s: namespace `er-nuance`, label `app: er-nuance`, Service 80 → 8080, Ingress host
  `er.smartnuance.com`, `ingressClassName: nginx`, `cert-manager.io/cluster-issuer: letsencrypt`,
  `nginx.ingress.kubernetes.io/ssl-redirect: "true"`, TLS secret `er-nuance-tls`, requests
  10m/32Mi, limits 100m/64Mi, probes on `/`.
- Never push images or apply to the cluster during implementation.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. A picked or dropped file that is not UTF-8 (or not Mermaid at all) must not crash: it opens
   with replacement characters / parser warnings and a status message. → test in Task 3.
2. Cancelling the file dialog must leave the current diagram untouched. → test in Task 3.
3. Two different diagrams with the same file name share a localStorage layout: an unrelated saved
   layout must not crash or hide entities (unknown ids ignored, missing ones placed). → test in Task 2.
4. Pasting nothing (empty / whitespace) must not replace the current diagram; the paste window
   stays open with a message. → test in Task 3.
5. "Compare with file…" while the built-in example (an in-memory side) is open must work — the
   example becomes the new side. → test in Task 3.

---

## File structure

| File | Responsibility |
|---|---|
| `src/source.rs` (new) | `Side` (`Text` / `File` / `Git`), `Source` (`View` / `Diff`), reading and parsing a source, labels, layout keys, live paths |
| `src/vcs.rs` | git only: `GitFile`, `Rev`, `locate`, `read(file, rev)`, `verify`, `history`; gix code native-only, web stubs return `None` / `Err` |
| `src/persist.rs` (new) | `LayoutKey`, `load`, `save`; sidecar natively, localStorage on the web |
| `src/picker.rs` (new) | `Picker`: open a file dialog for a `Purpose`, deliver `Picked { purpose, side }` later; blocking natively, async on the web |
| `src/app.rs` | uses `Source` / `Side`, `persist`, `Picker`; paste window; hides native-only commands on the web |
| `src/main.rs` | native `main` (CLI → `Source`), web `main` (WebRunner on the canvas) |
| `src/cli.rs` | native-only; produces `Source` sides (imports `source::Side`) |
| `index.html`, `Trunk.toml` (new) | trunk entry and config |
| `Dockerfile`, `docker/nginx.conf`, `.dockerignore` (new) | image |
| `Makefile.toml` (new) | `web-serve`, `web-image`, `web-push`, `web-deploy` |
| `k8s/*.yaml` (new) | namespace, deployment, service, ingress, kustomization |

---

### Task 1: Portable source model

Move `Side` out of `vcs.rs` into a new `source.rs`, add `Side::Text`, turn the app's private
`Source` into `source::Source { View(Side), Diff { old, new } }` (the example becomes a
`Side::Text`), and make `ErApp::new` take `Option<Source>` instead of the CLI's `Launch`.

**Files:**
- Create: `src/source.rs`
- Modify: `src/vcs.rs` (remove `Side`, `read(&Side)` → `read(&GitFile, &Rev)`), `src/cli.rs`
  (import `Side` from `source`), `src/app.rs` (Source enum, `read_source`, `new`, actions, tests),
  `src/main.rs` (module + Launch → Source)

**Interfaces:**
- Produces:
  - `source::Side { Text { name: String, text: String }, File(PathBuf), Git { file: vcs::GitFile, rev: vcs::Rev } }` with
    `Side::git(&GitFile, Rev) -> Side`, `Side::example() -> Side`, `label(&self) -> String`,
    `read(&self) -> Result<Option<String>, String>`, `live_path(&self) -> Option<PathBuf>`,
    `path(&self) -> Option<PathBuf>` (on-disk file, for File/Git), `git_file(&self) -> Option<&GitFile>`
  - `source::Source { View(Side), Diff { old: Side, new: Side } }` with `current(&self) -> &Side`,
    `is_diff(&self) -> bool`, `live_paths(&self) -> Vec<PathBuf>`,
    `read(&self) -> Result<(Diagram, Option<DiffMarks>, String), String>` (diagram, marks, title)
  - `vcs::read(file: &GitFile, rev: &Rev) -> Result<Option<String>, String>`
  - `app::ErApp::new(cc, initial: Option<Source>)`

- [ ] **Step 1: Write the failing tests** in `src/source.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_side_reads_its_text_and_is_labelled_by_name() {
        let s = Side::Text { name: "a.mmd".into(), text: "erDiagram\nA ||--o{ B : x\n".into() };
        assert_eq!(s.read().unwrap().as_deref(), Some("erDiagram\nA ||--o{ B : x\n"));
        assert_eq!(s.label(), "a.mmd");
        assert_eq!(s.live_path(), None);
        assert_eq!(s.path(), None);
    }

    #[test]
    fn view_of_text_parses_and_diff_of_texts_marks_changes() {
        let a = Side::Text { name: "a.mmd".into(), text: "erDiagram\nA ||--o{ B : x\n".into() };
        let b = Side::Text { name: "b.mmd".into(), text: "erDiagram\nA ||--o{ B : x\nB ||--o{ C : y\n".into() };
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
        let file = crate::vcs::GitFile { root: "/r".into(), path: "s.mmd".into() };
        assert_eq!(Side::git(&file, Rev::WorkTree).label(), "working tree");
        assert_eq!(Side::git(&file, Rev::Commit("a".repeat(40))).label(), "aaaaaaaa");
        assert_eq!(Side::git(&file, Rev::Commit("main".into())).label(), "main");
        assert_eq!(Side::File("/tmp/x/old.mmd".into()).label(), "old.mmd");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --bin ernuance source::`
Expected: compile error (`source` module does not exist).

- [ ] **Step 3: Implement `src/source.rs`**

```rust
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
        Self::Git { file: file.clone(), rev }
    }

    pub fn example() -> Self {
        Self::Text { name: "commerce.mmd".into(), text: EXAMPLE.into() }
    }

    /// Short description for the status bar and window title.
    pub fn label(&self) -> String {
        match self {
            Self::Text { name, .. } => name.clone(),
            Self::File(p) => p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned(),
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
            Self::Git { file, rev: Rev::WorkTree } => Some(file.disk_path()),
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
                let text = s.read()?.ok_or_else(|| format!("{} does not exist", s.label()))?;
                let title = s.path().map_or_else(|| s.label(), |p| p.display().to_string());
                Ok((parse(&text), None, title))
            }
            Self::Diff { old, new } => {
                // A side without the file (not yet committed, deleted) is an empty diagram.
                let side = |s: &Side| s.read().map(|t| t.as_deref().map(parse).unwrap_or_default());
                let r = diff::diff(&side(old)?, &side(new)?);
                let place = new.path().map_or_else(|| new.label(), |p| p.display().to_string());
                let title = format!("diff: {} → {} — {place}", old.label(), new.label());
                Ok((r.merged, Some(r.marks), title))
            }
        }
    }
}
```

In `src/vcs.rs`: delete `Side`, its `impl`, and the old `read(side: &Side)`; add `Rev::label`
(the body of the old `Git` arms of `Side::label`) and the new `read`:

```rust
impl Rev {
    pub fn label(&self) -> String {
        match self {
            Self::WorkTree => "working tree".to_owned(),
            Self::Index => "index".to_owned(),
            Self::Commit(c) => short_rev(c),
        }
    }
}

/// The text of `file` at `rev`. `Ok(None)` when the file does not exist there.
pub fn read(file: &GitFile, rev: &Rev) -> Result<Option<String>, String> {
    let bytes = match rev {
        Rev::WorkTree => match std::fs::read(file.disk_path()) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("cannot read {}: {e}", file.disk_path().display())),
        },
        Rev::Index => {
            let repo = open(file)?;
            let index = repo.open_index().map_err(|e| format!("cannot read the git index: {e}"))?;
            match index.entry_by_path(unix_path(&file.path).as_str().into()) {
                None => None,
                Some(entry) => Some(repo.find_object(entry.id).map_err(|e| e.to_string())?.data.clone()),
            }
        }
        Rev::Commit(spec) => {
            let repo = open(file)?;
            let tree = commit_tree(&repo, spec)?;
            match tree.lookup_entry_by_path(&file.path).map_err(|e| e.to_string())? {
                None => None,
                Some(entry) => Some(entry.object().map_err(|e| e.to_string())?.data.clone()),
            }
        }
    };
    Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
}
```

Update `vcs` tests: `read(&Side::git(&file, rev))` → `read(&file, &rev)`; drop the `labels` test
(moved to `source`). In `cli.rs`: `use crate::source::Side; use crate::vcs::{self, Rev};`.

In `src/app.rs`:
- Remove the private `Source` enum + impl, `EXAMPLE`, `read_source`; `use crate::source::{Side, Source};`.
- `ErApp::new(cc, initial: Option<Source>)` with `pending: initial`.
- `Action::LoadExample` → `self.load(ctx, Source::View(Side::example()))`.
- `load`: `source.read()`; `saved = source.current().path().and_then(|p| read_sidecar(&p))`;
  `git = source.current().git_file().cloned().or_else(|| source.current().path().and_then(|p| vcs::locate(&p)))`.
- `reload`: drop the `Example` early return (in-memory sides simply re-read the same text); use `source.read()`.
- `save_if_due`: only for `Some(Source::View(side))` with `Some(p) = side.path()`.
- Open: `Source::View(Side::File(p))`; drop: same; ExitDiff: `Source::View(Side::File(p))` with
  `p = source.current().path()` filtered by `is_file()`.
- CompareFile: `let current = self.source.as_ref().map(|s| s.current().clone())`; load
  `Source::Diff { old: Side::File(p), new: current }`.
- `in_diff`: `self.source.as_ref().is_some_and(Source::is_diff)`.
- `diff_labels`: unchanged logic on `Source::Diff`.
- `native_menu` state: `has_file: self.source.as_ref().is_some_and(|s| s.current().path().is_some())`.
- Palette/menu "Compare with file…" condition: `self.source.is_some()`.
- Tests: `Launch::View(None)` → `None`; `Launch::Diff { old, new }` → `Some(Source::Diff { old, new })`;
  `Source::File(f)` → `Source::View(Side::File(f))`; `Source::Example` → `Source::View(Side::example())`;
  `read_source(&src)` → `src.read()`.

In `src/main.rs`: `mod source;` and convert the launch:

```rust
let initial = match launch {
    cli::Launch::View(path) => path.map(|p| source::Source::View(source::Side::File(p))),
    cli::Launch::Diff { old, new } => Some(source::Source::Diff { old, new }),
};
// … app::ErApp::new(cc, initial)
```

- [ ] **Step 4: Run all tests**

Run: `cargo test` and `cargo clippy --all-targets`
Expected: all pass (existing + 4 new), no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/source.rs src/vcs.rs src/cli.rs src/app.rs src/main.rs
git commit -m "Portable source model: in-memory sides, Source::View / Diff"
```

---

### Task 2: Layout persistence module

**Files:**
- Create: `src/persist.rs`
- Modify: `src/source.rs` (add `layout_key`), `src/app.rs` (use `persist` instead of
  `sidecar_path` / `read_sidecar` / `std::fs::write`), `src/main.rs` (`mod persist;`)

**Interfaces:**
- Consumes: `source::Side`, `scene::SavedLayout`
- Produces:
  - `persist::LayoutKey { Path(PathBuf), Name(String) }`
  - `persist::load(key: &LayoutKey) -> Option<SavedLayout>`
  - `persist::save(key: &LayoutKey, layout: &SavedLayout) -> Result<(), String>`
  - `persist::sidecar_path(p: &Path) -> PathBuf` (native; used by tests)
  - `Side::layout_key(&self) -> LayoutKey` — `Text` → `Name(name)`, `File` / `Git` → `Path(disk path)`

- [ ] **Step 1: Write the failing tests** in `src/persist.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::GroupingMode;
    use crate::mermaid::parse;
    use crate::scene::{Scene, tests::CharMeasure};

    #[test]
    fn sidecar_round_trip_and_names_are_not_stored_natively() {
        let dir = tempfile::tempdir().unwrap();
        let key = LayoutKey::Path(dir.path().join("s.mmd"));
        assert!(load(&key).is_none());
        let layout = SavedLayout { descriptions: true, tags: vec!["x".into()], ..Default::default() };
        save(&key, &layout).unwrap();
        assert!(dir.path().join("s.mmd.layout.json").is_file());
        let back = load(&key).unwrap();
        assert!(back.descriptions && back.tags == ["x"]);
        // In-memory sides have no place to store a layout on the desktop.
        save(&LayoutKey::Name("pasted.mmd".into()), &layout).unwrap();
        assert!(load(&LayoutKey::Name("pasted.mmd".into())).is_none());
    }

    #[test]
    fn a_layout_of_another_diagram_is_harmless() {
        // Same file name, different diagram (likely in the browser): unknown ids are ignored and
        // entities without a saved position are placed.
        let other = Scene::new(parse("erDiagram\nX ||--o{ Y : r\n"), GroupingMode::None, None, &CharMeasure);
        let saved = other.to_saved([0.0, 0.0, 1.0]);
        let s = Scene::new(parse("erDiagram\nA ||--o{ B : r\nB ||--o{ C : s\n"), GroupingMode::None, Some(&saved), &CharMeasure);
        assert_eq!(s.ents.len(), 3);
        let r: Vec<_> = (0..3).map(|e| s.entity_rect(e, crate::scene::Which::Target)).collect();
        assert!(!r[0].intersects(r[1]) && !r[1].intersects(r[2]) && !r[0].intersects(r[2]));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --bin ernuance persist::`
Expected: compile error (`persist` module does not exist).

- [ ] **Step 3: Implement `src/persist.rs`** (web part is added in Task 4; this task is native):

```rust
//! Where a diagram's layout (positions, open/closed state, view toggles) is remembered:
//! next to the file as `<file>.layout.json`, or (in the browser) in localStorage by file name.

use std::path::{Path, PathBuf};

use crate::scene::SavedLayout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutKey {
    /// A file on disk: its sidecar.
    Path(PathBuf),
    /// An in-memory diagram, by file name.
    Name(String),
}

pub fn load(key: &LayoutKey) -> Option<SavedLayout> {
    serde_json::from_str(&read(key)?).ok()
}

pub fn save(key: &LayoutKey, layout: &SavedLayout) -> Result<(), String> {
    let json = serde_json::to_string_pretty(layout).map_err(|e| e.to_string())?;
    write(key, &json)
}

pub fn sidecar_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".layout.json");
    PathBuf::from(s)
}

#[cfg(not(target_arch = "wasm32"))]
fn read(key: &LayoutKey) -> Option<String> {
    match key {
        LayoutKey::Path(p) => std::fs::read_to_string(sidecar_path(p)).ok(),
        LayoutKey::Name(_) => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn write(key: &LayoutKey, json: &str) -> Result<(), String> {
    match key {
        LayoutKey::Path(p) => std::fs::write(sidecar_path(p), json).map_err(|e| e.to_string()),
        LayoutKey::Name(_) => Ok(()),
    }
}
```

Add to `Side` in `source.rs`:

```rust
pub fn layout_key(&self) -> crate::persist::LayoutKey {
    use crate::persist::LayoutKey;
    match self {
        Self::Text { name, .. } => LayoutKey::Name(name.clone()),
        Self::File(p) => LayoutKey::Path(p.clone()),
        Self::Git { file, .. } => LayoutKey::Path(file.disk_path()),
    }
}
```

In `app.rs`: delete `sidecar_path` and `read_sidecar`; `load` uses
`persist::load(&source.current().layout_key())`; `save_if_due` becomes:

```rust
if let (Some(Source::View(side)), Some(scene)) = (&self.source, &self.scene) {
    let mut saved = scene.to_saved(self.camera_array());
    saved.descriptions = self.desc_toggle;
    saved.panels_hidden = self.panels_hidden;
    saved.tags = self.tag_highlight.iter().cloned().collect();
    if let Err(e) = crate::persist::save(&side.layout_key(), &saved) {
        self.status = format!("Cannot save layout: {e}");
    }
}
```

Tests in `app.rs` that call `sidecar_path(…)` use `crate::persist::sidecar_path(…)`.

- [ ] **Step 4: Run all tests** — `cargo test`, `cargo clippy --all-targets`: pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/persist.rs src/source.rs src/app.rs src/main.rs
git commit -m "Layout persistence module: sidecar per file, keyed by side"
```

---

### Task 3: File picker, drop of in-memory files, paste window

**Files:**
- Create: `src/picker.rs`
- Modify: `src/app.rs`, `src/main.rs` (`mod picker;`)

**Interfaces:**
- Consumes: `source::{Side, Source}`
- Produces:
  - `picker::Purpose { Open, CompareWith }` (Copy, PartialEq, Debug)
  - `picker::Picked { purpose: Purpose, side: Side }`
  - `picker::Picker` (Default, Clone): `pick(&self, purpose, ctx: &egui::Context)`,
    `deliver(&self, picked: Picked)`, `take(&self) -> Vec<Picked>`
  - `app::dropped_side(f: &egui::DroppedFile) -> Option<Side>`
  - `Action::Paste` (open the paste window), `Action::OpenPasted` (open its text)
  - `ErApp::handle_picked(&mut self, ctx, picked: Picked)`

- [ ] **Step 1: Write the failing tests** at the end of `mod screenshots` in `app.rs` (these use a
headless harness without `.wgpu()`, so they are not `#[ignore]`d):

```rust
fn headless(initial: Option<Source>) -> Harness<'static, ErApp> {
    Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(move |cc| ErApp::new(cc, initial.clone()))
}

fn text(name: &str, t: &str) -> Side {
    Side::Text { name: name.into(), text: t.into() }
}

#[test]
fn picked_files_open_and_compare() {
    let mut h = headless(Some(Source::View(Side::example())));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    let a = text("a.mmd", "erDiagram\nA ||--o{ B : x\n");
    h.state_mut().handle_picked(&ctx, Picked { purpose: Purpose::Open, side: a.clone() });
    assert_eq!(h.state().source, Some(Source::View(a.clone())));
    // The open diagram becomes the new side of the comparison.
    let old = text("old.mmd", "erDiagram\nA ||--o{ B : x\nB ||--o{ C : y\n");
    h.state_mut().handle_picked(&ctx, Picked { purpose: Purpose::CompareWith, side: old.clone() });
    assert_eq!(h.state().source, Some(Source::Diff { old, new: a }));
}

#[test]
fn compare_with_file_works_on_the_example() {
    let mut h = headless(Some(Source::View(Side::example())));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    let old = text("old.mmd", "erDiagram\nCUSTOMER { int id }\n");
    h.state_mut().handle_picked(&ctx, Picked { purpose: Purpose::CompareWith, side: old.clone() });
    assert_eq!(h.state().source, Some(Source::Diff { old, new: Side::example() }));
    assert!(h.state().scene.as_ref().unwrap().diff.is_some());
}

#[test]
fn cancelled_dialog_changes_nothing() {
    let mut h = headless(Some(Source::View(Side::example())));
    h.run_steps(2);
    let picker = h.state().picker.clone();
    assert!(picker.take().is_empty(), "nothing delivered: nothing to handle");
    h.run_steps(2);
    assert_eq!(h.state().source, Some(Source::View(Side::example())));
}

#[test]
fn non_utf8_and_non_mermaid_files_open_without_crashing() {
    let f = egui::DroppedFile { name: "weird.mmd".into(), bytes: Some(vec![0xff, 0xfe, b'x', b'\n'].into()), ..Default::default() };
    let side = dropped_side(&f).unwrap();
    let Side::Text { name, text: t } = &side else { panic!("in-memory side expected") };
    assert_eq!(name, "weird.mmd");
    assert!(t.contains('\u{FFFD}'));
    let mut h = headless(None);
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().handle_picked(&ctx, Picked { purpose: Purpose::Open, side });
    assert!(h.state().scene.is_some());
    // A dropped file with a path (desktop) is a file side.
    let f = egui::DroppedFile { path: Some("/tmp/x.mmd".into()), ..Default::default() };
    assert_eq!(dropped_side(&f), Some(Side::File("/tmp/x.mmd".into())));
    assert_eq!(dropped_side(&egui::DroppedFile::default()), None);
}

#[test]
fn pasting_opens_text_and_empty_paste_keeps_the_diagram() {
    let mut h = headless(Some(Source::View(Side::example())));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().apply(&ctx, Action::Paste, 0.0);
    h.state_mut().paste = Some("   \n".into());
    h.state_mut().apply(&ctx, Action::OpenPasted, 0.0);
    assert_eq!(h.state().source, Some(Source::View(Side::example())), "empty paste: nothing replaced");
    assert!(h.state().paste.is_some(), "window stays open");
    assert!(h.state().status.contains("Nothing to open"));
    h.state_mut().paste = Some("erDiagram\nP ||--o{ Q : r\n".into());
    h.state_mut().apply(&ctx, Action::OpenPasted, 0.0);
    assert_eq!(h.state().source, Some(Source::View(text("pasted.mmd", "erDiagram\nP ||--o{ Q : r\n"))));
    assert!(h.state().paste.is_none());
}
```

(`Source`/`Side` derive `PartialEq` from Task 1. `ErApp` fields `source`, `picker`, `paste`,
`status` are read directly; the tests module is a child of `app`.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --bin ernuance screenshots::`
Expected: compile errors (`picker`, `Picked`, `dropped_side`, `Action::Paste` missing).

- [ ] **Step 3: Implement `src/picker.rs`** (native part; Task 4 adds the web part):

```rust
//! Choosing a file to open or compare with. Natively the dialog blocks and the result is ready
//! at once; in the browser it is asynchronous and arrives a few frames later. Either way the app
//! collects results with [`Picker::take`] each frame.

use std::sync::{Arc, Mutex};

use crate::source::Side;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Open,
    /// Compare the picked file (old) with the open diagram (new).
    CompareWith,
}

#[derive(Clone, Debug)]
pub struct Picked {
    pub purpose: Purpose,
    pub side: Side,
}

#[derive(Clone, Default)]
pub struct Picker {
    ready: Arc<Mutex<Vec<Picked>>>,
}

const EXTENSIONS: &[&str] = &["mmd", "mermaid", "md", "markdown", "txt"];

impl Picker {
    pub fn deliver(&self, picked: Picked) {
        self.ready.lock().unwrap().push(picked);
    }

    pub fn take(&self) -> Vec<Picked> {
        std::mem::take(&mut *self.ready.lock().unwrap())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick(&self, purpose: Purpose, _ctx: &egui::Context) {
        let title = match purpose {
            Purpose::Open => "Open diagram",
            Purpose::CompareWith => "Compare with…",
        };
        if let Some(p) = rfd::FileDialog::new().add_filter("Mermaid ER", EXTENSIONS).set_title(title).pick_file() {
            self.deliver(Picked { purpose, side: Side::File(p) });
        }
    }
}
```

In `app.rs`:
- Field `picker: crate::picker::Picker` (init `Default::default()`), field `paste: Option<String>` (init `None`).
- `Action::Open` → `self.picker.pick(Purpose::Open, ctx); return;`
  `Action::CompareFile` → `if self.source.is_some() { self.picker.pick(Purpose::CompareWith, ctx); } return;`
- New actions and early handling in `apply`:

```rust
Action::Paste => {
    self.paste.get_or_insert_with(String::new);
    return;
}
Action::OpenPasted => {
    let text = self.paste.clone().unwrap_or_default();
    if text.trim().is_empty() {
        self.status = "Nothing to open: paste a Mermaid erDiagram first".to_owned();
        return;
    }
    self.paste = None;
    return self.load(ctx, Source::View(Side::Text { name: "pasted.mmd".into(), text }));
}
```

  Add `Action::Paste | Action::OpenPasted` to the `unreachable!()` arm list.
- `handle_picked`:

```rust
fn handle_picked(&mut self, ctx: &Context, picked: Picked) {
    match picked.purpose {
        Purpose::Open => self.load(ctx, Source::View(picked.side)),
        Purpose::CompareWith => {
            let Some(current) = self.source.as_ref().map(|s| s.current().clone()) else { return };
            self.load(ctx, Source::Diff { old: picked.side, new: current });
        }
    }
}
```

- `dropped_side` (free function):

```rust
/// A dropped file as a side: by path on the desktop, by its bytes in the browser.
fn dropped_side(f: &egui::DroppedFile) -> Option<Side> {
    if let Some(p) = f.path.as_ref().filter(|p| !p.as_os_str().is_empty()) {
        return Some(Side::File(p.clone()));
    }
    let bytes = f.bytes.as_ref()?;
    let name = if f.name.is_empty() { "dropped.mmd".to_owned() } else { f.name.clone() };
    Some(Side::Text { name, text: String::from_utf8_lossy(bytes).into_owned() })
}
```

- In `ui()`, replace the dropped-file block with:

```rust
if let Some(side) = ctx.input(|i| i.raw.dropped_files.iter().find_map(dropped_side)) {
    self.load(&ctx, Source::View(side));
}
for picked in self.picker.take() {
    self.handle_picked(&ctx, picked);
}
```

- Paste window, shown in `ui()` next to the help window:

```rust
if let Some(text) = &mut self.paste {
    let mut open = true;
    let mut go = false;
    egui::Window::new("Paste diagram").open(&mut open).default_size([520.0, 420.0]).show(&ctx, |ui| {
        ui.label("Paste a Mermaid erDiagram (or Markdown with ```mermaid blocks):");
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(14));
        });
        go = ui.button("Open").clicked();
    });
    if go {
        self.apply(&ctx, Action::OpenPasted, now);
    } else if !open {
        self.paste = None;
    }
}
```

- "Paste diagram…" in: welcome screen (button after "Open file…"), File menu (egui and native:
  `m.item("Paste Diagram…", None, Action::Paste, Needs::Always)`), palette
  (`Item::command("Paste diagram…", None, Action::Paste)` next to "Open file…").

- [ ] **Step 4: Run all tests** — `cargo test`, `cargo clippy --all-targets`: pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/picker.rs src/app.rs src/main.rs src/native_menu.rs
git commit -m "File picker abstraction, in-memory drops, paste window"
```

---

### Task 4: Web target

**Files:**
- Create: `index.html`, `Trunk.toml`
- Modify: `Cargo.toml`, `src/main.rs`, `src/vcs.rs`, `src/persist.rs`, `src/picker.rs`, `src/app.rs`,
  `.gitignore`

**Interfaces:**
- Consumes: everything above.
- Produces: `trunk build --release` → `target/web/index.html` + hashed `.js` / `.wasm`.

- [ ] **Step 1: Add the target and see it fail**

Run: `rustup target add wasm32-unknown-unknown && cargo check --target wasm32-unknown-unknown`
Expected: FAIL (gix / clap / rfd blocking dialog / std-only code does not build for wasm).

- [ ] **Step 2: Dependencies** — in `Cargo.toml` move `clap` and `gix` to a native-only table and
add the web ones:

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
clap = { version = "4.6.7", features = ["derive"] }
gix = { version = "0.89.0", default-features = false, features = ["revision", "index", "sha1"] }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["Window", "Document", "Element", "HtmlCanvasElement", "Storage"] }

[profile.web-release]
inherits = "release"
opt-level = "s"
lto = true
codegen-units = 1
```

(Use the `wasm-bindgen-futures` / `web-sys` versions eframe already resolves in `Cargo.lock`:
`cargo tree -i web-sys --target wasm32-unknown-unknown`.)

- [ ] **Step 3: Native-only modules and web stubs**
  - `main.rs`: `#[cfg(not(target_arch = "wasm32"))] mod cli;` and the existing `fn main` under
    the same cfg; add the web entry:

```rust
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window().expect("window").document().expect("document");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("canvas in index.html")
            .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
            .expect("a canvas");
        let start = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(app::ErApp::new(cc, None)))))
            .await;
        if let Some(loading) = document.get_element_by_id("loading_text") {
            match start {
                Ok(()) => loading.remove(),
                Err(e) => {
                    loading.set_inner_html("<p>ER Nuance could not start. See the browser console for details.</p>");
                    eframe::web_sys::console::error_1(&e);
                }
            }
        }
    });
}
```

    (add `"console"` to the web-sys features).
  - `vcs.rs`: put everything that uses `gix` (`locate`, `read`, `verify`, `history`, `open`,
    `commit_tree`) under `#[cfg(not(target_arch = "wasm32"))]`, and add web stubs with the same
    signatures:

```rust
#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    const NO_GIT: &str = "git is not available in the browser";
    pub fn locate(_: &Path) -> Option<GitFile> { None }
    pub fn read(_: &GitFile, _: &Rev) -> Result<Option<String>, String> { Err(NO_GIT.into()) }
    pub fn history(_: &GitFile, _: usize) -> Result<Vec<Commit>, String> { Err(NO_GIT.into()) }
}
#[cfg(target_arch = "wasm32")]
pub use web::{history, locate, read};
```

    Keep `verify` native-only (only `cli.rs` uses it). Keep the vcs tests native-only (`#[cfg(all(test, not(target_arch = "wasm32")))]`).
  - `persist.rs`: add the web `read` / `write`:

```rust
#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

#[cfg(target_arch = "wasm32")]
fn storage_key(key: &LayoutKey) -> String {
    match key {
        LayoutKey::Name(n) => format!("ernuance.layout.{n}"),
        LayoutKey::Path(p) => format!("ernuance.layout.{}", p.display()),
    }
}

#[cfg(target_arch = "wasm32")]
fn read(key: &LayoutKey) -> Option<String> {
    storage()?.get_item(&storage_key(key)).ok()?
}

#[cfg(target_arch = "wasm32")]
fn write(key: &LayoutKey, json: &str) -> Result<(), String> {
    // Private browsing or a full quota: report, never crash.
    let s = storage().ok_or("browser storage is not available")?;
    s.set_item(&storage_key(key), json).map_err(|_| "browser storage is full or blocked".to_owned())
}
```

    Mark `sidecar_path` `#[cfg(not(target_arch = "wasm32"))]`.
  - `picker.rs`: web `pick`:

```rust
#[cfg(target_arch = "wasm32")]
pub fn pick(&self, purpose: Purpose, ctx: &egui::Context) {
    let this = self.clone();
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new().add_filter("Mermaid ER", EXTENSIONS).pick_file().await else { return };
        let text = String::from_utf8_lossy(&file.read().await).into_owned();
        this.deliver(Picked { purpose, side: Side::Text { name: file.file_name(), text } });
        ctx.request_repaint();
    });
}
```

  - `app.rs`: add `const WEB: bool = cfg!(target_arch = "wasm32");`
    - native menu code is already `cfg(target_os = "macos")`;
    - hide on the web: "Reload" (File menu, palette), "Compare with HEAD" / "Compare with commit…"
      (already hidden: `self.git` is always `None` there), "Exit diff" when `current().path()` is
      `None` (true for all web sides; also correct natively for in-memory sides);
    - welcome screen: below the buttons, when `WEB`:
      `ui.label(RichText::new("Files are read in your browser and never uploaded.").weak());`
    - `ctx.send_viewport_cmd(Title…)` is harmless on the web (no-op), keep it.
  - Fix whatever else `cargo check --target wasm32-unknown-unknown` reports (e.g. `#[cfg]` on
    helpers only used natively such as `modified`) — no `#[allow(dead_code)]`.

- [ ] **Step 4: trunk entry** — `index.html`:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>ER Nuance</title>
  <meta name="description" content="Navigate and diff large Mermaid ER diagrams, right in your browser. Files never leave your computer." />
  <link data-trunk rel="rust" data-bin="ernuance" data-cargo-profile="web-release" data-wasm-opt="s" />
  <link data-trunk rel="icon" href="assets/logo.png" />
  <style>
    html, body { margin: 0; padding: 0; width: 100%; height: 100%; overflow: hidden; background: #1c1d21; }
    canvas { display: block; width: 100%; height: 100%; }
    #loading_text { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center;
      color: #e6e7ea; font: 16px system-ui, sans-serif; }
  </style>
</head>
<body>
  <canvas id="the_canvas_id"></canvas>
  <div id="loading_text">Loading ER Nuance…</div>
</body>
</html>
```

`Trunk.toml`:

```toml
[build]
target = "index.html"
dist = "target/web"
```

`.gitignore`: nothing to add (`/target` covers `target/web`).

- [ ] **Step 5: Verify**

Run:
```bash
cargo check --target wasm32-unknown-unknown
cargo clippy --target wasm32-unknown-unknown -- -D warnings
cargo test && cargo clippy --all-targets -- -D warnings
trunk build --release && ls -la target/web
```
Expected: all clean; `target/web` has `index.html`, `ernuance-<hash>.js`, `ernuance-<hash>_bg.wasm`.

Then `trunk serve --release --port 8089` (background) and open `http://127.0.0.1:8089` in a
browser (headless Chrome screenshot if available:
`"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --screenshot=target/web-shot.png --window-size=1400,900 --virtual-time-budget=8000 http://127.0.0.1:8089`).
Expected: the welcome screen renders (Open file…, Paste diagram…, Load example, the privacy line).
If it stays on "Loading…", check the console: if wgpu finds no WebGPU/WebGL adapter, enable
WebGL: add `wgpu = { version = "<eframe's wgpu version>", features = ["webgl"] }` to the web
dependencies and rebuild.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock index.html Trunk.toml src/
git commit -m "Web target: wasm build with trunk, localStorage layouts, async file dialog"
```

---

### Task 5: Docker image and cargo-make tasks

**Files:**
- Create: `Dockerfile`, `docker/nginx.conf`, `.dockerignore`, `Makefile.toml`

**Interfaces:**
- Consumes: `trunk build --release` → `target/web/`
- Produces: image `ghcr.io/smartnuance/er-nuance:<version>` serving on port 8080; tasks
  `web-serve`, `web-image`, `web-push`, `web-deploy`.

- [ ] **Step 1: `docker/nginx.conf`**

```nginx
server {
    listen 8080;
    server_name _;
    root /usr/share/nginx/html;
    index index.html;

    gzip on;
    gzip_min_length 1024;
    gzip_types application/wasm application/javascript text/css image/svg+xml;

    # trunk puts a content hash in every asset name: cache them for good.
    location ~* \.(?:wasm|js|css|png|ico|svg)$ {
        add_header Cache-Control "public, max-age=31536000, immutable";
        try_files $uri =404;
    }

    # The page itself must always be fresh, so new releases load.
    location / {
        add_header Cache-Control "no-cache";
        try_files $uri $uri/ /index.html;
    }
}
```

- [ ] **Step 2: `Dockerfile`** (the wasm build is platform-independent, so it runs on the build
machine's platform; only the nginx stage is amd64):

```dockerfile
# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG TRUNK_VERSION=0.21.14
RUN rustup target add wasm32-unknown-unknown \
 && arch="$(uname -m)" \
 && case "$arch" in aarch64) t=aarch64-unknown-linux-gnu ;; *) t=x86_64-unknown-linux-gnu ;; esac \
 && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${t}.tar.gz" | tar -xz -C /usr/local/bin
WORKDIR /src
COPY . .
RUN trunk build --release

FROM nginxinc/nginx-unprivileged:1.27-alpine
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /src/target/web /usr/share/nginx/html
EXPOSE 8080
```

`.dockerignore`:

```
target
.git
docs
*.layout.json
```

- [ ] **Step 3: `Makefile.toml`**

```toml
[config]
default_to_workspace = false

[env]
DOCKER_REGISTRY = "ghcr.io/smartnuance"
IMAGE_NAME = "er-nuance"
KUBE_CONTEXT = "contentnuance"

[tasks.web-serve]
description = "Run the web version locally with live rebuilds"
command = "trunk"
args = ["serve", "--open"]

[tasks.web-image]
description = "Build the web image (linux/amd64)"
script = '''
#!/bin/bash
set -euo pipefail
VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)
docker buildx build --platform linux/amd64 --load \
  -t "${DOCKER_REGISTRY}/${IMAGE_NAME}:${VERSION}" -t "${DOCKER_REGISTRY}/${IMAGE_NAME}:latest" .
'''

[tasks.web-push]
description = "Push the web image to ghcr"
dependencies = ["web-image"]
script = '''
#!/bin/bash
set -euo pipefail
VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)
docker push "${DOCKER_REGISTRY}/${IMAGE_NAME}:${VERSION}"
docker push "${DOCKER_REGISTRY}/${IMAGE_NAME}:latest"
'''

[tasks.web-deploy]
description = "Deploy the current version to the contentnuance cluster"
script = '''
#!/bin/bash
set -euo pipefail
VERSION=$(grep "^version" Cargo.toml | head -1 | cut -d'"' -f2)
sed -i.bak "s|image: .*er-nuance:.*|image: ${DOCKER_REGISTRY}/${IMAGE_NAME}:${VERSION}|" k8s/deployment.yaml
rm -f k8s/deployment.yaml.bak
kubectl --context="${KUBE_CONTEXT}" apply -k k8s/
kubectl --context="${KUBE_CONTEXT}" rollout status deployment/er-nuance -n er-nuance --timeout=90s
'''
```

- [ ] **Step 4: Verify the image locally** (no push)

```bash
cargo make web-image
docker run --rm -d --name ern-test -p 8090:8080 --read-only --tmpfs /tmp ghcr.io/smartnuance/er-nuance:0.1.0
sleep 2
curl -sI http://127.0.0.1:8090/ | grep -i -E "^HTTP|cache-control|content-type"
W=$(curl -s http://127.0.0.1:8090/ | grep -o 'ernuance-[a-f0-9]*_bg.wasm' | head -1)
curl -sI -H 'Accept-Encoding: gzip' "http://127.0.0.1:8090/$W" | grep -i -E "^HTTP|content-type|cache-control|content-encoding"
docker stop ern-test
```

Expected: `/` → `200`, `text/html`, `Cache-Control: no-cache`; the wasm → `200`,
`Content-Type: application/wasm`, `Cache-Control: public, max-age=31536000, immutable`,
`Content-Encoding: gzip`. Works with `--read-only --tmpfs /tmp` (as in the cluster).

- [ ] **Step 5: Commit**

```bash
git add Dockerfile docker/nginx.conf .dockerignore Makefile.toml
git commit -m "Docker image (nginx-unprivileged) and cargo-make web tasks"
```

---

### Task 6: Kubernetes manifests and docs

**Files:**
- Create: `k8s/namespace.yaml`, `k8s/deployment.yaml`, `k8s/service.yaml`, `k8s/ingress.yaml`,
  `k8s/kustomization.yaml`
- Modify: `README.md`

- [ ] **Step 1: Manifests**

`k8s/namespace.yaml`:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: er-nuance
```

`k8s/deployment.yaml`:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: er-nuance
  labels:
    app: er-nuance
spec:
  replicas: 1
  selector:
    matchLabels:
      app: er-nuance
  template:
    metadata:
      labels:
        app: er-nuance
    spec:
      securityContext:
        runAsNonRoot: true
      containers:
        - name: web
          image: ghcr.io/smartnuance/er-nuance:0.1.0
          imagePullPolicy: Always
          ports:
            - containerPort: 8080
              name: http
          resources:
            requests:
              cpu: 10m
              memory: 32Mi
            limits:
              cpu: 100m
              memory: 64Mi
          readinessProbe:
            httpGet:
              path: /
              port: http
            periodSeconds: 10
          livenessProbe:
            httpGet:
              path: /
              port: http
            initialDelaySeconds: 5
            periodSeconds: 30
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
          volumeMounts:
            - name: tmp
              mountPath: /tmp
      volumes:
        - name: tmp
          emptyDir: {}
```

`k8s/service.yaml`:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: er-nuance
  labels:
    app: er-nuance
spec:
  type: ClusterIP
  selector:
    app: er-nuance
  ports:
    - port: 80
      targetPort: http
      name: http
```

`k8s/ingress.yaml`:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: er-nuance
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt"
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
spec:
  ingressClassName: nginx
  tls:
    - hosts:
        - er.smartnuance.com
      secretName: er-nuance-tls
  rules:
    - host: er.smartnuance.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: er-nuance
                port:
                  number: 80
```

`k8s/kustomization.yaml`:

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: er-nuance

resources:
  - namespace.yaml
  - deployment.yaml
  - service.yaml
  - ingress.yaml
```

- [ ] **Step 2: Validate**

Run: `kubectl kustomize k8s/ | grep -E "^kind:|namespace:|image:|host:"`
Expected: Namespace, Service, Deployment, Ingress; every namespaced object in `er-nuance`; image
`ghcr.io/smartnuance/er-nuance:0.1.0`; host `er.smartnuance.com`.
Also run the deploy task's sed on a scratch copy to check it rewrites the image line:
`cp k8s/deployment.yaml /tmp/d.yaml && sed "s|image: .*er-nuance:.*|image: ghcr.io/smartnuance/er-nuance:9.9.9|" /tmp/d.yaml | grep image:`
→ `image: ghcr.io/smartnuance/er-nuance:9.9.9`.

- [ ] **Step 3: README** — add a "Web version" section after "Install":

```markdown
## Web version

ER Nuance also runs in the browser at **[er.smartnuance.com](https://er.smartnuance.com)**. Open,
drop or paste a diagram, or compare two files; everything happens in your browser and nothing is
uploaded. Layouts are remembered in the browser per file name. Git comparisons, reload and file
watching are desktop-only.

Run it locally with `cargo make web-serve` (needs `rustup target add wasm32-unknown-unknown` and
[trunk](https://trunkrs.dev)).

### Deploying

The image is a static nginx site (`Dockerfile`), the manifests are in `k8s/`.

1. `cargo make web-push` builds `ghcr.io/smartnuance/er-nuance:<version>` (linux/amd64) and pushes it.
   The ghcr package is public, so the cluster needs no pull secret.
2. `cargo make web-deploy` sets that version in `k8s/deployment.yaml` and applies `k8s/` to the
   `contentnuance` cluster.

One-time: a DNS A record for `er.smartnuance.com` pointing at the cluster's ingress load balancer.
cert-manager issues the TLS certificate on first deploy.
```

Also add `Paste diagram…` to the README sentence that lists ways to open a file
("…then File → Open, drag & drop, Paste diagram, or "Load example"").

- [ ] **Step 4: Commit**

```bash
git add k8s/ README.md
git commit -m "k8s manifests for er.smartnuance.com and web docs"
```

---

## Self-review notes

- Spec coverage: one crate two targets (T1–T4), in-memory source (T1), localStorage layouts (T2,
  T4), async dialog + drops (T3, T4), paste window (T3), compare with file on web (T3), hidden
  commands + privacy line (T4), trunk/profile (T4), Dockerfile/nginx/cache headers (T5), cargo-make
  (T5), k8s (T6), manual steps documented (T6), testing list (each task).
- Review Focus items map to: non-UTF-8 → T3 `non_utf8_and_non_mermaid_files_open_without_crashing`;
  cancel → T3 `cancelled_dialog_changes_nothing`; foreign layout → T2
  `a_layout_of_another_diagram_is_harmless`; empty paste → T3
  `pasting_opens_text_and_empty_paste_keeps_the_diagram`; compare on example → T3
  `compare_with_file_works_on_the_example`.
