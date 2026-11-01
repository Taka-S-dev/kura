//! Walks the configured roots in parallel and reports every directory that
//! holds a `.git`, without descending into it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use ignore::{WalkBuilder, WalkState};

pub enum Event {
    Found {
        path: PathBuf,
        root: PathBuf,
    },
    /// Every root has been walked. `reachable` lists the roots that could be
    /// opened, so repositories under a missing drive are kept, not dropped.
    Done {
        reachable: Vec<PathBuf>,
    },
}

#[derive(Clone)]
pub struct Options {
    pub max_depth: usize,
    pub exclude: Vec<String>,
}

pub fn spawn(
    roots: Vec<PathBuf>,
    options: Options,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reachable = Vec::new();
        for root in roots {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if !root.is_dir() || check_scan_root(&root).is_err() {
                continue;
            }
            reachable.push(root.clone());
            walk(&root, &options, &cancel, &tx);
        }
        if !cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Done { reachable });
        }
    })
}

/// Walks one root, sending what it finds. Blocks until the walk is over.
pub fn walk(root: &Path, options: &Options, cancel: &Arc<AtomicBool>, tx: &Sender<Event>) {
    let exclude: Arc<HashSet<String>> = Arc::new(options.exclude.iter().map(|n| fold(n)).collect());
    let mut builder = WalkBuilder::new(root);
    // .gitignore must not hide anything: a repository inside an ignored
    // folder is still a repository. Links are not followed, so a junction
    // such as "Application Data" cannot loop, while OneDrive folders, which
    // are reparse points but not links, are walked as usual.
    builder
        .standard_filters(false)
        .follow_links(false)
        .max_depth(Some(options.max_depth));
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        if !entry.file_type().is_some_and(|t| t.is_dir()) {
            return false;
        }
        let name = entry.file_name().to_string_lossy();
        !exclude.contains(&fold(&name)) && !name.starts_with('.')
    });
    let root = Arc::new(root.to_path_buf());
    builder.build_parallel().run(|| {
        let tx = tx.clone();
        let cancel = Arc::clone(cancel);
        let root = Arc::clone(&root);
        Box::new(move |result| {
            if cancel.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            // Folders that refuse to be read are passed over quietly.
            let Ok(entry) = result else {
                return WalkState::Continue;
            };
            if crate::repo::is_repo(entry.path()) {
                let found = Event::Found {
                    path: entry.into_path(),
                    root: root.as_ref().clone(),
                };
                if tx.send(found).is_err() {
                    return WalkState::Quit;
                }
                // Nested repositories are submodules or vendored copies,
                // which belong to the one already found.
                return WalkState::Skip;
            }
            WalkState::Continue
        })
    });
}

/// Folder names compare without case on Windows, as the file system does.
fn fold(name: &str) -> String {
    if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.to_string()
    }
}

/// Network drives are not walked: a whole-tree scan over SMB takes minutes
/// and can wake up a sleeping NAS.
#[cfg(windows)]
pub fn check_scan_root(root: &Path) -> Result<(), String> {
    use std::path::{Component, Prefix};
    let absolute = std::path::absolute(root).map_err(|e| e.to_string())?;
    let drive = match absolute.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err("network paths are not scanned".into()),
        },
        _ => return Err("cannot tell which drive this is".into()),
    };
    match drive_type(drive) {
        2 | 3 | 5 | 6 => Ok(()),
        _ => Err("network drives are not scanned".into()),
    }
}

#[cfg(not(windows))]
pub fn check_scan_root(_root: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub fn drive_type(drive: u8) -> u32 {
    let name = [drive as u16, b':' as u16, b'\\' as u16, 0];
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    // SAFETY: name is a valid, NUL-terminated UTF-16 drive root for this call.
    unsafe { GetDriveTypeW(name.as_ptr()) }
}

/// Runs a scan to the end on this thread and returns what it found.
pub fn collect(roots: &[PathBuf], options: &Options) -> (Vec<(PathBuf, PathBuf)>, Vec<PathBuf>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = spawn(roots.to_vec(), options.clone(), cancel, tx);
    let mut found = Vec::new();
    let mut reachable = Vec::new();
    for event in rx {
        match event {
            Event::Found { path, root } => found.push((path, root)),
            Event::Done { reachable: r } => reachable = r,
        }
    }
    let _ = handle.join();
    (found, reachable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo(path: &Path) {
        fs::create_dir_all(path.join(".git")).unwrap();
        fs::write(
            path.join(".git/HEAD"),
            "ref: refs/heads/main
",
        )
        .unwrap();
    }

    fn names(found: &[(PathBuf, PathBuf)]) -> Vec<String> {
        let mut names: Vec<String> = found
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn finds_repositories_and_worktrees_but_not_what_is_inside_or_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("dev/go/baton"));
        repo(&root.join("dev/rust/tally"));
        // A submodule inside a repository is part of it.
        repo(&root.join("dev/rust/tally/vendor/sub"));
        // A linked worktree has a .git file.
        fs::create_dir_all(root.join("dev/wt")).unwrap();
        fs::write(root.join("dev/wt/.git"), "gitdir: elsewhere").unwrap();
        repo(&root.join("dev/web/node_modules/pkg"));
        repo(&root.join(".cache/clone"));
        // Too deep for max_depth.
        repo(&root.join("a/b/c/d/e/deep"));
        // .gitignore must not hide a repository.
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        repo(&root.join("ignored/kept"));
        // A .git folder without HEAD is not a repository, and the walk
        // goes on below it.
        fs::create_dir_all(root.join("dev/vs/.git/objects")).unwrap();
        repo(&root.join("dev/vs/lib"));

        let options = Options {
            max_depth: 4,
            exclude: vec!["node_modules".into()],
        };
        let (found, reachable) = collect(&[root.to_path_buf()], &options);
        assert_eq!(names(&found), ["baton", "kept", "lib", "tally", "wt"]);
        assert_eq!(reachable, [root.to_path_buf()]);
    }

    #[test]
    fn a_missing_root_is_not_reachable() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("unplugged");
        let options = Options {
            max_depth: 4,
            exclude: vec![],
        };
        let (found, reachable) = collect(&[missing], &options);
        assert!(found.is_empty());
        assert!(reachable.is_empty());
    }

    #[test]
    fn a_root_that_is_itself_a_repository_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let options = Options {
            max_depth: 4,
            exclude: vec![],
        };
        let (found, _) = collect(&[tmp.path().to_path_buf()], &options);
        assert_eq!(found.len(), 1);
    }
}
