//! Command line: `ernuance [FILE]` views a diagram, `ernuance diff …` compares two versions of one.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::vcs::{self, Rev, Side};

#[derive(Parser, Debug)]
#[command(name = "ernuance", version, about = "Navigate large Mermaid ER diagrams", args_conflicts_with_subcommands = true)]
struct Cli {
    /// Diagram to open (.mmd, or Markdown with ```mermaid blocks).
    file: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show what changed between two versions of a diagram.
    ///
    ///   ernuance diff OLD NEW         two files (also what `git difftool` calls)
    ///   ernuance diff FILE            HEAD → working tree
    ///   ernuance diff FILE REV        REV → working tree
    ///   ernuance diff FILE A..B       A → B
    ///   ernuance diff FILE --staged   HEAD → index
    ///
    /// As a git difftool:  git config difftool.ernuance.cmd 'ernuance diff "$LOCAL" "$REMOTE"'
    #[command(verbatim_doc_comment)]
    Diff {
        /// The diagram (in a git work tree unless SECOND is a file).
        file: PathBuf,
        /// A second file, a revision (`main`, `HEAD~2`), or a range `A..B`.
        second: Option<String>,
        /// Compare the index (staged changes) instead of the working tree.
        #[arg(long, alias = "cached")]
        staged: bool,
    },
}

#[derive(Debug, PartialEq)]
pub enum Launch {
    View(Option<PathBuf>),
    Diff { old: Side, new: Side },
}

/// Parses `args` (including the program name). Help, version and usage errors come back as
/// `clap::Error`, which prints and exits appropriately.
pub fn parse<I, T>(args: I) -> Result<Result<Launch, String>, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    Ok(match cli.command {
        None => Ok(Launch::View(cli.file)),
        Some(Command::Diff { file, second, staged }) => resolve_diff(&file, second.as_deref(), staged),
    })
}

fn resolve_diff(file: &Path, second: Option<&str>, staged: bool) -> Result<Launch, String> {
    if let Some(s) = second
        && !s.contains("..")
        && Path::new(s).is_file()
    {
        if staged {
            return Err("--staged compares git versions; it cannot be used with two files".into());
        }
        return Ok(Launch::Diff { old: Side::File(file.to_owned()), new: Side::File(s.into()) });
    }

    let git = vcs::locate(file).ok_or_else(|| match second {
        None => format!("{} is not in a git repository (give two files to compare them)", file.display()),
        Some(s) => format!("{s} is neither a file nor a revision: {} is not in a git repository", file.display()),
    })?;
    let current = if staged { Rev::Index } else { Rev::WorkTree };
    let (old, new) = match second {
        None => (Rev::Commit("HEAD".into()), current),
        Some(s) if s.contains("...") => {
            return Err(format!("`{s}`: symmetric ranges are not supported; use `git difftool -t ernuance {s} -- <file>`"));
        }
        Some(s) => match s.split_once("..") {
            Some(_) if staged => return Err("--staged cannot be combined with a range".into()),
            Some((a, b)) => {
                let rev = |r: &str| Rev::Commit(if r.is_empty() { "HEAD".into() } else { r.into() });
                (rev(a), rev(b))
            }
            None => (Rev::Commit(s.into()), current),
        },
    };
    for rev in [&old, &new] {
        if let Rev::Commit(c) = rev {
            vcs::verify(&git, c)?;
        }
    }
    Ok(Launch::Diff { old: Side::git(&git, old), new: Side::git(&git, new) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::tests::Fixture;

    fn run(args: &[&str]) -> Result<Launch, String> {
        parse(std::iter::once("ernuance").chain(args.iter().copied())).expect("valid usage")
    }

    fn revs(l: Launch) -> (Rev, Rev) {
        match l {
            Launch::Diff { old: Side::Git { rev: a, .. }, new: Side::Git { rev: b, .. } } => (a, b),
            other => panic!("not a git diff: {other:?}"),
        }
    }

    fn commit(s: &str) -> Rev {
        Rev::Commit(s.into())
    }

    #[test]
    fn plain_viewer() {
        assert_eq!(run(&[]).unwrap(), Launch::View(None));
        assert_eq!(run(&["x.mmd"]).unwrap(), Launch::View(Some("x.mmd".into())));
    }

    #[test]
    fn two_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mmd"), dir.path().join("b.mmd"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let l = run(&["diff", a.to_str().unwrap(), b.to_str().unwrap()]).unwrap();
        assert_eq!(l, Launch::Diff { old: Side::File(a.clone()), new: Side::File(b.clone()) });
        assert!(run(&["diff", a.to_str().unwrap(), b.to_str().unwrap(), "--staged"]).is_err());
        assert!(run(&["diff", a.to_str().unwrap()]).unwrap_err().contains("not in a git repository"));
    }

    #[test]
    fn git_forms() {
        let f = Fixture::new();
        f.commit("s.mmd", "1", "one");
        f.git(&["branch", "base"]);
        f.commit("s.mmd", "2", "two");
        let p = f.dir.path().join("s.mmd");
        let p = p.to_str().unwrap();
        assert_eq!(revs(run(&["diff", p]).unwrap()), (commit("HEAD"), Rev::WorkTree));
        assert_eq!(revs(run(&["diff", p, "--staged"]).unwrap()), (commit("HEAD"), Rev::Index));
        assert_eq!(revs(run(&["diff", p, "base"]).unwrap()), (commit("base"), Rev::WorkTree));
        assert_eq!(revs(run(&["diff", p, "--cached", "base"]).unwrap()), (commit("base"), Rev::Index));
        assert_eq!(revs(run(&["diff", p, "base..main"]).unwrap()), (commit("base"), commit("main")));
        assert_eq!(revs(run(&["diff", p, "HEAD~1.."]).unwrap()), (commit("HEAD~1"), commit("HEAD")));
        assert!(run(&["diff", p, "nope"]).unwrap_err().contains("unknown revision"));
        assert!(run(&["diff", p, "a...b"]).unwrap_err().contains("difftool"));
    }

    #[test]
    fn usage_errors_go_to_clap() {
        assert!(parse(["ernuance", "diff"]).is_err());
        assert!(parse(["ernuance", "--help"]).is_err());
    }
}
