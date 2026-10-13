# Diff mode — design

Status: approved in conversation, 2026-10-13.

## Goal

Show what changed between two versions of an ER diagram (`.mmd`, or `.md` with mermaid blocks) on
the existing canvas. Primary use: reviewing a change (uncommitted edit, branch, PR). Secondary,
later: stepping through a file's history. The version picker is designed so history browsing can
grow out of it.

Success: one command on a changed schema shows the structural changes immediately, with the usual
navigation (groups, collapse, search, keyboard).

## Non-goals

- Textual (line) diff. git is used only to fetch the two texts; the diff is semantic.
- Rename detection (a renamed entity is removed + added).
- Writing to git, or hand-rolled git access (no parsing of `git` CLI output in product code).
- History scrubbing UI (later; the picker is the hook).

## CLI

```sh
er [file]                          # unchanged viewer
er diff OLD NEW                    # two files (what git difftool calls)
er diff FILE                       # HEAD ↔ working tree
er diff FILE REV                   # REV ↔ working tree
er diff FILE A..B                  # A ↔ B, both from git
er diff FILE --staged              # HEAD ↔ index
```

`er diff a b` means two files when both exist as files and no `..` is involved; otherwise the
second argument is a revision. git difftool setup, documented in the README:

```sh
git config difftool.er.cmd 'er diff "$LOCAL" "$REMOTE"'
git difftool -t er origin/main... -- schema.mmd
```

Errors (bad rev, path not in a repository when a rev is needed, unreadable file) are printed to
stderr with a non-zero exit before any window opens. A path that does not exist at a revision is not
an error: that side is an empty diagram (everything added/removed) and the status bar says so.

Parsing uses `clap` (derive).

## Units

| Unit | Job | Depends on |
|---|---|---|
| `src/diff.rs` (new, pure) | `diff(old: &Diagram, new: &Diagram) -> DiffResult { merged: Diagram, marks: DiffMarks }` | `model` |
| `src/vcs.rs` (new) | `Side`, `read(&Side) -> Result<Option<String>>`, `history(path) -> Result<Vec<Commit>>`, labels | `gix` |
| `src/cli.rs` (new) | argv → `Launch::{View(Option<PathBuf>), Diff { old: Side, new: Side }}` | `clap`, `vcs` |
| `scene.rs` | `diff: Option<DiffMarks>`; changed-attribute rows widen boxes | `diff` |
| `canvas.rs` | colors, strike-through, `old → new`, group badges | `diff` |
| `app.rs` | `Source::Diff`, live reload, changes panel, N / Shift+N, compare menu entries | all |

### `vcs::Side`

```rust
enum Side {
    File(PathBuf),                                  // plain file on disk
    Git { repo: PathBuf, path: PathBuf, rev: Rev }, // path relative to the work tree
}
enum Rev { WorkTree, Index, Commit(String) }        // Commit: anything rev-parse accepts
```

`File` and `Git{rev: WorkTree}` are polled for live reload. `read` returns `Ok(None)` when the path
does not exist at that revision. `history(path)` returns the commits (newest first, capped) whose
tree entry for the path differs from their first parent's. Library: `gix` (pure Rust), minimal
features.

## Diff semantics

- Entities match by id; attributes by name within an entity; relationships by unordered
  endpoint ids + label (duplicates matched in order).
- Status per item: `Same | Added | Removed | Changed`.
- Entity Changed: alias, description, or group differ, or any attribute is not Same.
- Attribute Changed: type, keys, or comment differ; the old attribute is kept for display.
- Relationship Changed: cardinalities or identifying flag differ (compared after orienting to the
  same endpoint order); the old relationship is kept.
- Merged diagram: new version's entities in order, then removed entities; within an entity, new
  attributes in order with removed ones inserted after their old predecessor. Relationships: new
  ones, then removed ones.
- Groups: union of group definitions. New version's membership wins; a removed entity keeps its old
  group. An entity whose explicit group changed is Changed with a note "moved from X". Group status
  is derived for display (counts of added/removed/changed members).
- Reordering, whitespace and comments that the parser ignores produce no difference.

## Rendering

- Added: green outline/header accent. Removed: red, faded, name struck through. Changed: amber.
- Field rows: added/removed rows colored; changed rows show `old → new` for the differing part.
- Relationships: edge colored by status; removed edges dashed-faded.
- Collapsed groups: badge `+a −r ~c` when any member differs.
- Sidebar in diff mode: a "Changes" list (entities, then relationships) above the entity list;
  click jumps.
- N / Shift+N: next / previous changed entity (expands its group, centers it).
- Status bar: `<old label> → <new label> · entities +a −r ~c · relationships …` (`→`, not `↔`: the UI font lacks `↔`).
- Window title: `ER Navigator — diff: <old label> → <new label> — <path>`.

## Layout and persistence

Positions/expansion are read from the sidecar of the new side's path (for `Git` sides: the work
tree file's sidecar). Diff mode never writes a sidecar. Removed entities are placed by the existing
`place_new` logic near their neighbours.

## Behaviour

- Live reload: every `File` / `WorkTree` side is polled by mtime; the diff is recomputed, keeping
  positions and open state (same mechanism as `reload`).
- In-app (palette + menu): "Compare with HEAD", "Compare with commit…" (lists `vcs::history`), "Exit
  diff". Outside a repository "Compare with commit…" is hidden and "Compare with file…" opens the
  file picker.

## Testing

- `diff.rs`: unit tests per change kind; reorder/whitespace produce no diff; group move.
- `vcs.rs`: tests against a temporary repository; fixture commits may be created by invoking the
  `git` binary in tests only.
- `cli.rs`: parse tests.
- One screenshot test rendering a diff (`cargo test screenshots -- --ignored`).
