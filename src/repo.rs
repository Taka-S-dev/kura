//! What kura knows about one repository, read straight from its `.git`
//! without running git: the branch from `HEAD`, the last change from the
//! index's modification time, and a language from the files at the top.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Repo {
    pub path: PathBuf,
    pub name: String,
    /// The checked-out branch, or `@` and a short hash on a detached HEAD.
    pub branch: Option<String>,
    pub modified: Option<SystemTime>,
    /// A linked worktree (`git worktree add`), whose `.git` is a file.
    pub worktree: bool,
    pub lang: Option<&'static str>,
}

impl Repo {
    pub fn inspect(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let (branch, modified, worktree) = match git_dir(&path) {
            Some((dir, worktree)) => (
                read_branch(&dir),
                mtime(&dir.join("index")).or_else(|| mtime(&dir.join("HEAD"))),
                worktree,
            ),
            None => (None, None, false),
        };
        let lang = detect_lang(&path);
        Self {
            path,
            name,
            branch,
            modified,
            worktree,
            lang,
        }
    }

    pub fn key(&self) -> String {
        key(&self.path)
    }
}

/// A path as a comparison key: Windows paths compare without regard to case
/// or which slash was typed.
pub fn key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    } else {
        text.trim_end_matches('/').to_string()
    }
}

/// Whether `inner` is `outer` or lies below it, both given as keys.
pub fn key_within(inner: &str, outer: &str) -> bool {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    inner == outer || (inner.starts_with(outer) && inner[outer.len()..].starts_with(sep))
}

/// Whether `path` is the top of a repository git would recognise. A `.git`
/// folder alone is not enough: tools such as Visual Studio leave `.git`
/// folders without a HEAD behind, and taking one for a repository hid the
/// real repositories below it.
pub fn is_repo(path: &Path) -> bool {
    let dot = path.join(".git");
    match fs::metadata(&dot) {
        Ok(meta) if meta.is_dir() => dot.join("HEAD").is_file(),
        Ok(_) => fs::read_to_string(&dot).is_ok_and(|text| text.starts_with("gitdir:")),
        Err(_) => false,
    }
}

/// The git directory and whether it belongs to a linked worktree. A `.git`
/// file points elsewhere: into `.git/worktrees/` for a worktree, into
/// `.git/modules/` for a submodule.
fn git_dir(repo: &Path) -> Option<(PathBuf, bool)> {
    let dot = repo.join(".git");
    let meta = fs::metadata(&dot).ok()?;
    if meta.is_dir() {
        return Some((dot, false));
    }
    let text = fs::read_to_string(&dot).ok()?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    let dir = repo.join(target);
    let worktree = dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "worktrees");
    Some((dir, worktree))
}

/// What the repository is, from the first paragraph of its README, for
/// the preview. Headings, badges, images and HTML are passed over, and
/// links and code marks are reduced to their text. Only the file is read.
pub fn about(repo: &Path) -> Option<String> {
    // The main README before translations such as README.ja.md.
    let rank = |name: &str| match name {
        "readme.md" => Some(0),
        "readme" | "readme.markdown" | "readme.rst" | "readme.txt" => Some(1),
        _ if name.starts_with("readme.") => Some(2),
        _ => None,
    };
    let readme = fs::read_dir(repo)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            rank(&name).map(|r| (r, name, e.path()))
        })
        .min()?
        .2;
    let mut text = String::new();
    fs::File::open(readme)
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    first_paragraph(&text)
}

fn first_paragraph(text: &str) -> Option<String> {
    let mut words: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut in_comment = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        // An HTML comment, such as a licence header, is not shown on GitHub.
        if in_comment || line.starts_with("<!--") {
            in_comment = !line.contains("-->");
            continue;
        }
        let skip = in_code
            || line.starts_with('#')
            || line.starts_with('<')
            || line.starts_with("![")
            || line.starts_with("[![")
            || line.starts_with('|')
            || line.starts_with("---")
            || line.starts_with("===");
        if line.is_empty() || skip {
            if !words.is_empty() && (line.is_empty() || line.starts_with('#')) {
                // A lone word, such as a title under a logo, is a heading.
                if words.len() == 1 && !words[0].contains(char::is_whitespace) {
                    words.clear();
                    continue;
                }
                break;
            }
            continue;
        }
        if let Some(text) = prose(line) {
            words.push(text);
        }
    }
    let about = words.join(" ");
    (!about.is_empty()).then_some(about)
}

/// A README line as text, or `None` for a line that is not prose: links
/// alone (`[English](README.en.md)`, download links), a bare URL, a row
/// of language names split by `|`, or ASCII art.
fn prose(line: &str) -> Option<String> {
    let line = strip_marker(line);
    let (text, outside_links) = plain(line);
    let text: String = text
        .split_whitespace()
        .filter(|w| !w.starts_with("http://") && !w.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ");
    let letters = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).count();
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    let links_only = letters(&outside_links) == 0;
    let menu = text.contains('|')
        && text
            .split('|')
            .all(|part| part.split_whitespace().count() <= 2);
    let art = letters(&text) * 2 < total;
    // A short line led by a link is a call to action, not a description:
    // "Download for Windows — installer or zip".
    let leads_with_link = line.trim_start_matches(['*', '_']).starts_with('[');
    let action = leads_with_link && text.chars().count() < 60;
    (letters(&text) > 0 && !links_only && !menu && !art && !action).then_some(text)
}

/// A line without its list, quote or numbered-list marker.
fn strip_marker(line: &str) -> &str {
    let line = line.trim_start_matches('>').trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest;
        }
    }
    match line.split_once(". ") {
        Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => line,
    }
}

/// Markdown reduced to its text: `[text](url)` to text, code marks and
/// emphasis dropped. Also the text outside any link, to tell a line of
/// links from a sentence that holds one.
fn plain(line: &str) -> (String, String) {
    let mut out = String::new();
    let mut outside = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        outside.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match (after.find(']'), after.find("](")) {
            (Some(close), Some(link)) if close == link => {
                out.push_str(&after[..close]);
                let tail = &after[close + 2..];
                rest = tail.find(')').map_or("", |end| &tail[end + 1..]);
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    outside.push_str(rest);
    let clean = |s: String| s.replace(['`', '*'], "").replace("__", "");
    (clean(out), clean(outside))
}

/// The config files git would read for this repository: the shared one
/// (a linked worktree's lives in the main repository) and the worktree's own.
pub fn config_files(repo: &Path) -> Vec<PathBuf> {
    let Some((dir, _)) = git_dir(repo) else {
        return Vec::new();
    };
    let common = fs::read_to_string(dir.join("commondir"))
        .map(|c| dir.join(c.trim()))
        .unwrap_or_else(|_| dir.clone());
    vec![common.join("config"), dir.join("config.worktree")]
}

fn read_branch(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    parse_head(&head)
}

fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(name) = head.strip_prefix("ref: refs/heads/") {
        return Some(name.to_string());
    }
    let hash = head.get(..7)?;
    hash.chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| format!("@{hash}"))
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A two-letter tag for the main language, from marker files at the top of
/// the repository. One directory read, no file contents.
fn detect_lang(repo: &Path) -> Option<&'static str> {
    let names: Vec<String> = fs::read_dir(repo)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    let has = |n: &str| names.iter().any(|x| x == n);
    let ends = |s: &str| names.iter().any(|x| x.ends_with(s));
    let lang = if has("cargo.toml") {
        "rs"
    } else if has("go.mod") {
        "go"
    } else if has("tsconfig.json") {
        "ts"
    } else if has("package.json") {
        "js"
    } else if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        "py"
    } else if ends(".sln") || ends(".csproj") {
        "cs"
    } else if has("cmakelists.txt") {
        "c"
    } else if has(".obsidian") {
        "md"
    } else {
        return None;
    };
    Some(lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_names_the_branch_or_a_short_hash() {
        assert_eq!(parse_head("ref: refs/heads/main\n"), Some("main".into()));
        assert_eq!(
            parse_head("ref: refs/heads/feat/tree"),
            Some("feat/tree".into())
        );
        assert_eq!(
            parse_head("3f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49\n"),
            Some("@3f2a9c1".into())
        );
        assert_eq!(parse_head("garbage"), None);
    }

    #[test]
    fn a_worktree_and_a_plain_repository_are_told_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            main.join(".git/worktrees/wt/HEAD"),
            "ref: refs/heads/feature\n",
        )
        .unwrap();
        fs::write(main.join("Cargo.toml"), "").unwrap();
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();

        let repo = Repo::inspect(main.clone());
        assert_eq!(repo.branch.as_deref(), Some("main"));
        assert!(!repo.worktree);
        assert_eq!(repo.lang, Some("rs"));

        let linked = Repo::inspect(wt);
        assert_eq!(linked.branch.as_deref(), Some("feature"));
        assert!(linked.worktree);
    }

    #[test]
    fn about_is_the_readme_s_first_paragraph_as_text() {
        let text = "# desk-buddy\n\n[![CI](x.svg)](y)\n<p align=\"center\"><img src=\"a.png\"></p>\n\nA desktop companion that reacts to your\n**posture** and [notifications](https://x).\n\n## Install\nrun `cargo install`\n";
        assert_eq!(
            first_paragraph(text).as_deref(),
            Some("A desktop companion that reacts to your posture and notifications.")
        );
        assert_eq!(first_paragraph("# Title only\n"), None);
        assert_eq!(
            first_paragraph("```\ncode\n```\nThen text.\n").as_deref(),
            Some("Then text.")
        );
        let commented = "<!--
Copyright (C) someone
-->

A tool.
";
        assert_eq!(first_paragraph(commented).as_deref(), Some("A tool."));
        // A title under a logo and a download line lead nothing.
        let readme = "  _ _ |_|\n\nREADME\n\n**[Download for Windows](x)** - installer or zip\n\nA tray app that collects prompts.\n";
        assert_eq!(
            first_paragraph(readme).as_deref(),
            Some("A tray app that collects prompts.")
        );
        // Lines that are not prose are passed over, wherever they come.
        let noisy = "English | 日本語\n\n[English](README.en.md)\n\n[Download for Windows](x) — [installer](y) / [zip](z)\n\nhttps://app.example.com/\n\n _   _ _| | |_) |\n\n- A tool that keeps notes.\n";
        assert_eq!(
            first_paragraph(noisy).as_deref(),
            Some("A tool that keeps notes.")
        );
        assert_eq!(
            first_paragraph("See [the docs](d) for more, it [works](w) well.\n").as_deref(),
            Some("See the docs for more, it works well.")
        );
    }

    /// `cargo test about_of_every_cached -- --ignored --nocapture` shows
    /// what the preview would say for the repositories kura knows here.
    #[test]
    #[ignore]
    fn about_of_every_cached_repository() {
        for entry in crate::cache::load() {
            let name = entry
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let about = about(&entry.path).unwrap_or_else(|| "-".into());
            eprintln!("{name:<24} {}", about.chars().take(90).collect::<String>());
        }
    }

    #[test]
    fn keys_ignore_case_and_trailing_separators_on_windows() {
        if cfg!(windows) {
            assert_eq!(key(Path::new(r"D:\Dev\Baton\")), r"d:\dev\baton");
            assert_eq!(key(Path::new("D:/dev/baton")), r"d:\dev\baton");
        }
        assert!(key_within(
            &key(Path::new("/a/b/c")),
            &key(Path::new("/a/b"))
        ));
        assert!(!key_within(
            &key(Path::new("/a/bc")),
            &key(Path::new("/a/b"))
        ));
    }
}
