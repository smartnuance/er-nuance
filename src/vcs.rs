//! Where one side of a diff comes from: a plain file, or a file at some git revision.
//!
//! git access goes through `gix` only (read-only): resolving revisions, reading blobs from
//! commits or the index, and listing the commits that touched a file.

use std::path::{Path, PathBuf};

/// A file inside a git work tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitFile {
    /// The work tree root.
    pub root: PathBuf,
    /// Relative to `root`.
    pub path: PathBuf,
}

impl GitFile {
    pub fn disk_path(&self) -> PathBuf {
        self.root.join(&self.path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rev {
    WorkTree,
    Index,
    /// Anything `git rev-parse` accepts: `HEAD`, `main`, `HEAD~3`, a hash.
    Commit(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    File(PathBuf),
    Git { file: GitFile, rev: Rev },
}

impl Side {
    pub fn git(file: &GitFile, rev: Rev) -> Self {
        Self::Git { file: file.clone(), rev }
    }

    /// Short description for the status bar and window title.
    pub fn label(&self) -> String {
        match self {
            Self::File(p) => p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned(),
            Self::Git { rev: Rev::WorkTree, .. } => "working tree".to_owned(),
            Self::Git { rev: Rev::Index, .. } => "index".to_owned(),
            Self::Git { rev: Rev::Commit(c), .. } => short_rev(c),
        }
    }

    /// The file on disk that can change while we look at it (polled for live reload).
    pub fn live_path(&self) -> Option<PathBuf> {
        match self {
            Self::File(p) => Some(p.clone()),
            Self::Git { file, rev: Rev::WorkTree } => Some(file.disk_path()),
            Self::Git { .. } => None,
        }
    }

    /// The path whose layout sidecar this side uses.
    pub fn layout_path(&self) -> PathBuf {
        match self {
            Self::File(p) => p.clone(),
            Self::Git { file, .. } => file.disk_path(),
        }
    }
}

/// Full hashes are shortened; names like `main` or `HEAD~2` are kept.
fn short_rev(rev: &str) -> String {
    if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) { rev[..8].to_owned() } else { rev.to_owned() }
}

/// The work tree and relative path of `path`, if it lies in a git work tree.
pub fn locate(path: &Path) -> Option<GitFile> {
    let abs = std::path::absolute(path).ok()?;
    let (dir, name) = (abs.parent()?, abs.file_name()?);
    let dir = dir.canonicalize().ok()?;
    let repo = gix::discover(&dir).ok()?;
    let root = repo.workdir()?.canonicalize().ok()?;
    let rel = dir.strip_prefix(&root).ok()?.join(name);
    Some(GitFile { root, path: rel })
}

/// The text of a side. `Ok(None)` when the file does not exist there (e.g. not yet committed).
pub fn read(side: &Side) -> Result<Option<String>, String> {
    let bytes = match side {
        Side::File(p) => Some(std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?),
        Side::Git { file, rev: Rev::WorkTree } => match std::fs::read(file.disk_path()) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("cannot read {}: {e}", file.disk_path().display())),
        },
        Side::Git { file, rev: Rev::Index } => {
            let repo = open(file)?;
            let index = repo.open_index().map_err(|e| format!("cannot read the git index: {e}"))?;
            match index.entry_by_path(unix_path(&file.path).as_str().into()) {
                None => None,
                Some(entry) => Some(repo.find_object(entry.id).map_err(|e| e.to_string())?.data.clone()),
            }
        }
        Side::Git { file, rev: Rev::Commit(spec) } => {
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

/// Checks that a revision exists, for early error reporting.
pub fn verify(file: &GitFile, spec: &str) -> Result<(), String> {
    commit_tree(&open(file)?, spec).map(drop)
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub summary: String,
    /// `YYYY-MM-DD`.
    pub date: String,
}

impl Commit {
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

/// Commits reachable from HEAD that changed `file`, newest first, at most `limit`.
pub fn history(file: &GitFile, limit: usize) -> Result<Vec<Commit>, String> {
    const MAX_WALK: usize = 20_000;
    let repo = open(file)?;
    let head = repo.head_id().map_err(|e| format!("no HEAD: {e}"))?;
    let walk = repo
        .rev_walk([head.detach()])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default()))
        .all()
        .map_err(|e| e.to_string())?;
    let blob_at = |c: &gix::Commit<'_>| -> Option<gix::ObjectId> {
        c.tree().ok()?.lookup_entry_by_path(&file.path).ok()?.map(|e| e.object_id())
    };
    let mut out = Vec::new();
    for info in walk.take(MAX_WALK) {
        let info = info.map_err(|e| e.to_string())?;
        let commit = info.object().map_err(|e| e.to_string())?;
        let mine = blob_at(&commit);
        let parent = info.parent_ids().next().and_then(|p| p.object().ok()?.try_into_commit().ok());
        let theirs = parent.as_ref().and_then(blob_at);
        if mine == theirs {
            continue;
        }
        let summary = commit.message().map(|m| m.summary().to_string()).unwrap_or_default();
        let date = commit
            .time()
            .ok()
            .and_then(|t| t.format(gix::date::time::format::SHORT).ok())
            .unwrap_or_default();
        out.push(Commit { id: info.id.to_string(), summary, date });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn open(file: &GitFile) -> Result<gix::Repository, String> {
    gix::open(&file.root).map_err(|e| format!("cannot open git repository at {}: {e}", file.root.display()))
}

fn commit_tree<'r>(repo: &'r gix::Repository, spec: &str) -> Result<gix::Tree<'r>, String> {
    let id = repo.rev_parse_single(spec).map_err(|e| format!("unknown revision `{spec}`: {e}"))?;
    let object = id.object().map_err(|e| e.to_string())?;
    let commit = object.peel_to_commit().map_err(|e| format!("`{spec}` is not a commit: {e}"))?;
    commit.tree().map_err(|e| e.to_string())
}

fn unix_path(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::process::Command;

    /// A scratch repository; fixture commits are made with the `git` binary (tests only).
    pub struct Fixture {
        pub dir: tempfile::TempDir,
    }

    impl Fixture {
        pub fn new() -> Self {
            let f = Self { dir: tempfile::tempdir().unwrap() };
            f.git(&["init", "-q", "-b", "main"]);
            f
        }

        pub fn git(&self, args: &[&str]) {
            let ok = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(self.dir.path())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }

        pub fn write(&self, rel: &str, text: &str) {
            let p = self.dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }

        pub fn commit(&self, rel: &str, text: &str, msg: &str) {
            self.write(rel, text);
            self.git(&["add", rel]);
            self.git(&["commit", "-q", "-m", msg]);
        }

        pub fn file(&self, rel: &str) -> GitFile {
            locate(&self.dir.path().join(rel)).expect("in the repository")
        }
    }

    #[test]
    fn reads_every_kind_of_side() {
        let f = Fixture::new();
        f.commit("db/schema.mmd", "one", "first");
        f.commit("db/schema.mmd", "two", "second");
        f.write("db/schema.mmd", "staged");
        f.git(&["add", "db/schema.mmd"]);
        f.write("db/schema.mmd", "edited");

        let file = f.file("db/schema.mmd");
        assert_eq!(file.path, Path::new("db/schema.mmd"));
        let read = |rev| read(&Side::git(&file, rev)).unwrap();
        assert_eq!(read(Rev::WorkTree).as_deref(), Some("edited"));
        assert_eq!(read(Rev::Index).as_deref(), Some("staged"));
        assert_eq!(read(Rev::Commit("HEAD".into())).as_deref(), Some("two"));
        assert_eq!(read(Rev::Commit("HEAD~1".into())).as_deref(), Some("one"));
        assert_eq!(read(Rev::Commit("main".into())).as_deref(), Some("two"));
        assert!(super::read(&Side::git(&file, Rev::Commit("nope".into()))).is_err());
        assert!(verify(&file, "HEAD~1").is_ok() && verify(&file, "nope").is_err());
    }

    #[test]
    fn missing_file_at_a_revision_is_none() {
        let f = Fixture::new();
        f.commit("other.txt", "x", "first");
        f.write("new.mmd", "erDiagram");
        let file = f.file("new.mmd");
        assert_eq!(read(&Side::git(&file, Rev::Commit("HEAD".into()))).unwrap(), None);
        assert_eq!(read(&Side::git(&file, Rev::Index)).unwrap(), None);
    }

    #[test]
    fn history_lists_commits_touching_the_file() {
        let f = Fixture::new();
        f.commit("a.mmd", "1", "add a");
        f.commit("b.txt", "x", "unrelated");
        f.commit("a.mmd", "2", "change a");
        let h = history(&f.file("a.mmd"), 50).unwrap();
        let summaries: Vec<&str> = h.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["change a", "add a"]);
        assert_eq!(h[0].date.len(), 10);
        assert_eq!(read(&Side::git(&f.file("a.mmd"), Rev::Commit(h[1].id.clone()))).unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn outside_a_repository_there_is_no_git_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.mmd"), "").unwrap();
        assert_eq!(locate(&dir.path().join("x.mmd")), None);
    }

    #[test]
    fn labels() {
        let file = GitFile { root: "/r".into(), path: "s.mmd".into() };
        assert_eq!(Side::git(&file, Rev::WorkTree).label(), "working tree");
        assert_eq!(Side::git(&file, Rev::Commit("a".repeat(40))).label(), "aaaaaaaa");
        assert_eq!(Side::git(&file, Rev::Commit("main".into())).label(), "main");
        assert_eq!(Side::File("/tmp/x/old.mmd".into()).label(), "old.mmd");
    }
}
