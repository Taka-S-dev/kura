//! Which repositories go in the hidden tab: full paths, which the action
//! menu adds and removes, and glob patterns written in the settings, as in
//! .gitignore (`*` within a folder name, `**` across folders).

use std::collections::HashSet;
use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::repo::key;

#[derive(Clone, Debug, Default)]
pub struct Rules {
    exact: HashSet<String>,
    /// The patterns as written, in the order `set` numbers them.
    patterns: Vec<String>,
    set: GlobSet,
}

impl Rules {
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut exact = HashSet::new();
        let mut patterns = Vec::new();
        let mut builder = GlobSetBuilder::new();
        for entry in entries {
            if !is_pattern(entry) {
                exact.insert(key(Path::new(&expand_home(entry))));
                continue;
            }
            let glob = GlobBuilder::new(&normalize(entry))
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|e| format!("hidden pattern {entry}: {e}"))?;
            builder.add(glob);
            patterns.push(entry.clone());
        }
        let set = builder.build().map_err(|e| e.to_string())?;
        Ok(Self {
            exact,
            patterns,
            set,
        })
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.exact.contains(&key(path)) || self.pattern(path).is_some()
    }

    /// The first pattern that hides `path`, as written in the settings.
    pub fn pattern(&self, path: &Path) -> Option<&str> {
        if self.patterns.is_empty() {
            return None;
        }
        let candidate = key(path).replace('\\', "/");
        self.set
            .matches(candidate.as_str())
            .first()
            .map(|&i| self.patterns[i].as_str())
    }

    pub fn hide(&mut self, path: &Path) {
        self.exact.insert(key(path));
    }

    pub fn show(&mut self, path: &Path) {
        self.exact.remove(&key(path));
    }
}

fn is_pattern(entry: &str) -> bool {
    entry.contains(['*', '?', '[', '{'])
}

fn expand_home(entry: &str) -> String {
    match (entry.strip_prefix('~'), directories::BaseDirs::new()) {
        (Some(rest), Some(dirs)) => format!("{}{rest}", dirs.home_dir().display()),
        _ => entry.to_string(),
    }
}

/// A pattern in the form candidates take: forward slashes, lower case on
/// Windows, and anchored nowhere unless it starts at a root, so `old_*`
/// means a folder of that name anywhere, as in .gitignore.
fn normalize(entry: &str) -> String {
    let mut pattern = expand_home(entry).replace('\\', "/");
    if cfg!(windows) {
        pattern = pattern.to_lowercase();
    }
    let pattern = pattern.trim_end_matches('/').to_string();
    let rooted = pattern.starts_with('/') || pattern.as_bytes().get(1) == Some(&b':');
    if rooted || pattern.starts_with("**/") {
        pattern
    } else {
        format!("**/{pattern}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(entries: &[&str]) -> Rules {
        Rules::parse(&entries.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn full_paths_and_patterns_both_hide() {
        let r = rules(&["/work/junk", "/work/C/**", "old_*", "vendor/*"]);
        assert!(r.contains(Path::new("/work/junk")));
        assert!(!r.contains(Path::new("/work/junkyard")));
        assert!(r.contains(Path::new("/work/C/linux")));
        assert!(r.contains(Path::new("/work/C/Libcurl/curl")));
        assert!(!r.contains(Path::new("/work/C#/shrun")));
        assert!(r.contains(Path::new("/dev/old_grepnavi")));
        assert!(!r.contains(Path::new("/dev/bold_idea")));
        assert!(r.contains(Path::new("/dev/app/vendor/lib")));
        assert_eq!(r.pattern(Path::new("/work/C/linux")), Some("/work/C/**"));
        assert_eq!(r.pattern(Path::new("/work/junk")), None);
    }

    #[test]
    fn windows_paths_match_without_regard_to_case_or_slashes() {
        if cfg!(windows) {
            let r = rules(&[r"C:\Users\Dev\Work\C\**", r"D:\Archive\Old"]);
            assert!(r.contains(Path::new(r"c:\users\dev\work\c\linux")));
            assert!(r.contains(Path::new(r"D:\archive\old")));
        }
    }

    #[test]
    fn menu_changes_touch_only_full_paths() {
        let mut r = rules(&["old_*"]);
        r.hide(Path::new("/a/b"));
        assert!(r.contains(Path::new("/a/b")));
        r.show(Path::new("/a/b"));
        assert!(!r.contains(Path::new("/a/b")));
        r.show(Path::new("/x/old_one"));
        assert!(r.contains(Path::new("/x/old_one")));
    }

    #[test]
    fn a_broken_pattern_is_reported() {
        let err = Rules::parse(&["[unclosed".to_string()]).unwrap_err();
        assert!(err.contains("[unclosed"));
    }
}
