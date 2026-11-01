//! The last scan's results, so the list appears at once while a fresh scan
//! runs behind it. Only where repositories are: their state goes stale too
//! quickly to be worth showing from a file.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::repo::key;

type Error = Box<dyn std::error::Error>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub root: PathBuf,
    pub last_seen: u64,
    /// The languages once counted, with their share in percent, largest
    /// first, so the list and the preview show them at once. Empty when
    /// counted and nothing was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub langs: Option<Vec<(String, u8)>>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    repos: Vec<Entry>,
}

const VERSION: u32 = 1;

pub fn path() -> Result<PathBuf, Error> {
    path_for(crate::config::profile().as_deref())
}

/// The cache of one profile: each searches its own folders, so each keeps
/// its own cache. Under `KURA_CONFIG_DIR` it sits beside the settings.
pub fn path_for(profile: Option<&str>) -> Result<PathBuf, Error> {
    let file = match profile {
        Some(name) => format!("cache-{name}.json"),
        None => "cache.json".into(),
    };
    if let Some(dir) = crate::config::override_dir() {
        return Ok(dir.join("cache").join(file));
    }
    let dirs = directories::BaseDirs::new().ok_or("cannot find the home directory")?;
    Ok(dirs.cache_dir().join("kura").join(file))
}

/// The cached repositories, or none when the file is missing or unreadable.
/// A broken cache only costs a full scan, so it is never an error.
pub fn load() -> Vec<Entry> {
    let Ok(path) = path() else {
        return Vec::new();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<File>(&text).ok())
        .filter(|file| file.version == VERSION)
        .map(|file| file.repos)
        .unwrap_or_default()
}

pub fn save(entries: &[Entry]) -> Result<(), Error> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = File {
        version: VERSION,
        repos: entries.to_vec(),
    };
    crate::config::write_atomic(&path, &serde_json::to_string(&file)?)?;
    Ok(())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The cache after a completed scan. Everything found is kept. A cached
/// repository that was not found is dropped only when its root could be
/// walked: a root on an unplugged drive keeps its repositories until the
/// drive is back. Roots no longer configured take their repositories along.
pub fn merge(
    old: &[Entry],
    found: &[(PathBuf, PathBuf)],
    roots: &[PathBuf],
    reachable: &[PathBuf],
    now: u64,
) -> Vec<Entry> {
    let roots: HashSet<String> = roots.iter().map(|r| key(r)).collect();
    let reachable: HashSet<String> = reachable.iter().map(|r| key(r)).collect();
    let counted: std::collections::HashMap<String, &Vec<(String, u8)>> = old
        .iter()
        .filter_map(|e| e.langs.as_ref().map(|l| (key(&e.path), l)))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (path, root) in found {
        if seen.insert(key(path)) {
            out.push(Entry {
                path: path.clone(),
                root: root.clone(),
                last_seen: now,
                // A language counted before stays until counted again.
                langs: counted.get(&key(path)).map(|l| (*l).clone()),
            });
        }
    }
    for entry in old {
        let root = key(&entry.root);
        if roots.contains(&root) && !reachable.contains(&root) && seen.insert(key(&entry.path)) {
            out.push(entry.clone());
        }
    }
    out
}

pub fn entries_from(found: &[(PathBuf, PathBuf)], now: u64) -> Vec<Entry> {
    merge(&[], found, &[], &[], now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, root: &str) -> Entry {
        Entry {
            path: path.into(),
            root: root.into(),
            last_seen: 1,
            langs: None,
        }
    }

    #[test]
    fn repositories_under_an_unreachable_root_are_kept() {
        let old = [
            entry("/d/dev/gone", "/d/dev"),
            entry("/e/usb/kept", "/e/usb"),
            entry("/x/dropped/repo", "/x/dropped"),
        ];
        let found = [(PathBuf::from("/d/dev/new"), PathBuf::from("/d/dev"))];
        let roots = [PathBuf::from("/d/dev"), PathBuf::from("/e/usb")];
        let reachable = [PathBuf::from("/d/dev")];
        let merged = merge(&old, &found, &roots, &reachable, 9);
        let paths: Vec<_> = merged.iter().map(|e| e.path.clone()).collect();
        // gone: its root was walked and it was not there.
        // dropped: its root is no longer configured.
        assert_eq!(
            paths,
            [PathBuf::from("/d/dev/new"), PathBuf::from("/e/usb/kept")]
        );
        assert_eq!(merged[0].last_seen, 9);
    }
}
