//! The first start, before any settings exist: look for repositories in the
//! likely places, and turn what was found into a short list of roots to
//! confirm.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::repo::key;
use crate::scan::{self, Options};

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub root: PathBuf,
    pub count: usize,
    /// Off by default: a folder of downloaded dependencies, not your work.
    pub dependency: bool,
}

/// Folders below a drive root that hold the system or installed programs.
const SYSTEM_DIRS: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
    "users",
];

/// Folder names that mean "downloaded by a tool", compared in lower case.
const DEPENDENCY_DIRS: &[&str] = &[
    ".cargo",
    ".rustup",
    "node_modules",
    "appdata",
    "scoop",
    ".vscode",
    "pkg",
    ".nuget",
];

/// Where to look and how deep: the home folder, and every fixed drive.
fn places() -> Vec<(PathBuf, usize)> {
    let mut places = Vec::new();
    if let Some(dirs) = directories::BaseDirs::new() {
        places.push((dirs.home_dir().to_path_buf(), 4));
    }
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        // 3 is DRIVE_FIXED: no network shares, no card readers.
        if scan::drive_type(letter) == 3 {
            places.push((PathBuf::from(format!("{}:\\", letter as char)), 3));
        }
    }
    places
}

/// Scans the likely places and groups what it finds. Takes a few seconds.
pub fn discover() -> (Vec<Candidate>, Vec<(PathBuf, PathBuf)>) {
    let mut found = Vec::new();
    for (place, depth) in places() {
        let mut exclude: Vec<String> = crate::config::DEFAULT_EXCLUDE
            .iter()
            .map(|s| s.to_string())
            .collect();
        if place.parent().is_none() {
            // A drive root: skip the system, and Users, which the home folder covers.
            exclude.extend(SYSTEM_DIRS.iter().map(|s| s.to_string()));
        } else {
            exclude.push("AppData".into());
        }
        let options = Options {
            max_depth: depth,
            exclude,
        };
        let (more, _) = scan::collect(std::slice::from_ref(&place), &options);
        found.extend(more.into_iter().map(|(path, _)| (path, place.clone())));
    }
    let bases: Vec<PathBuf> = places().into_iter().map(|(p, _)| p).collect();
    let candidates = group(
        &found.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
        &bases,
    );
    // Each repository belongs to the candidate root that holds it.
    let found = found
        .into_iter()
        .filter_map(|(path, _)| {
            let k = key(&path);
            candidates
                .iter()
                .find(|c| crate::repo::key_within(&k, &key(&c.root)))
                .map(|c| (path, c.root.clone()))
        })
        .collect();
    (candidates, found)
}

/// Turns repository paths into a few roots: one per top-level folder of each
/// base, narrowed to the deepest folder that still holds all its repositories.
/// `~\src\github\app` and `~\src\scratch\demo` become `~\src`.
pub fn group(repos: &[PathBuf], bases: &[PathBuf]) -> Vec<Candidate> {
    let mut groups: BTreeMap<String, (PathBuf, Vec<PathBuf>)> = BTreeMap::new();
    for repo in repos {
        // The most specific base: the home folder rather than C:\.
        let Some(base) = bases
            .iter()
            .filter(|b| repo.starts_with(b))
            .max_by_key(|b| b.components().count())
        else {
            continue;
        };
        let Ok(rel) = repo.strip_prefix(base) else {
            continue;
        };
        let top = match rel.components().next() {
            Some(first) => base.join(first),
            None => repo.clone(),
        };
        groups
            .entry(key(&top))
            .or_insert_with(|| (top, Vec::new()))
            .1
            .push(repo.clone());
    }
    let mut out: Vec<Candidate> = groups
        .into_values()
        .map(|(top, members)| {
            let root = if members.len() == 1 && members[0] == top {
                top
            } else {
                common_parent(&members).unwrap_or(top)
            };
            Candidate {
                dependency: is_dependency(&root),
                count: members.len(),
                root,
            }
        })
        .collect();
    out.sort_by(|a, b| a.dependency.cmp(&b.dependency).then(b.count.cmp(&a.count)));
    out
}

/// The deepest folder holding every repository's parent.
fn common_parent(repos: &[PathBuf]) -> Option<PathBuf> {
    let mut parents = repos.iter().filter_map(|r| r.parent());
    let mut common: Vec<_> = parents.next()?.components().collect();
    for parent in parents {
        let shared = common
            .iter()
            .zip(parent.components())
            .take_while(|(a, b)| key(Path::new(a.as_os_str())) == key(Path::new(b.as_os_str())))
            .count();
        common.truncate(shared);
    }
    (!common.is_empty()).then(|| common.iter().collect())
}

fn is_dependency(root: &Path) -> bool {
    root.components().any(|c| {
        let name = c.as_os_str().to_string_lossy().to_lowercase();
        DEPENDENCY_DIRS.contains(&name.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_grouped_under_the_deepest_shared_folder() {
        let home = PathBuf::from("/home/user");
        let drive = PathBuf::from("/");
        let repos: Vec<PathBuf> = [
            "/home/user/src/github/app",
            "/home/user/src/github/tool",
            "/home/user/src/scratch/demo",
            "/home/user/notes/vault",
            "/home/user/dotfiles",
            "/home/user/.cargo/registry/src/a/crate1",
            "/home/user/.cargo/registry/src/b/crate2",
            "/work/app",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let groups = group(&repos, &[home, drive]);
        let shown: Vec<(String, usize, bool)> = groups
            .iter()
            .map(|c| {
                (
                    c.root.to_string_lossy().replace('\\', "/"),
                    c.count,
                    c.dependency,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("/home/user/src".to_string(), 3, false),
                ("/home/user/dotfiles".to_string(), 1, false),
                ("/home/user/notes".to_string(), 1, false),
                ("/work".to_string(), 1, false),
                ("/home/user/.cargo/registry/src".to_string(), 2, true),
            ]
        );
    }
}

#[cfg(test)]
mod probe {
    /// `cargo test probe -- --ignored --nocapture` shows what a first run
    /// would offer on this PC, and how long the look takes.
    #[test]
    #[ignore]
    fn discover_on_this_pc() {
        let started = std::time::Instant::now();
        let (candidates, found) = super::discover();
        eprintln!("{} repositories in {:?}", found.len(), started.elapsed());
        for c in candidates {
            eprintln!(
                "{:>4}  {}  {}",
                c.count,
                if c.dependency { "off" } else { "on " },
                c.root.display()
            );
        }
    }
}
