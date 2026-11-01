//! A repository's main language, counted much as GitHub's Linguist counts
//! it, without its finer heuristics: the files git tracks, by extension and
//! weighed by size, with dependencies, generated files and dot folders
//! (.github, .obsidian, …) left out. Prose counts only when there is
//! nothing else, so a notes vault reads md and a project with a README
//! does not.

use std::collections::HashMap;
use std::path::Path;

use ignore::WalkBuilder;

/// Files looked at per repository, so a checkout the size of Linux still
/// takes a moment. Past this many, the first files git lists stand in for
/// the rest.
const MAX_FILES: usize = 4000;
/// A file counts for at most this many bytes, so one bundled or generated
/// file cannot outweigh the sources around it.
const MAX_BYTES: u64 = 256 * 1024;

/// Folders of dependencies and build output, a few of those Linguist's
/// vendored list covers.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "third_party",
    "dist",
    "build",
    "target",
    "out",
    "bin",
    "obj",
];

/// The tag for a file name, from its extension.
pub fn of_file(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") || lower.ends_with(".d.ts") {
        return None;
    }
    let ext = lower.rsplit_once('.')?.1;
    let tag = match ext {
        "rs" => "rs",
        "go" => "go",
        "ts" | "tsx" | "mts" | "cts" => "ts",
        "js" | "jsx" | "mjs" | "cjs" => "js",
        "svelte" => "sv",
        "vue" => "vu",
        "py" => "py",
        "cs" => "cs",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cp",
        "java" => "ja",
        "kt" | "kts" => "kt",
        "rb" => "rb",
        "php" => "ph",
        "swift" => "sw",
        "lua" => "lu",
        "ps1" | "psm1" | "psd1" => "ps",
        "sh" | "bash" | "zsh" => "sh",
        "html" | "htm" | "css" | "scss" => "ht",
        "md" | "markdown" => "md",
        _ => return None,
    };
    Some(tag)
}

/// The language with the most bytes of source in `repo`, or `None` when it
/// holds no file kura can name.
#[cfg(test)]
pub fn count(repo: &Path) -> Option<&'static str> {
    main(&mix(repo))
}

/// Languages with their share in percent, largest first.
pub type Mix = Vec<(&'static str, u8)>;

/// The main language of a mix: the first, which is the largest.
pub fn main(mix: &[(&'static str, u8)]) -> Option<&'static str> {
    mix.first().map(|(tag, _)| *tag)
}

/// The share of each language in `repo`, in percent, largest first, as
/// GitHub's language bar shows it. Prose is left out unless it is all
/// there is, so a notes vault reads md and a project with a README does
/// not.
pub fn mix(repo: &Path) -> Vec<(&'static str, u8)> {
    let mut bytes: HashMap<&'static str, u64> = HashMap::new();
    let mut add = |rel: &str, size: u64| {
        if let Some(tag) = counted(rel) {
            *bytes.entry(tag).or_default() += size.clamp(1, MAX_BYTES);
        }
    };
    match crate::gitstat::tracked_files(repo) {
        // What git tracks, as GitHub counts: files git ignores, or that
        // were never added, are not part of the project.
        Some(files) => {
            for rel in files
                .iter()
                .filter(|f| counted(f).is_some())
                .take(MAX_FILES)
            {
                let size = std::fs::metadata(repo.join(rel)).map_or(0, |m| m.len());
                add(rel, size);
            }
        }
        // Without git's list, the folder as .gitignore leaves it.
        None => {
            let walk = WalkBuilder::new(repo).build();
            let mut seen = 0;
            for entry in walk.filter_map(Result::ok) {
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(repo) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if counted(&rel).is_none() {
                    continue;
                }
                add(&rel, entry.metadata().map_or(0, |m| m.len()));
                seen += 1;
                if seen >= MAX_FILES {
                    break;
                }
            }
        }
    }
    if bytes.keys().any(|tag| *tag != "md") {
        bytes.remove("md");
    }
    shares(bytes)
}

/// Bytes per language as whole percents, largest first; ties go to the
/// name, so the order is the same on every run.
fn shares(bytes: HashMap<&'static str, u64>) -> Vec<(&'static str, u8)> {
    let total: u64 = bytes.values().sum();
    if total == 0 {
        return Vec::new();
    }
    let mut out: Vec<(&'static str, u64)> = bytes.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    out.into_iter()
        .map(|(tag, n)| (tag, ((n * 100 + total / 2) / total) as u8))
        .collect()
}

/// The tag a file counts for, by its path relative to the repository:
/// none inside dependency, build or dot folders.
fn counted(rel: &str) -> Option<&'static str> {
    let mut parts: Vec<&str> = rel.split(['/', '\\']).collect();
    let name = parts.pop()?;
    let skipped = parts
        .iter()
        .any(|dir| dir.starts_with('.') || SKIP_DIRS.contains(&dir.to_lowercase().as_str()));
    if skipped { None } else { of_file(name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn file(root: &Path, path: &str, size: usize) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x".repeat(size)).unwrap();
    }

    #[test]
    fn the_language_with_most_source_wins_and_dependencies_do_not_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // TypeScript sources with a package.json and no tsconfig at the top.
        file(root, "package.json", 400);
        file(root, "src/app.ts", 3000);
        file(root, "src/view.tsx", 2000);
        file(root, "scripts/build.js", 800);
        file(root, "node_modules/lib/index.js", 90_000);
        file(root, "public/app.min.js", 90_000);
        file(root, "README.md", 9000);
        assert_eq!(count(root), Some("ts"));
    }

    #[test]
    fn markers_are_not_needed_and_prose_counts_only_alone() {
        let tmp = tempfile::tempdir().unwrap();
        file(tmp.path(), "lesson1/main.go", 500);
        assert_eq!(count(tmp.path()), Some("go"));

        let notes = tempfile::tempdir().unwrap();
        file(notes.path(), "daily/2026-11-01.md", 500);
        assert_eq!(count(notes.path()), Some("md"));

        let empty = tempfile::tempdir().unwrap();
        file(empty.path(), "data.bin", 500);
        assert_eq!(count(empty.path()), None);
    }

    #[test]
    fn gitignored_files_do_not_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        file(root, ".gitignore", 10);
        fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        file(root, "generated/big.py", 100_000);
        file(root, "init.lua", 200);
        assert_eq!(count(root), Some("lu"));
    }

    /// `cargo test --release lang_of_every_cached -- --ignored --nocapture`
    /// compares the marker-file guess with the count for the repositories
    /// kura knows here.
    #[test]
    #[ignore]
    fn lang_of_every_cached_repository() {
        let started = std::time::Instant::now();
        for entry in crate::cache::load() {
            let guess = crate::repo::Repo::inspect(entry.path.clone()).lang;
            let counted = count(&entry.path);
            let mark = if guess == counted { " " } else { "*" };
            eprintln!(
                "{mark} {:>3} -> {:>3}  {}",
                guess.unwrap_or("-"),
                counted.unwrap_or("-"),
                entry.path.file_name().unwrap().to_string_lossy()
            );
        }
        eprintln!("{:?}", started.elapsed());
    }

    #[test]
    fn only_what_git_tracks_counts_and_dot_folders_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .status();
        if !init.is_ok_and(|s| s.success()) {
            return;
        }
        file(root, "tools/install.ps1", 300);
        file(root, ".obsidian/plugins/x/main.js", 50_000);
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "tools/install.ps1", ".obsidian"])
            .status()
            .unwrap();
        // Never added: not part of the project.
        file(root, "scratch/main.go", 50_000);
        assert_eq!(count(root), Some("ps"));
    }

    #[test]
    fn shares_are_percents_largest_first() {
        let mut bytes = HashMap::new();
        bytes.insert("ts", 720);
        bytes.insert("rs", 210);
        bytes.insert("ht", 70);
        assert_eq!(shares(bytes), [("ts", 72), ("rs", 21), ("ht", 7)]);
        assert!(shares(HashMap::new()).is_empty());
    }

    #[test]
    fn extensions_map_to_tags() {
        assert_eq!(of_file("Main.CPP"), Some("cp"));
        assert_eq!(of_file("profile.ps1"), Some("ps"));
        assert_eq!(of_file("index.html"), Some("ht"));
        assert_eq!(of_file("types.d.ts"), None);
        assert_eq!(of_file("Makefile"), None);
        assert_eq!(of_file("App.svelte"), Some("sv"));
        assert_eq!(counted(".github/workflows/x.js"), None);
        assert_eq!(counted("src/node_modules/x.js"), None);
        assert_eq!(counted("src/app.ts"), Some("ts"));
    }
}
