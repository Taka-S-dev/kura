//! The order of the list: zoxide's history when the query is empty, fuzzy
//! matching when it is not, and when to skip the list and jump straight in.

use std::cmp::Reverse;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

use crate::repo::{Repo, key, key_within};

/// zoxide's remembered directories with their scores, as path keys.
pub fn zoxide_scores() -> Vec<(f64, String)> {
    let mut command = std::process::Command::new("zoxide");
    command.args(["query", "--list", "--score"]);
    crate::launch::no_window(&mut command);
    let Ok(output) = command.output() else {
        // No zoxide: the list falls back to the last change alone.
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (score, path) = line.trim_start().split_once(' ')?;
            Some((score.parse().ok()?, key(std::path::Path::new(path.trim()))))
        })
        .collect()
}

/// Each repository's share of zoxide's history: the scores of the repository
/// and every folder below it, so time spent in `src\` counts too.
pub fn repo_scores(repos: &[Repo], zoxide: &[(f64, String)]) -> Vec<f64> {
    repos
        .iter()
        .map(|repo| {
            let k = repo.key();
            zoxide
                .iter()
                .filter(|(_, path)| key_within(path, &k))
                .map(|(score, _)| score)
                .sum()
        })
        .collect()
}

/// How the list is ordered before a query narrows it, and how equal
/// matches are ordered after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// By zoxide's history: where you spend time.
    Used,
    /// Most recently changed first.
    Changed,
    Name,
}

impl Sort {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "used" => Some(Sort::Used),
            "changed" => Some(Sort::Changed),
            "name" => Some(Sort::Name),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Sort::Used => Sort::Changed,
            Sort::Changed => Sort::Name,
            Sort::Name => Sort::Used,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Used => "by use",
            Sort::Changed => "by change",
            Sort::Name => "by name",
        }
    }
}

/// Indices into `repos` in `sort` order. Ties fall back to the next key:
/// use, then the last change, then the name.
pub fn base_order(repos: &[Repo], scores: &[f64], sort: Sort) -> Vec<usize> {
    let mut order: Vec<usize> = (0..repos.len()).collect();
    let used = |a: usize, b: usize| scores[b].total_cmp(&scores[a]);
    let changed = |a: usize, b: usize| repos[b].modified.cmp(&repos[a].modified);
    let name = |a: usize, b: usize| {
        repos[a]
            .name
            .to_lowercase()
            .cmp(&repos[b].name.to_lowercase())
    };
    order.sort_by(|&a, &b| match sort {
        Sort::Used => used(a, b)
            .then_with(|| changed(a, b))
            .then_with(|| name(a, b)),
        Sort::Changed => changed(a, b).then_with(|| name(a, b)),
        Sort::Name => name(a, b),
    });
    order
}

/// The language tags `Repo::lang` can hold.
pub const LANGS: &[&str] = &[
    "rs", "go", "ts", "js", "py", "cs", "c", "cp", "ja", "kt", "rb", "ph", "sw", "lu", "ps", "sh",
    "ht", "sv", "vu", "md",
];

/// Splits a query into language filters (`rs:`, `py:`) and the text to
/// match. Several filters keep any of their languages; a word that only
/// looks like one, such as `foo:`, stays text.
pub fn parse_query(query: &str) -> (Vec<&'static str>, String) {
    let mut langs = Vec::new();
    let mut text = Vec::new();
    for word in query.split_whitespace() {
        let tag = word.strip_suffix(':').map(str::to_lowercase);
        match tag.and_then(|t| LANGS.iter().find(|l| **l == t)) {
            Some(lang) => langs.push(*lang),
            None => text.push(word),
        }
    }
    (langs, text.join(" "))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub index: usize,
    /// Matched character positions in the name, for highlighting.
    pub name_hits: Vec<u32>,
}

pub struct Searcher {
    matcher: Matcher,
    path_matcher: Matcher,
}

impl Searcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            path_matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    /// The repositories matching `query`, best first; ties keep the base
    /// order. A single word is looked for in names only: fuzzy matching over
    /// whole paths lets almost any word match everything under the home
    /// folder. Several words, or a separator, search the path as well, so
    /// `obsidian notes` tells two repositories called notes apart. A match
    /// in the name still comes first.
    pub fn search(&mut self, repos: &[Repo], order: &[usize], query: &str) -> Vec<Hit> {
        let (langs, query) = parse_query(query);
        let order: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| langs.is_empty() || repos[i].lang.is_some_and(|l| langs.contains(&l)))
            .collect();
        let query = query.as_str();
        if query.is_empty() {
            return order
                .iter()
                .map(|&index| Hit {
                    index,
                    name_hits: Vec::new(),
                })
                .collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let with_path = query.trim().contains([' ', '\\', '/']);
        let mut buf = Vec::new();
        let mut scored = Vec::new();
        for &index in &order {
            let repo = &repos[index];
            let mut hits = Vec::new();
            let name = Utf32Str::new(&repo.name, &mut buf);
            if let Some(score) = pattern.indices(name, &mut self.matcher, &mut hits) {
                hits.sort_unstable();
                hits.dedup();
                scored.push((
                    1u8,
                    score,
                    Hit {
                        index,
                        name_hits: hits,
                    },
                ));
                continue;
            }
            if !with_path {
                continue;
            }
            let path = repo.path.to_string_lossy();
            let path = Utf32Str::new(&path, &mut buf);
            if let Some(score) = pattern.score(path, &mut self.path_matcher) {
                scored.push((
                    0u8,
                    score,
                    Hit {
                        index,
                        name_hits: Vec::new(),
                    },
                ));
            }
        }
        // Stable, so equal scores keep the chosen order.
        scored.sort_by_key(|(in_name, score, _)| (Reverse(*in_name), Reverse(*score)));
        scored.into_iter().map(|(_, _, hit)| hit).collect()
    }
}

/// The repository to jump to without showing the list, if the query leaves
/// no doubt: exactly one repository has that name, or exactly one matches at
/// all. When in doubt the list opens, since landing in the wrong place is
/// worse than one look at the list.
pub fn direct(repos: &[Repo], hits: &[Hit], query: &str) -> Option<usize> {
    let wanted = parse_query(query).1.to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let mut named = hits
        .iter()
        .filter(|h| repos[h.index].name.to_lowercase() == wanted);
    if let (Some(only), None) = (named.next(), named.next()) {
        return Some(only.index);
    }
    match hits {
        [only] => Some(only.index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn repo(path: &str, age: u64) -> Repo {
        let path = PathBuf::from(path);
        Repo {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            branch: None,
            modified: Some(UNIX_EPOCH + Duration::from_secs(1_000_000 - age)),
            worktree: false,
            lang: None,
        }
    }

    fn names(repos: &[Repo], hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| repos[h.index].name.clone()).collect()
    }

    #[test]
    fn zoxide_time_inside_a_repository_counts_for_it() {
        let repos = [repo("/dev/baton", 0), repo("/dev/bat", 0)];
        let z = vec![
            (10.0, key(std::path::Path::new("/dev/baton"))),
            (4.0, key(std::path::Path::new("/dev/baton/internal"))),
            (1.0, key(std::path::Path::new("/dev/bat"))),
            (99.0, key(std::path::Path::new("/dev/batonx"))),
        ];
        assert_eq!(repo_scores(&repos, &z), [14.0, 1.0]);
    }

    #[test]
    fn zoxide_first_then_most_recent() {
        let repos = [repo("/a/old", 500), repo("/a/new", 1), repo("/a/used", 900)];
        let scores = [0.0, 0.0, 3.0];
        assert_eq!(base_order(&repos, &scores, Sort::Used), [2, 1, 0]);
        assert_eq!(base_order(&repos, &scores, Sort::Changed), [1, 0, 2]);
        assert_eq!(base_order(&repos, &scores, Sort::Name), [1, 0, 2]);
        assert_eq!(Sort::Name.next(), Sort::Used);
    }

    #[test]
    fn language_words_filter_and_the_rest_is_matched() {
        assert_eq!(parse_query("rs: tad"), (vec!["rs"], "tad".to_string()));
        assert_eq!(
            parse_query("RS: go: x y"),
            (vec!["rs", "go"], "x y".to_string())
        );
        assert_eq!(parse_query("foo: bar"), (vec![], "foo: bar".to_string()));
        let mut rust = repo("/dev/tally", 0);
        rust.lang = Some("rs");
        let mut go = repo("/dev/taskgo", 0);
        go.lang = Some("go");
        let repos = [rust, go, repo("/dev/tally", 0)];
        let order = [0, 1, 2];
        let mut s = Searcher::new();
        assert_eq!(names(&repos, &s.search(&repos, &order, "rs:")), ["tally"]);
        assert_eq!(
            names(&repos, &s.search(&repos, &order, "rs: go: ta")),
            ["tally", "taskgo"]
        );
        let hits = s.search(&repos, &order, "go: ta");
        assert_eq!(direct(&repos, &hits, "go: ta"), Some(1));
        assert_eq!(
            direct(&repos, &s.search(&repos, &order, "rs:"), "rs:"),
            None
        );
    }

    #[test]
    fn one_word_searches_names_and_more_words_search_paths_too() {
        let repos = [
            repo("/bat/zzz", 0),
            repo("/dev/baton", 0),
            repo("/work/obsidian/notes", 0),
            repo("/archive/notes", 0),
        ];
        let order = [0, 1, 2, 3];
        let mut searcher = Searcher::new();
        let hits = searcher.search(&repos, &order, "bat");
        assert_eq!(names(&repos, &hits), ["baton"]);
        assert_eq!(hits[0].name_hits, [0, 1, 2]);
        let hits = searcher.search(&repos, &order, "obsidian notes");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            repos[hits[0].index].path,
            PathBuf::from("/work/obsidian/notes")
        );
        assert_eq!(
            names(&repos, &searcher.search(&repos, &order, "bat/")),
            ["zzz"]
        );
    }

    #[test]
    fn jumps_only_when_the_answer_is_clear() {
        let repos = [
            repo("/a/baton", 0),
            repo("/a/bat-notes", 0),
            repo("/b/notes", 0),
            repo("/c/notes", 0),
        ];
        let order = [0, 1, 2, 3];
        let mut s = Searcher::new();
        let hit = |s: &mut Searcher, q: &str| {
            let hits = s.search(&repos, &order, q);
            direct(&repos, &hits, q)
        };
        // Exact name, even though bat-notes also matches.
        assert_eq!(hit(&mut s, "baton"), Some(0));
        // Only one candidate at all.
        assert_eq!(hit(&mut s, "bton"), Some(0));
        // Two repositories called notes: show the list.
        assert_eq!(hit(&mut s, "notes"), None);
        // Several candidates, none named exactly: show the list.
        assert_eq!(hit(&mut s, "bat"), None);
        assert_eq!(hit(&mut s, ""), None);
    }
}
