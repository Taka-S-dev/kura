//! What git says about a repository: uncommitted changes and how far it is
//! ahead of or behind its upstream, and for the preview, its recent commits
//! and remote. git itself answers, so the counts are the ones `git status`
//! would show.
//!
//! Requests go through a small pool of threads, so a slow repository never
//! holds up the screen, and the rows in view can be moved to the front.

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A repository that takes longer than this is shown as unknown.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Changed files kept for the preview; the count covers the rest.
const FILES_KEPT: usize = 50;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Changed, staged and untracked files together.
    pub dirty: usize,
    pub ahead: u32,
    pub behind: u32,
    pub upstream: Option<String>,
    /// `XY path` for the first changed files, as `git status --short` shows.
    pub files: Vec<String>,
}

impl Status {
    /// Something to deal with: work not committed, or not pushed or pulled.
    pub fn needs_attention(&self) -> bool {
        self.dirty > 0 || self.ahead > 0 || self.behind > 0
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Log {
    /// Short hash, relative date and subject of the latest commits.
    pub commits: Vec<(String, String, String)>,
    pub remote: Option<String>,
    /// Commits in each of the last `WEEKS` weeks, oldest first.
    pub weeks: Vec<u32>,
    /// The other local branches, most recently committed first.
    pub branches: Vec<Branch>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    pub name: String,
    /// Seconds since its last commit.
    pub age: u64,
    pub ahead: u32,
    pub behind: u32,
    /// Its upstream was deleted, as after a merged pull request.
    pub gone: bool,
}

/// The span of the activity line in the preview.
pub const WEEKS: usize = 12;
/// Commits fetched for the preview, which shows as many as fit.
const COMMITS: &str = "-40";

#[derive(Clone)]
enum Job {
    Status(PathBuf),
    Log(PathBuf),
}

impl Job {
    fn path(&self) -> &Path {
        match self {
            Job::Status(p) | Job::Log(p) => p,
        }
    }
}

/// A finished request. `None` means git failed or took too long.
pub enum Done {
    Status(PathBuf, Option<Status>),
    Log(PathBuf, Option<Log>),
    /// Not asked: the repository's own config would make git run a program.
    Skipped(PathBuf),
}

pub struct Pool {
    queue: Arc<Mutex<VecDeque<Job>>>,
    /// Threads at work. Changed only with the queue locked, so a job pushed
    /// as the last thread leaves always finds a thread.
    workers: Arc<AtomicUsize>,
    max: usize,
    tx: Sender<Done>,
    pub rx: Receiver<Done>,
}

impl Pool {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let max = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8);
        Self {
            queue: Arc::default(),
            workers: Arc::default(),
            max,
            tx,
            rx,
        }
    }

    pub fn status(&self, path: PathBuf) {
        self.push(Job::Status(path), false);
    }

    /// The preview's commits and remote, ahead of any status still waiting.
    pub fn log(&self, path: PathBuf) {
        self.push(Job::Log(path), true);
    }

    pub fn busy(&self) -> bool {
        self.workers.load(Ordering::Relaxed) > 0
    }

    /// Moves waiting status requests for these repositories to the front:
    /// the rows on screen are answered before the rest of the list.
    pub fn prioritize(&self, keys: &HashSet<String>) {
        let mut queue = self.queue.lock().expect("queue lock");
        let (first, rest): (VecDeque<Job>, VecDeque<Job>) = queue
            .drain(..)
            .partition(|job| keys.contains(&crate::repo::key(job.path())));
        queue.extend(first);
        queue.extend(rest);
    }

    fn push(&self, job: Job, front: bool) {
        let mut queue = self.queue.lock().expect("queue lock");
        if front {
            queue.push_front(job);
        } else {
            queue.push_back(job);
        }
        if self.workers.load(Ordering::Relaxed) < self.max {
            self.workers.fetch_add(1, Ordering::Relaxed);
            let queue = Arc::clone(&self.queue);
            let workers = Arc::clone(&self.workers);
            let tx = self.tx.clone();
            std::thread::spawn(move || work(queue, workers, tx));
        }
    }
}

fn work(queue: Arc<Mutex<VecDeque<Job>>>, workers: Arc<AtomicUsize>, tx: Sender<Done>) {
    loop {
        let job = {
            let mut queue = queue.lock().expect("queue lock");
            match queue.pop_front() {
                Some(job) => job,
                None => {
                    workers.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
        };
        let done = answer(job);
        if tx.send(done).is_err() {
            workers.fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }
}

/// Asks git, unless the repository's config could make that run a program.
fn answer(job: Job) -> Done {
    if runs_programs(job.path()) {
        return Done::Skipped(job.path().to_path_buf());
    }
    match job {
        Job::Status(path) => {
            let status = status(&path);
            Done::Status(path, status)
        }
        Job::Log(path) => {
            let log = log(&path);
            Done::Log(path, log)
        }
    }
}

/// Whether a repository's own config holds settings that make git start a
/// program and that no command-line switch can turn off: clean, smudge and
/// process filters (which status runs for files .gitattributes sends
/// through them), and includes, which could bring in anything. kura looks
/// at every repository it finds, including ones it did not create, such as
/// an archive unpacked with its .git, so it asks git nothing about these.
/// An fsmonitor and signature checks are switched off on every call
/// instead (see `git`), so a repository using them is still asked.
pub fn runs_programs(repo: &Path) -> bool {
    crate::repo::config_files(repo)
        .iter()
        .filter(|file| file.is_file())
        .any(|file| config_names(file).is_none_or(|names| names.iter().any(|n| starts_program(n))))
}

/// The setting names in one config file, read by git itself so that every
/// form the syntax allows is understood: `[core]fsmonitor = x` on one line,
/// `[Filter.evil]`, quoting, continuations. Includes are not followed, and
/// listing names runs no program. `None` when git cannot read the file,
/// which is then treated as unsafe.
fn config_names(file: &Path) -> Option<Vec<String>> {
    let mut command = Command::new("git");
    command
        .args([
            "config",
            "--no-includes",
            "--null",
            "--name-only",
            "--list",
            "--file",
        ])
        .arg(file)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    crate::launch::no_window(&mut command);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty())
            .collect(),
    )
}

/// Whether a setting, by its lower-case name, can make git start a program.
fn starts_program(name: &str) -> bool {
    ["filter.", "include.", "includeif."]
        .iter()
        .any(|p| name.starts_with(p))
}

pub fn status(repo: &Path) -> Option<Status> {
    // Submodules are left alone: each has a config of its own, which the
    // check above does not read.
    git(
        repo,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--ignore-submodules=all",
        ],
    )
    .map(|out| parse_status(&out))
}

pub fn log(repo: &Path) -> Option<Log> {
    // A repository without commits makes log fail; that is an empty log.
    // When HEAD names a commit all the same, the commit cannot be read: an
    // object went missing, as a cloud folder's sync can leave it.
    let commits = match git(
        repo,
        &[
            "log",
            COMMITS,
            "--no-show-signature",
            "--format=%h%x09%cr%x09%s",
        ],
    ) {
        Some(out) => parse_log(&out),
        None if git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_some() => return None,
        None => Vec::new(),
    };
    let remote = git(repo, &["remote", "get-url", "origin"])
        .map(|out| out.trim().to_string())
        .filter(|url| !url.is_empty());
    let now = now();
    let since = format!("--since={}", now.saturating_sub(WEEKS as u64 * WEEK));
    let weeks = git(
        repo,
        &["log", &since, "--no-show-signature", "--format=%ct"],
    )
    .map(|out| weekly(&out, now))
    .unwrap_or_else(|| vec![0; WEEKS]);
    let branches = git(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%09%(committerdate:unix)%09%(upstream:track,nobracket)",
            "refs/heads",
        ],
    )
    .map(|out| parse_branches(&out, now))
    .unwrap_or_default();
    Some(Log {
        commits,
        remote,
        weeks,
        branches,
    })
}

const WEEK: u64 = 7 * 24 * 60 * 60;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Commit times, one per line, counted into weeks ending now, oldest first.
fn weekly(out: &str, now: u64) -> Vec<u32> {
    let mut weeks = vec![0; WEEKS];
    for at in out.lines().filter_map(|l| l.trim().parse::<u64>().ok()) {
        let ago = (now.saturating_sub(at) / WEEK) as usize;
        if ago < WEEKS {
            weeks[WEEKS - 1 - ago] += 1;
        }
    }
    weeks
}

/// `for-each-ref` lines: name, commit time, and the track as `ahead 3,
/// behind 1`, `gone` or nothing.
fn parse_branches(out: &str, now: u64) -> Vec<Branch> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let name = parts.next()?.to_string();
            let at: u64 = parts.next()?.trim().parse().ok()?;
            let track = parts.next().unwrap_or("").trim();
            let mut branch = Branch {
                name,
                age: now.saturating_sub(at),
                gone: track == "gone",
                ..Branch::default()
            };
            for part in track.split(',') {
                let mut words = part.split_whitespace();
                match (words.next(), words.next().and_then(|n| n.parse().ok())) {
                    (Some("ahead"), Some(n)) => branch.ahead = n,
                    (Some("behind"), Some(n)) => branch.behind = n,
                    _ => {}
                }
            }
            Some(branch)
        })
        .collect()
}

/// Runs git in `repo` and returns what it printed, or `None` if it failed
/// or ran past the timeout.
/// The files git tracks in `repo`, as paths relative to it, or `None`
/// where git is not to be asked or did not answer. ls-files reads the
/// index only, so no filter or other program the repository names runs.
pub fn tracked_files(repo: &Path) -> Option<Vec<String>> {
    if runs_programs(repo) {
        return None;
    }
    let out = git(repo, &["ls-files", "-z"])?;
    Some(
        out.split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        // A second guard after runs_programs: the settings that start a
        // program and can be switched off by name are, for every call. log
        // here prints no diffs, so diff and textconv drivers never run, and
        // hooks do not run on status or log.
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "log.showSignature=false",
        ])
        .arg("-C")
        .arg(repo)
        .args(args)
        // Reading status must not take the index lock from Fork or an editor.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::launch::no_window(&mut command);
    let mut child = command.spawn().ok()?;
    // Read on a thread of its own, so a long listing cannot fill the pipe
    // and stall git while this thread waits on the clock.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let out = reader.join().ok()?.ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

fn parse_status(out: &str) -> Status {
    let mut status = Status::default();
    for line in out.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            if let Some(ab) = header.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        status.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        status.behind = n.parse().unwrap_or(0);
                    }
                }
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                status.upstream = Some(upstream.to_string());
            }
            continue;
        }
        let Some(file) = short_entry(line) else {
            continue;
        };
        status.dirty += 1;
        if status.files.len() < FILES_KEPT {
            status.files.push(file);
        }
    }
    status
}

/// One porcelain v2 entry as `XY path`, the way `git status --short` reads.
fn short_entry(line: &str) -> Option<String> {
    let (kind, rest) = line.split_once(' ')?;
    let (xy, path) = match kind {
        // 1 XY sub mH mI mW hH hI path
        "1" => (rest.get(..2)?, rest.splitn(8, ' ').nth(7)?),
        // 2 XY sub mH mI mW hH hI Xscore path<TAB>origPath
        "2" => (
            rest.get(..2)?,
            rest.splitn(9, ' ').nth(8)?.split('\t').next()?,
        ),
        // u XY sub m1 m2 m3 mW h1 h2 h3 path
        "u" => (rest.get(..2)?, rest.splitn(10, ' ').nth(9)?),
        "?" => ("??", rest),
        _ => return None,
    };
    Some(format!("{} {path}", xy.replace('.', " ")))
}

fn parse_log(out: &str) -> Vec<(String, String, String)> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some((
                parts.next()?.to_string(),
                short_age(parts.next()?),
                parts.next().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

/// git's "3 hours ago" as the list writes ages: "3h".
fn short_age(relative: &str) -> String {
    let mut words = relative.split_whitespace();
    let (Some(n), Some(unit)) = (words.next(), words.next()) else {
        return relative.to_string();
    };
    let unit = match unit.trim_end_matches(',').trim_end_matches('s') {
        "second" => "s",
        "minute" => "m",
        "hour" => "h",
        "day" => "d",
        "week" => "w",
        "month" => "mo",
        "year" => "y",
        _ => return relative.to_string(),
    };
    // "1 year, 2 months ago" keeps only the larger part.
    format!("{}{unit}", n.trim_end_matches(','))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_gives_changes_and_distance() {
        let out = "\
# branch.oid 3f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 .M N... 100644 100644 100644 aaa bbb src/app.rs
1 A. N... 000000 100644 100644 000 ccc docs/new file.md
2 R. N... 100644 100644 100644 ddd eee R100 src/new.rs\tsrc/old.rs
u UU N... 100644 100644 100644 100644 f g h conflict.txt
? notes.md
";
        let s = parse_status(out);
        assert_eq!((s.dirty, s.ahead, s.behind), (5, 2, 1));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(
            s.files,
            [
                " M src/app.rs",
                "A  docs/new file.md",
                "R  src/new.rs",
                "UU conflict.txt",
                "?? notes.md"
            ]
        );
        assert!(s.needs_attention());
    }

    #[test]
    fn a_clean_branch_without_upstream_needs_nothing() {
        let s = parse_status("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(s, Status::default());
        assert!(!s.needs_attention());
    }

    #[test]
    fn commits_fall_into_the_weeks_they_were_made() {
        let now = 100 * WEEK;
        let out = format!(
            "{}\n{}\n{}\n{}\n",
            now - 10,
            now - WEEK - 5,
            now - 3 * WEEK,
            now - 20 * WEEK
        );
        let weeks = weekly(&out, now);
        assert_eq!(weeks.len(), WEEKS);
        assert_eq!(weeks[WEEKS - 1], 1);
        assert_eq!(weeks[WEEKS - 2], 1);
        assert_eq!(weeks[WEEKS - 4], 1);
        assert_eq!(
            weeks.iter().sum::<u32>(),
            3,
            "older than the span is left out"
        );
    }

    #[test]
    fn branches_carry_their_age_and_distance() {
        let out = "feat/x\t990\tahead 3, behind 1\nold\t100\tgone\nlocal\t500\t\n";
        let branches = parse_branches(out, 1000);
        assert_eq!(
            branches[0],
            Branch {
                name: "feat/x".into(),
                age: 10,
                ahead: 3,
                behind: 1,
                gone: false
            }
        );
        assert!(branches[1].gone);
        assert_eq!((branches[2].ahead, branches[2].behind), (0, 0));
    }

    #[test]
    fn log_lines_and_ages_are_shortened() {
        let commits =
            parse_log("3f2a9c1\t2 hours ago\tfix: a\tb\nab12cd3\t1 year, 2 months ago\tinit\n");
        assert_eq!(
            commits[0],
            ("3f2a9c1".into(), "2h".into(), "fix: a\tb".into())
        );
        assert_eq!(commits[1].1, "1y");
        assert_eq!(short_age("5 minutes ago"), "5m");
    }

    /// `cargo test --release time_every_cached_repository -- --ignored --nocapture`
    /// shows how long git takes for each repository kura knows here.
    #[test]
    #[ignore]
    fn time_every_cached_repository() {
        let mut times: Vec<(Duration, String, bool)> = crate::cache::load()
            .into_iter()
            .map(|e| {
                let started = Instant::now();
                let ok = status(&e.path).is_some();
                (started.elapsed(), e.path.display().to_string(), ok)
            })
            .collect();
        times.sort_by_key(|t| std::cmp::Reverse(t.0));
        let total: Duration = times.iter().map(|t| t.0).sum();
        eprintln!(
            "{} repositories, {:?} one after another",
            times.len(),
            total
        );
        for (took, path, ok) in times.iter().take(8) {
            eprintln!("{took:>10.2?} {} {path}", if *ok { "ok" } else { "--" });
        }
    }

    #[test]
    fn configs_that_would_run_programs_are_recognised_as_git_reads_them() {
        let tmp = tempfile::tempdir().unwrap();
        let check = |text: &str| {
            let file = tmp.path().join("config");
            std::fs::write(&file, text).unwrap();
            config_names(&file).map(|names| names.iter().any(|n| starts_program(n)))
        };
        if check("[core]\n\tbare = false\n").is_none() {
            return; // No git here.
        }
        let risky = [
            "[filter \"lfs\"]\n\tclean = evil %f\n",
            "[include]\n\tpath = ../x\n",
            "[includeIf \"gitdir:~/\"]\n\tpath = x\n",
            // Forms a line-by-line reading missed.
            "[filter \"x\"]clean = evil\n",
            "[Filter.evil]\n\tclean = evil\n",
            "[core]\n[INCLUDE]path = x\n",
        ];
        for text in risky {
            assert_eq!(check(text), Some(true), "{text}");
        }
        let plain = "[core]\n\tbare = false\n# [filter]\n[remote \"origin\"]\n\turl = x\n";
        assert_eq!(check(plain), Some(false));
        // Switched off on every call instead, so these repositories are asked.
        let switched_off =
            "[core]\n\tfsmonitor = true\n[log]\n\tshowSignature = true\n[gpg]\n\tprogram = x\n";
        assert_eq!(check(switched_off), Some(false));
        // A file git cannot read is not trusted.
        assert_eq!(check("[core\n"), None);
    }

    #[test]
    fn a_repository_with_a_clean_filter_is_not_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status();
        if !init.is_ok_and(|s| s.success()) {
            return;
        }
        let marker = tmp.path().join("ran");
        let script = if cfg!(windows) {
            format!("cmd /c echo x > \"{}\"", marker.display())
        } else {
            format!("touch '{}'", marker.display())
        };
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .status()
                .unwrap();
        };
        // A tracked file changed after the filter is set is what makes
        // status pass its content through the clean filter.
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        git(&["add", "a.txt"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "a",
        ]);
        git(&["config", "filter.evil.clean", &script]);
        std::fs::write(tmp.path().join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "y").unwrap();
        let status = answer(Job::Status(tmp.path().to_path_buf()));
        let log = answer(Job::Log(tmp.path().to_path_buf()));
        // First what matters: nothing the repository named was run.
        assert!(!marker.exists(), "the repository's clean filter ran");
        assert!(matches!(status, Done::Skipped(_)));
        assert!(matches!(log, Done::Skipped(_)));
    }

    #[test]
    fn a_repository_cannot_make_git_status_run_its_fsmonitor() {
        let tmp = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status();
        if !init.is_ok_and(|s| s.success()) {
            return;
        }
        // A config that runs a program when git status asks the monitor.
        let marker = tmp.path().join("ran");
        let script = if cfg!(windows) {
            format!("cmd /c echo x > \"{}\"", marker.display())
        } else {
            format!("touch '{}'", marker.display())
        };
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["config", "core.fsmonitor", &script])
            .status()
            .unwrap();
        status(tmp.path());
        assert!(!marker.exists(), "the repository's fsmonitor ran");
    }

    #[test]
    fn a_real_repository_reports_its_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status();
        if !init.is_ok_and(|s| s.success()) {
            return; // No git here; the parsing tests cover the rest.
        }
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let s = status(tmp.path()).expect("git status runs");
        assert_eq!(s.dirty, 1);
        assert_eq!(s.files, ["?? a.txt"]);
        let pool = Pool::new();
        pool.status(tmp.path().to_path_buf());
        match pool.rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Done::Status(path, Some(s)) => {
                assert_eq!(path, tmp.path());
                assert_eq!(s.dirty, 1);
            }
            _ => panic!("expected a status"),
        }
    }

    #[test]
    fn a_missing_commit_is_not_taken_for_an_empty_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !run(&["init", "-q"]) {
            return;
        }
        assert!(
            log(repo)
                .expect("an empty repository has a log")
                .commits
                .is_empty()
        );
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        assert!(run(&["add", "a.txt"]) && run(&["commit", "-q", "-m", "first"]));
        assert_eq!(log(repo).unwrap().commits.len(), 1);
        // The commit's object goes missing, as a cloud folder's sync can
        // leave it; HEAD still names it.
        let head = git(repo, &["rev-parse", "HEAD"]).unwrap();
        let head = head.trim();
        std::fs::remove_file(repo.join(".git/objects").join(&head[..2]).join(&head[2..])).unwrap();
        assert!(log(repo).is_none());
    }
}
