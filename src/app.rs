//! The interactive part: what the screen holds, what the keys do, and the
//! loop that ties them to the terminal.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::config::{self, After, Config, Mode};
use crate::first_run::Candidate;
use crate::gitstat::{self, Log, Status};
use crate::rank::{self, Hit, Searcher};
use crate::repo::{self, Repo};
use crate::{cache, keys, launch, scan};

type Error = Box<dyn std::error::Error>;

pub enum Outcome {
    /// Print this path for the shell function to cd into.
    Cd(PathBuf),
    /// Finished without a place to go, such as after opening Fork.
    Done,
    Cancelled,
}

enum Step {
    Continue,
    Exit(Outcome),
    /// Hand the terminal to a program and come back when it exits.
    /// Command lines with the folder each runs in, one after another.
    Run(Vec<(Vec<String>, PathBuf)>),
}

pub enum Screen {
    Setup(Setup),
    List,
    /// The folders searched for repositories, to check, add and remove.
    Folders(Folders),
}

pub struct Folders {
    pub rows: Vec<FolderRow>,
    pub selected: usize,
    /// More folders are being looked for, to offer below the current ones.
    pub looking: bool,
    rx: Option<Receiver<Vec<Candidate>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FolderRow {
    pub path: PathBuf,
    pub on: bool,
    pub repos: usize,
    /// A configured folder that is not there, such as an unplugged drive.
    pub missing: bool,
    /// Found by looking again, not yet searched.
    pub new: bool,
    pub dependency: bool,
}

pub struct Setup {
    pub candidates: Vec<Candidate>,
    pub on: Vec<bool>,
    pub selected: usize,
    /// Whether to show icons, switched with i while looking at a sample.
    pub icons: bool,
    /// A Nerd Font is installed, so the sample is worth a look.
    pub nerd_font: bool,
    found: Vec<(PathBuf, PathBuf)>,
}

#[derive(Clone, Copy)]
pub enum Action {
    Launch(usize),
    Copy,
    /// Hides the repository, or brings it back while hidden ones are shown.
    Hide,
    Menu,
    Rescan,
    ToggleHidden,
    ToggleChanged,
    EditSettings,
    Folders,
    Profiles,
}

/// The tabs: every repository, the ones with work to deal with, and the
/// hidden ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    All,
    Changed,
    Hidden,
}

impl View {
    pub const TABS: [View; 3] = [View::All, View::Changed, View::Hidden];

    pub fn label(self) -> &'static str {
        match self {
            View::All => "repos",
            View::Changed => "changed",
            View::Hidden => "hidden",
        }
    }
}

pub struct MenuItem {
    pub key: char,
    pub label: String,
    pub action: Action,
    pub terminal: bool,
}

pub struct Notice {
    pub text: String,
    pub bad: bool,
}

struct ScanRun {
    rx: Receiver<scan::Event>,
    cancel: Arc<AtomicBool>,
    found: Vec<(PathBuf, PathBuf)>,
}

pub struct App {
    pub screen: Screen,
    config: Option<Config>,
    pub repos: Vec<Repo>,
    cached: Vec<cache::Entry>,
    hidden: crate::hidden::Rules,
    /// When the settings file was last read or written by kura, to notice
    /// edits made elsewhere and apply them while open.
    settings_stamp: Stamp,
    zoxide: Vec<(f64, String)>,
    scores: Vec<f64>,
    pub hits: Vec<Hit>,
    searcher: Searcher,
    pub query: String,
    /// The selected repository's key, so the selection stays on the same
    /// repository when a scan inserts others above it.
    pub selected: Option<String>,
    /// Repositories marked with Tab, by key, for actions on several at once.
    pub marked: HashSet<String>,
    pub offset: usize,
    pub items: Vec<MenuItem>,
    pub menu: Option<usize>,
    pub help: bool,
    /// F1: the help page, which takes the whole screen.
    pub help_page: bool,
    /// Ctrl-G: the profiles to switch to (`None` is config.toml alone),
    /// and the one under the cursor.
    pub profiles: Option<(Vec<Option<String>>, usize)>,
    /// The name being typed for a new profile, from the last row of the
    /// profiles drop-down.
    pub naming: Option<String>,
    pub help_scroll: u16,
    /// Nerd Font icons in place of the two-letter language tags.
    pub icons: bool,
    pub sort: rank::Sort,
    /// Which tab is in view. Ctrl-D and Ctrl-O switch to changed and hidden.
    pub view: View,
    /// git's answer per repository key; `None` when git failed or timed out.
    /// A repository not here yet is still being asked.
    pub statuses: HashMap<String, Option<Status>>,
    /// The preview's commits and remote, per repository key.
    pub logs: HashMap<String, Option<Log>>,
    logs_asked: HashSet<String>,
    /// Repositories git was not asked about, their config running programs.
    pub untrusted: HashSet<String>,
    /// The repository the cursor rests on and since when, so the preview's
    /// log is asked for once the cursor stops rather than for every row
    /// scrolled past.
    resting: Option<(String, Instant)>,
    pool: gitstat::Pool,
    /// Languages are counted on a thread of their own, one repository at a
    /// time, since a large checkout takes a moment to walk.
    lang_tx: Option<mpsc::Sender<PathBuf>>,
    lang_rx: Option<Receiver<(PathBuf, crate::lang::Mix)>>,
    /// Each repository's languages with their share, for the preview's bar.
    pub mixes: HashMap<String, crate::lang::Mix>,
    /// What each repository is, from its README, read once it is selected.
    pub abouts: HashMap<String, Option<String>>,
    /// The cache changed since it was saved: a language was counted.
    cache_dirty: bool,
    /// Set once the screen opens: a direct jump never starts git.
    asking: bool,
    /// Ctrl-X in the menu: stay open or quit after starting a program,
    /// whatever its own setting says. `None` until it is pressed.
    pub stay: Option<bool>,
    pub notice: Option<Notice>,
    scan: Option<ScanRun>,
    pub tick: usize,
    pub list_area: Rect,
    /// Where the tabs were drawn, for clicks, in `View::TABS` order.
    pub tab_areas: [Rect; 3],
    /// Where the profile name sits in the header, which opens the
    /// profiles below it when clicked, and where their rows are.
    pub profile_area: Rect,
    pub profile_rows: Rect,
    /// Where the mouse is, so switches under it can light up.
    pub hover: Option<(u16, u16)>,
    last_click: Option<(Instant, usize)>,
}

impl App {
    fn new(screen: Screen, config: Option<Config>, query: String) -> Self {
        let mut app = Self {
            screen,
            config,
            repos: Vec::new(),
            cached: Vec::new(),
            hidden: Default::default(),
            settings_stamp: settings_stamp(),
            zoxide: rank::zoxide_scores(),
            scores: Vec::new(),
            hits: Vec::new(),
            searcher: Searcher::new(),
            query,
            selected: None,
            marked: HashSet::new(),
            offset: 0,
            items: Vec::new(),
            menu: None,
            help: false,
            help_page: false,
            profiles: None,
            naming: None,
            help_scroll: 0,
            icons: false,
            sort: rank::Sort::Used,
            view: View::All,
            statuses: HashMap::new(),
            logs: HashMap::new(),
            logs_asked: HashSet::new(),
            untrusted: HashSet::new(),
            resting: None,
            pool: gitstat::Pool::new(),
            lang_tx: None,
            lang_rx: None,
            mixes: HashMap::new(),
            abouts: HashMap::new(),
            cache_dirty: false,
            asking: false,
            stay: None,
            notice: None,
            scan: None,
            tick: 0,
            list_area: Rect::default(),
            tab_areas: [Rect::default(); 3],
            profile_area: Rect::default(),
            profile_rows: Rect::default(),
            hover: None,
            last_click: None,
        };
        app.apply_config();
        // Only at start: a reload keeps the order chosen with Ctrl-S.
        if let Some(config) = &app.config {
            app.sort = config.sort;
        }
        app
    }

    /// The list, filled from the cache at once; a scan starts with the loop.
    pub fn list(config: Config, cached: Vec<cache::Entry>, query: String) -> Self {
        let mut app = Self::new(Screen::List, Some(config), query);
        app.cached = cached.clone();
        for entry in &cached {
            app.add(entry.path.clone());
        }
        app.refresh();
        app
    }

    pub fn setup(
        candidates: Vec<Candidate>,
        found: Vec<(PathBuf, PathBuf)>,
        query: String,
    ) -> Self {
        let on = candidates.iter().map(|c| !c.dependency).collect();
        let setup = Setup {
            candidates,
            on,
            selected: 0,
            icons: false,
            nerd_font: crate::icons::detect(),
            found,
        };
        Self::new(Screen::Setup(setup), None, query)
    }

    fn apply_config(&mut self) {
        let Some(config) = &self.config else {
            return;
        };
        self.hidden = config.hidden.clone();
        self.icons = config.icons.unwrap_or(false);
        self.items = menu_items(config);
    }

    /// The repository to cd into without opening the screen, if the query
    /// names one beyond doubt.
    pub fn direct(&self) -> Option<PathBuf> {
        rank::direct(&self.repos, &self.hits, &self.query).map(|i| self.repos[i].path.clone())
    }

    /// How many repositories the current view can show, before the query.
    pub fn total(&self) -> usize {
        self.repos.iter().filter(|r| self.in_view(r)).count()
    }

    /// How many repositories a tab holds.
    pub fn count(&self, view: View) -> usize {
        self.repos.iter().filter(|r| self.belongs(view, r)).count()
    }

    fn in_view(&self, repo: &Repo) -> bool {
        self.belongs(self.view, repo)
    }

    fn belongs(&self, view: View, repo: &Repo) -> bool {
        let hidden = self.hidden.contains(&repo.path);
        match view {
            View::All => !hidden,
            View::Hidden => hidden,
            View::Changed => !hidden && self.status(repo).is_some_and(Status::needs_attention),
        }
    }

    pub fn status(&self, repo: &Repo) -> Option<&Status> {
        self.statuses.get(&repo.key()).and_then(Option::as_ref)
    }

    /// Whether git has answered for this repository, either way.
    pub fn checked(&self, repo: &Repo) -> bool {
        self.statuses.contains_key(&repo.key())
    }

    /// How many repositories git is still being asked about.
    pub fn checking(&self) -> usize {
        self.repos
            .iter()
            .filter(|r| !self.hidden.contains(&r.path) && !self.checked(r))
            .count()
    }

    pub fn roots(&self) -> &[PathBuf] {
        self.config.as_ref().map_or(&[], |c| c.roots.as_slice())
    }

    /// Starts asking git about every repository, from the first frame on.
    fn start_asking(&mut self) {
        self.asking = true;
        let (tx, jobs) = mpsc::channel::<PathBuf>();
        let (done, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for path in jobs {
                let mix = crate::lang::mix(&path);
                if done.send((path, mix)).is_err() {
                    return;
                }
            }
        });
        self.lang_tx = Some(tx);
        self.lang_rx = Some(rx);
        for repo in &self.repos {
            self.pool.status(repo.path.clone());
            self.count_lang(&repo.path);
        }
    }

    /// Counts a repository's language unless the cache has it already.
    fn count_lang(&self, path: &std::path::Path) {
        let key = repo::key(path);
        let cached = self
            .cached
            .iter()
            .any(|e| e.langs.is_some() && repo::key(&e.path) == key);
        if let (false, Some(tx)) = (cached, &self.lang_tx) {
            let _ = tx.send(path.to_path_buf());
        }
    }

    /// Saves languages counted since the last save, when no scan will.
    fn save_cache(&mut self) {
        if self.cache_dirty {
            let _ = cache::save(&self.cached);
            self.cache_dirty = false;
        }
    }

    /// Asks again after something may have changed a repository, such as
    /// lazygit run from the menu.
    fn ask_again(&mut self, path: &std::path::Path) {
        let key = repo::key(path);
        self.logs.remove(&key);
        self.logs_asked.remove(&key);
        if self.asking {
            self.pool.status(path.to_path_buf());
        }
    }

    /// Before each frame: the rows on screen go to the front of the queue,
    /// and the preview's log is asked for once the cursor has rested.
    fn prepare(&mut self) {
        if !self.asking || !matches!(self.screen, Screen::List) {
            return;
        }
        if self.pool.busy() {
            let height = self.list_area.height as usize;
            let visible: HashSet<String> = self
                .hits
                .iter()
                .skip(self.offset)
                .take(height)
                .map(|h| self.repos[h.index].key())
                .filter(|k| !self.statuses.contains_key(k))
                .collect();
            if !visible.is_empty() {
                self.pool.prioritize(&visible);
            }
        }
        let Some(current) = self.current().map(|r| (r.key(), r.path.clone())) else {
            return;
        };
        match &self.resting {
            Some((key, since)) if *key == current.0 => {
                if since.elapsed() >= Duration::from_millis(150) && !self.logs_asked.contains(key) {
                    // The README is a file read, not git, so even a
                    // repository git is not asked about gets its line.
                    let about = repo::about(&current.1);
                    self.abouts.insert(current.0.clone(), about);
                    self.logs_asked.insert(current.0);
                    self.pool.log(current.1);
                }
            }
            _ => self.resting = Some((current.0, Instant::now())),
        }
    }

    /// Whether the screen should come round again soon: a scan, git, or a
    /// preview waiting for the cursor to rest.
    fn busy(&self) -> bool {
        let waiting_log = self
            .resting
            .as_ref()
            .is_some_and(|(key, _)| !self.logs_asked.contains(key));
        let looking = matches!(&self.screen, Screen::Folders(f) if f.looking);
        self.scan.is_some() || self.pool.busy() || (self.asking && waiting_log) || looking
    }

    pub fn scanning(&self) -> Option<usize> {
        self.scan.as_ref().map(|s| s.found.len())
    }

    pub fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.hits
            .iter()
            .position(|h| &self.repos[h.index].key() == key)
    }

    pub fn current(&self) -> Option<&Repo> {
        self.selected_index()
            .map(|i| &self.repos[self.hits[i].index])
    }

    fn add(&mut self, path: PathBuf) -> bool {
        let key = repo::key(&path);
        if self.repos.iter().any(|r| r.key() == key) || !repo::is_repo(&path) {
            return false;
        }
        if self.asking {
            self.pool.status(path.clone());
            self.count_lang(&path);
        }
        let mut repo = Repo::inspect(path);
        // A counted language from the cache beats the guess from marker files.
        if let Some(langs) = self
            .cached
            .iter()
            .find(|e| repo::key(&e.path) == key)
            .and_then(|e| e.langs.as_ref())
        {
            let mix: Vec<(&'static str, u8)> = langs
                .iter()
                .filter_map(|(tag, share)| {
                    rank::LANGS
                        .iter()
                        .find(|l| **l == tag.as_str())
                        .map(|l| (*l, *share))
                })
                .collect();
            repo.lang = crate::lang::main(&mix);
            self.mixes.insert(key.clone(), mix);
        }
        self.scores
            .push(rank::repo_scores(std::slice::from_ref(&repo), &self.zoxide)[0]);
        self.repos.push(repo);
        true
    }

    fn refresh(&mut self) {
        let order: Vec<usize> = rank::base_order(&self.repos, &self.scores, self.sort)
            .into_iter()
            .filter(|&i| self.in_view(&self.repos[i]))
            .collect();
        self.hits = self.searcher.search(&self.repos, &order, &self.query);
        if self.selected_index().is_none() {
            self.selected = self.hits.first().map(|h| self.repos[h.index].key());
            self.offset = 0;
        }
    }

    fn start_scan(&mut self) {
        self.stop_scan();
        let Some(config) = &self.config else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let options = scan::Options {
            max_depth: config.max_depth,
            exclude: config.exclude.clone(),
        };
        scan::spawn(config.roots.clone(), options, Arc::clone(&cancel), tx);
        self.scan = Some(ScanRun {
            rx,
            cancel,
            found: Vec::new(),
        });
    }

    /// Stops a scan that is still running. What it found so far is added to
    /// the cache, without dropping anything, since an unfinished scan cannot
    /// tell a removed repository from one it had not reached yet.
    fn stop_scan(&mut self) {
        if let Some(scan) = self.scan.take() {
            scan.cancel.store(true, Ordering::Relaxed);
            if !scan.found.is_empty() {
                let mut entries = self.cached.clone();
                let known: HashSet<String> = entries.iter().map(|e| repo::key(&e.path)).collect();
                let now = cache::now();
                entries.extend(
                    cache::entries_from(&scan.found, now)
                        .into_iter()
                        .filter(|e| !known.contains(&repo::key(&e.path))),
                );
                let _ = cache::save(&entries);
            }
        }
    }

    /// Takes in what the scan found since the last frame.
    fn pump(&mut self) {
        if self.config.is_some() && settings_stamp() != self.settings_stamp {
            self.reload_settings();
        }
        let mut changed = false;
        while let Some(scan) = &mut self.scan {
            match scan.rx.try_recv() {
                Ok(scan::Event::Found { path, root }) => {
                    scan.found.push((path.clone(), root));
                    changed |= self.add(path);
                }
                Ok(scan::Event::Done { reachable }) => {
                    let scan = self.scan.take().expect("scan is running");
                    changed |= self.finish_scan(&scan.found, &reachable);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.scan = None;
                    break;
                }
            }
        }
        if let Screen::Folders(folders) = &mut self.screen
            && let Some(rx) = &folders.rx
            && let Ok(candidates) = rx.try_recv()
        {
            add_candidates(&mut folders.rows, candidates);
            folders.looking = false;
            folders.rx = None;
        }
        let mut counted = Vec::new();
        if let Some(rx) = &self.lang_rx {
            while let Ok(done) = rx.try_recv() {
                counted.push(done);
            }
        }
        for (path, mix) in counted {
            let key = repo::key(&path);
            if let Some(repo) = self.repos.iter_mut().find(|r| r.key() == key) {
                repo.lang = crate::lang::main(&mix);
            }
            if let Some(entry) = self.cached.iter_mut().find(|e| repo::key(&e.path) == key) {
                // An empty list marks "counted, nothing found", so it is not
                // counted again on every start.
                entry.langs = Some(mix.iter().map(|(t, s)| (t.to_string(), *s)).collect());
                self.cache_dirty = true;
            }
            self.mixes.insert(key, mix);
        }
        let mut answered = false;
        while let Ok(done) = self.pool.rx.try_recv() {
            match done {
                gitstat::Done::Status(path, status) => {
                    self.statuses.insert(repo::key(&path), status);
                    answered = true;
                }
                gitstat::Done::Log(path, log) => {
                    self.logs.insert(repo::key(&path), log);
                }
                gitstat::Done::Skipped(path) => {
                    let key = repo::key(&path);
                    self.statuses.insert(key.clone(), None);
                    self.logs.insert(key.clone(), None);
                    self.untrusted.insert(key);
                    answered = true;
                }
            }
        }
        // The changed tab is made of answers, so it fills in as they come.
        if changed || (answered && self.view == View::Changed) {
            self.refresh();
        }
    }

    fn finish_scan(&mut self, found: &[(PathBuf, PathBuf)], reachable: &[PathBuf]) -> bool {
        let roots = self
            .config
            .as_ref()
            .map(|c| c.roots.clone())
            .unwrap_or_default();
        let entries = cache::merge(&self.cached, found, &roots, reachable, cache::now());
        let _ = cache::save(&entries);
        let keep: HashSet<String> = entries.iter().map(|e| repo::key(&e.path)).collect();
        let before = self.repos.len();
        let (repos, scores): (Vec<_>, Vec<_>) = std::mem::take(&mut self.repos)
            .into_iter()
            .zip(std::mem::take(&mut self.scores))
            .filter(|(r, _)| keep.contains(&r.key()))
            .unzip();
        self.repos = repos;
        self.scores = scores;
        self.cached = entries;
        self.repos.len() != before
    }

    fn key(&mut self, key: KeyEvent) -> Step {
        if key.kind == KeyEventKind::Release {
            return Step::Continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Step::Exit(Outcome::Cancelled);
        }
        self.notice = None;
        match self.screen {
            Screen::Setup(_) => self.setup_key(key),
            Screen::Folders(_) => self.folders_key(key),
            Screen::List if self.profiles.is_some() => self.profiles_key(key),
            Screen::List if self.help_page => self.help_page_key(key),
            Screen::List if self.help => self.help_key(key),
            Screen::List if self.menu.is_some() => self.menu_key(key),
            Screen::List => self.list_key(key),
        }
    }

    fn setup_key(&mut self, key: KeyEvent) -> Step {
        let Screen::Setup(setup) = &mut self.screen else {
            return Step::Continue;
        };
        let last = setup.candidates.len().saturating_sub(1);
        match key.code {
            _ if step(&key) == Some(-1) => setup.selected = setup.selected.saturating_sub(1),
            _ if step(&key) == Some(1) => setup.selected = (setup.selected + 1).min(last),
            KeyCode::Char('i') => setup.icons = !setup.icons,
            KeyCode::Char(' ') if !setup.on.is_empty() => {
                setup.on[setup.selected] = !setup.on[setup.selected];
            }
            KeyCode::Enter => return self.finish_setup(),
            KeyCode::Esc => return Step::Exit(Outcome::Cancelled),
            _ => {}
        }
        Step::Continue
    }

    fn finish_setup(&mut self) -> Step {
        let Screen::Setup(setup) = std::mem::replace(&mut self.screen, Screen::List) else {
            return Step::Continue;
        };
        let roots: Vec<PathBuf> = setup
            .candidates
            .iter()
            .zip(&setup.on)
            .filter(|(_, on)| **on)
            .map(|(c, _)| c.root.clone())
            .collect();
        let written = config::write_initial(&roots, setup.icons).and_then(|path| {
            let text = std::fs::read_to_string(&path)?;
            Ok((path, Config::parse(&text)?))
        });
        let (path, config) = match written {
            Ok(done) => done,
            Err(e) => {
                self.screen = Screen::Setup(setup);
                self.notice = Some(Notice {
                    text: format!("Could not write the settings: {e}"),
                    bad: true,
                });
                return Step::Continue;
            }
        };
        let chosen: HashSet<String> = roots.iter().map(|r| repo::key(r)).collect();
        let found: Vec<(PathBuf, PathBuf)> = setup
            .found
            .into_iter()
            .filter(|(_, root)| chosen.contains(&repo::key(root)))
            .collect();
        self.cached = cache::entries_from(&found, cache::now());
        let _ = cache::save(&self.cached);
        self.config = Some(config);
        self.settings_stamp = settings_stamp();
        self.apply_config();
        for (path, _) in found {
            self.add(path);
        }
        self.refresh();
        self.notice = Some(Notice {
            text: format!("Saved {}", path.display()),
            bad: false,
        });
        // The first look stopped a few folders down; the settings go deeper.
        self.start_scan();
        Step::Continue
    }

    fn list_key(&mut self, key: KeyEvent) -> Step {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter => {
                return match self.current() {
                    Some(repo) => Step::Exit(Outcome::Cd(repo.path.clone())),
                    None => Step::Continue,
                };
            }
            // Esc undoes one thing at a time: the query, then the marks.
            KeyCode::Esc if !self.query.is_empty() => self.set_query(String::new()),
            KeyCode::Esc if !self.marked.is_empty() => self.marked.clear(),
            KeyCode::Esc => return Step::Exit(Outcome::Cancelled),
            _ if step(&key).is_some() => self.move_by(step(&key).unwrap_or(0)),
            KeyCode::PageUp => self.move_by(-(self.page() as isize)),
            KeyCode::PageDown => self.move_by(self.page() as isize),
            KeyCode::F(5) => return self.run(Action::Rescan),
            // Ctrl-H is Backspace in most terminals, and in readline.
            _ if key.code == KeyCode::Backspace || (ctrl && key.code == KeyCode::Char('h')) => {
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query);
            }
            KeyCode::Null => self.help = true,
            KeyCode::F(1) => {
                self.help_page = true;
                self.help_scroll = 0;
            }
            KeyCode::Char(' ') if ctrl => self.help = true,
            KeyCode::Char('p') if ctrl => return self.run(Action::Menu),
            KeyCode::Char('y') if ctrl => return self.run(Action::Copy),
            KeyCode::Char('u') if ctrl => self.set_query(String::new()),
            // Tab marks and steps down, Shift-Tab marks and steps up, as in fzf.
            KeyCode::Tab => self.toggle_mark(1),
            KeyCode::BackTab => self.toggle_mark(-1),
            KeyCode::Char('o') if ctrl => return self.run(Action::ToggleHidden),
            KeyCode::Char('g') if ctrl => return self.run(Action::Profiles),
            KeyCode::Char('d') if ctrl => return self.run(Action::ToggleChanged),
            KeyCode::Char('s') if ctrl => self.cycle_sort(),
            KeyCode::Char('a') if ctrl => self.toggle_all(),
            KeyCode::Char(c) if alt && !ctrl => {
                let c = c.to_ascii_lowercase();
                if let Some(action) = self.items.iter().find(|i| i.key == c).map(|i| i.action) {
                    return self.run(action);
                }
            }
            KeyCode::Char(c) if keys::is_typed_text(&key) => {
                let mut query = self.query.clone();
                query.push(c);
                self.set_query(query);
            }
            _ => {}
        }
        Step::Continue
    }

    fn menu_key(&mut self, key: KeyEvent) -> Step {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.items.len();
        let Some(index) = self.menu else {
            return Step::Continue;
        };
        match key.code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('p') if ctrl => self.menu = None,
            _ if step(&key) == Some(-1) => self.menu = Some((index + len - 1) % len),
            _ if step(&key) == Some(1) => self.menu = Some((index + 1) % len),
            KeyCode::Char('x') if ctrl => {
                let default_stay = self.default_stay(index);
                self.stay = Some(!self.stay.unwrap_or(default_stay));
            }
            KeyCode::Enter => {
                self.menu = None;
                return self.run(self.items[index].action);
            }
            KeyCode::Char(c) if keys::is_typed_text(&key) => {
                let c = c.to_ascii_lowercase();
                if let Some(action) = self.items.iter().find(|i| i.key == c).map(|i| i.action) {
                    self.menu = None;
                    return self.run(action);
                }
            }
            _ => {}
        }
        Step::Continue
    }

    fn help_page_key(&mut self, key: KeyEvent) -> Step {
        match key.code {
            KeyCode::Char('c') => {
                self.help_page = false;
                return self.run(Action::EditSettings);
            }
            KeyCode::Char('w') => {
                self.help_page = false;
                return self.run(Action::Folders);
            }
            _ if step(&key) == Some(-1) => self.help_scroll = self.help_scroll.saturating_sub(1),
            _ if step(&key) == Some(1) => self.help_scroll = self.help_scroll.saturating_add(1),
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
            KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
            _ => self.help_page = false,
        }
        Step::Continue
    }

    fn help_key(&mut self, key: KeyEvent) -> Step {
        self.help = false;
        match key.code {
            KeyCode::Char('p') => self.run(Action::Menu),
            KeyCode::Char('r') => self.run(Action::Rescan),
            KeyCode::Char('o') => self.run(Action::ToggleHidden),
            KeyCode::Char('d') => self.run(Action::ToggleChanged),
            KeyCode::Char('c') => self.run(Action::EditSettings),
            KeyCode::Char('w') => self.run(Action::Folders),
            KeyCode::Char('g') => self.run(Action::Profiles),
            KeyCode::Char('s') => {
                self.cycle_sort();
                Step::Continue
            }
            KeyCode::Char(c) => match self.items.iter().find(|i| i.key == c).map(|i| i.action) {
                Some(action) => self.run(action),
                None => Step::Continue,
            },
            _ => Step::Continue,
        }
    }

    /// Whether the item at `index` keeps kura open, before any Ctrl-X.
    fn default_stay(&self, index: usize) -> bool {
        match self.items[index].action {
            Action::Launch(i) => self
                .config
                .as_ref()
                .is_some_and(|c| c.launchers[i].after == After::Stay),
            _ => true,
        }
    }

    /// The menu's label for an item, which for hiding depends on the view.
    pub fn label<'a>(&self, item: &'a MenuItem) -> &'a str {
        match item.action {
            Action::Hide if self.view == View::Hidden => "show in the list",
            _ => &item.label,
        }
    }

    pub fn after_label(&self) -> &'static str {
        match self.stay {
            None => "as set",
            Some(true) => "stay",
            Some(false) => "quit",
        }
    }

    fn run(&mut self, action: Action) -> Step {
        match action {
            Action::Menu => {
                if self.current().is_some() && !self.items.is_empty() {
                    self.menu = Some(0);
                }
                return Step::Continue;
            }
            Action::Rescan => {
                self.start_scan();
                // A fresh look at every repository's state and language too.
                if self.asking {
                    for repo in &self.repos {
                        self.pool.status(repo.path.clone());
                        if let Some(tx) = &self.lang_tx {
                            let _ = tx.send(repo.path.clone());
                        }
                    }
                }
                return Step::Continue;
            }
            Action::EditSettings => return self.edit_settings(),
            Action::Profiles => {
                let mut names: Vec<Option<String>> = vec![None];
                names.extend(config::profiles().into_iter().map(Some));
                let at = names
                    .iter()
                    .position(|n| *n == config::profile())
                    .unwrap_or(0);
                self.menu = None;
                self.profiles = Some((names, at));
                return Step::Continue;
            }
            Action::Folders => {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::first_run::discover().0);
                });
                self.open_folders(Some(rx));
                return Step::Continue;
            }
            Action::ToggleHidden => {
                let view = if self.view == View::Hidden {
                    View::All
                } else {
                    View::Hidden
                };
                self.set_view(view);
                return Step::Continue;
            }
            Action::ToggleChanged => {
                let view = if self.view == View::Changed {
                    View::All
                } else {
                    View::Changed
                };
                self.set_view(view);
                return Step::Continue;
            }
            _ => {}
        }
        let targets = self.targets();
        let Some(first) = targets.first() else {
            return Step::Continue;
        };
        let label = match targets.len() {
            1 => first.name.clone(),
            n => format!("{n} repositories"),
        };
        let shown = match targets.len() {
            1 => first.path.display().to_string(),
            _ => label.clone(),
        };
        let paths: Vec<PathBuf> = targets.iter().map(|r| r.path.clone()).collect();
        match action {
            Action::Launch(i) => {
                let Some(launcher) = self.config.as_ref().map(|c| c.launchers[i].clone()) else {
                    return Step::Continue;
                };
                let mut runs = Vec::new();
                for repo in &targets {
                    match launch::command_line(&launcher.cmd, &repo.path, &repo.name) {
                        Ok(argv) => runs.push((argv, repo.path.clone())),
                        Err(e) => return self.bad(format!("[open] {}: {e}", launcher.name)),
                    }
                }
                self.marked.clear();
                if launcher.mode == Mode::Exec {
                    return Step::Run(runs);
                }
                for (argv, cwd) in &runs {
                    if let Err(e) = launch::detach(argv, cwd) {
                        return self.bad(format!(
                            "Could not start {}: {e} (kura config to fix)",
                            argv[0]
                        ));
                    }
                }
                if self.stay.unwrap_or(launcher.after == After::Stay) {
                    self.good(format!("Opened {label} in {}", launcher.name))
                } else {
                    Step::Exit(Outcome::Done)
                }
            }
            Action::Copy => {
                let text = paths
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n");
                match crate::clip::copy(&text) {
                    Ok(how) => {
                        self.marked.clear();
                        self.good(format!("{how}: {shown}"))
                    }
                    Err(e) => self.bad(format!("Could not copy: {e}")),
                }
            }
            Action::Hide if self.view == View::Hidden => {
                match config::remove_hidden(&self.hidden_entries(), &paths) {
                    Ok(()) => {
                        self.settings_stamp = settings_stamp();
                        for repo in &targets {
                            self.hidden.show(&repo.path);
                        }
                        // A pattern in the settings still holds what it matches;
                        // say which, since the menu cannot undo it.
                        let held = targets
                            .iter()
                            .find_map(|r| self.hidden.pattern(&r.path).map(str::to_string));
                        self.after_moving();
                        match held {
                        Some(pattern) => self.bad(format!(
                            "Still hidden by the pattern {pattern} (F1, then c, edits the settings)"
                        )),
                        None => self.good(format!("{label} back in the list")),
                    }
                    }
                    Err(e) => self.bad(format!("Could not bring it back: {e}")),
                }
            }
            Action::Hide => match config::add_hidden(&self.hidden_entries(), &paths) {
                Ok(()) => {
                    self.settings_stamp = settings_stamp();
                    for repo in &targets {
                        self.hidden.hide(&repo.path);
                    }
                    self.after_moving();
                    self.good(format!("Hid {label} (Ctrl-O lists hidden ones)"))
                }
                Err(e) => self.bad(format!("Could not hide: {e}")),
            },
            Action::Menu
            | Action::Rescan
            | Action::ToggleHidden
            | Action::ToggleChanged
            | Action::EditSettings
            | Action::Folders
            | Action::Profiles => Step::Continue,
        }
    }

    /// What an action applies to: the marked repositories in the current
    /// tab, or the selected one when nothing is marked. Marks hold through a
    /// change of query, as in fzf, so a repository filtered out of view
    /// still counts.
    fn targets(&self) -> Vec<Repo> {
        if self.marked.is_empty() {
            return self.current().cloned().into_iter().collect();
        }
        self.repos
            .iter()
            .filter(|r| self.in_view(r) && self.marked.contains(&r.key()))
            .cloned()
            .collect()
    }

    fn after_moving(&mut self) {
        self.marked.clear();
        self.selected = None;
        self.refresh();
    }

    /// Opens the settings file. Saving it is enough: the loop notices the
    /// change and applies it while kura stays open.
    fn edit_settings(&mut self) -> Step {
        let path = match config::target_path() {
            Ok(path) => path,
            Err(e) => return self.bad(e.to_string()),
        };
        let (cmd, mode) = config::editor_command(self.config.as_ref());
        let argv = match launch::command_line(&cmd, &path, "config.toml") {
            Ok(argv) => argv,
            Err(e) => return self.bad(format!("[open] editor: {e}")),
        };
        let dir = path.parent().unwrap_or(&path).to_path_buf();
        if mode == Mode::Exec {
            return Step::Run(vec![(argv, dir)]);
        }
        match launch::detach(&argv, &dir) {
            Ok(()) => self.good("Opened the settings; saving them applies the changes here".into()),
            Err(e) => self.bad(format!("Could not start {}: {e}", argv[0])),
        }
    }

    /// Applies a settings file changed while kura is open. A file that no
    /// longer parses keeps the settings in use and says what is wrong.
    fn reload_settings(&mut self) {
        self.settings_stamp = settings_stamp();
        let config = match Config::load() {
            Ok(Some(config)) => config,
            Ok(None) => return,
            Err(e) => {
                self.bad(format!("Settings not applied: {e}"));
                return;
            }
        };
        let rescan = self.config.as_ref().is_none_or(|old| {
            old.roots != config.roots
                || old.max_depth != config.max_depth
                || old.exclude != config.exclude
        });
        self.config = Some(config);
        self.apply_config();
        self.marked.clear();
        self.refresh();
        if rescan {
            self.start_scan();
        }
        self.good("Settings applied".into());
    }

    #[cfg(test)]
    pub fn refresh_for_tests(&mut self) {
        self.refresh();
    }

    /// The folders screen: the configured folders with what they hold,
    /// and, as `rx` answers, folders found by looking again.
    fn open_folders(&mut self, rx: Option<Receiver<Vec<Candidate>>>) {
        let rows = self
            .roots()
            .iter()
            .map(|root| {
                let k = repo::key(root);
                FolderRow {
                    path: root.clone(),
                    on: true,
                    repos: self
                        .repos
                        .iter()
                        .filter(|r| repo::key_within(&r.key(), &k))
                        .count(),
                    missing: !root.is_dir(),
                    new: false,
                    dependency: false,
                }
            })
            .collect();
        self.menu = None;
        self.help = false;
        self.screen = Screen::Folders(Folders {
            rows,
            selected: 0,
            looking: rx.is_some(),
            rx,
        });
    }

    fn folders_key(&mut self, key: KeyEvent) -> Step {
        let Screen::Folders(folders) = &mut self.screen else {
            return Step::Continue;
        };
        let last = folders.rows.len().saturating_sub(1);
        match key.code {
            _ if step(&key) == Some(-1) => folders.selected = folders.selected.saturating_sub(1),
            _ if step(&key) == Some(1) => folders.selected = (folders.selected + 1).min(last),
            KeyCode::Char(' ') if !folders.rows.is_empty() => {
                let row = &mut folders.rows[folders.selected];
                row.on = !row.on;
            }
            KeyCode::Enter => {
                let roots: Vec<PathBuf> = folders
                    .rows
                    .iter()
                    .filter(|r| r.on)
                    .map(|r| r.path.clone())
                    .collect();
                self.screen = Screen::List;
                // Saving changes the file; the next frame reads it back and
                // searches the folders as they now are.
                return match config::set_roots(&roots) {
                    Ok(()) => self.good("Saved the folders to search".into()),
                    Err(e) => self.bad(format!("Could not save the folders: {e}")),
                };
            }
            KeyCode::Esc => self.screen = Screen::List,
            _ => {}
        }
        Step::Continue
    }

    /// The terminal's title while open, from `[ui] name`.
    fn title(&self) -> String {
        match &self.config {
            Some(config) => config.name.clone(),
            None => "kura".into(),
        }
    }

    fn hidden_entries(&self) -> Vec<String> {
        self.config
            .as_ref()
            .map(|c| c.hidden_entries.clone())
            .unwrap_or_default()
    }

    /// The drop-down: the profiles, then a last row that makes a new one.
    fn profiles_key(&mut self, key: KeyEvent) -> Step {
        if self.naming.is_some() {
            return self.naming_key(key);
        }
        let Some((names, at)) = &mut self.profiles else {
            return Step::Continue;
        };
        // One row past the profiles: "+ new profile".
        let last = names.len();
        match key.code {
            _ if step(&key) == Some(-1) => *at = at.saturating_sub(1),
            _ if step(&key) == Some(1) => *at = (*at + 1).min(last),
            KeyCode::Enter if *at == last => self.naming = Some(String::new()),
            KeyCode::Enter => {
                let name = names[*at].clone();
                self.profiles = None;
                self.switch_profile(name);
            }
            _ => self.profiles = None,
        }
        Step::Continue
    }

    fn naming_key(&mut self, key: KeyEvent) -> Step {
        let Some(name) = &mut self.naming else {
            return Step::Continue;
        };
        match key.code {
            KeyCode::Esc => self.naming = None,
            KeyCode::Backspace => {
                name.pop();
            }
            KeyCode::Char(c)
                if keys::is_typed_text(&key)
                    && config::valid_profile_name(&format!("{name}{c}")) =>
            {
                name.push(c);
            }
            KeyCode::Enter if !name.is_empty() => {
                let name = name.clone();
                return self.make_profile(name);
            }
            _ => {}
        }
        Step::Continue
    }

    /// Makes a profile from the folders in use, switches to it, and opens
    /// the folders screen, where the new profile's folders are chosen.
    fn make_profile(&mut self, name: String) -> Step {
        let roots = self.roots().to_vec();
        if let Err(e) = config::create_profile(&name, &roots) {
            return self.bad(format!("Could not make the profile: {e}"));
        }
        self.naming = None;
        self.profiles = None;
        self.switch_profile(Some(name.clone()));
        let step = self.run(Action::Folders);
        self.good(format!(
            "Made profile {name}: check the folders it searches, then Enter"
        ));
        step
    }

    /// Changes the profile for the rest of this run: its settings, its
    /// cache and a scan of its folders. A profile that does not load
    /// leaves the one in use as it was.
    fn switch_profile(&mut self, name: Option<String>) {
        let before = config::profile();
        if name == before {
            return;
        }
        self.stop_scan();
        config::set_profile(name.clone());
        let config = match Config::load() {
            Ok(Some(config)) => config,
            Ok(None) => return,
            Err(e) => {
                config::set_profile(before);
                self.bad(format!("Could not switch: {e}"));
                return;
            }
        };
        self.config = Some(config);
        self.apply_config();
        self.settings_stamp = settings_stamp();
        self.repos.clear();
        self.scores.clear();
        self.statuses.clear();
        self.logs.clear();
        self.logs_asked.clear();
        self.marked.clear();
        self.selected = None;
        self.view = View::All;
        self.cached = cache::load();
        for path in self
            .cached
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
        {
            self.add(path);
        }
        self.refresh();
        self.start_scan();
        let shown = name.as_deref().unwrap_or("default");
        self.good(format!("Profile: {shown}"));
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.marked.clear();
        self.menu = None;
        self.selected = None;
        self.refresh();
    }

    fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.selected = None;
        self.refresh();
    }

    /// Marks every repository in view, or clears the marks when all of them
    /// are marked already. In view means after the query, so `rs:` then
    /// Ctrl-A marks every Rust repository.
    fn toggle_all(&mut self) {
        let keys: Vec<String> = self
            .hits
            .iter()
            .map(|h| self.repos[h.index].key())
            .collect();
        if keys.iter().all(|k| self.marked.contains(k)) {
            self.marked.clear();
        } else {
            self.marked.extend(keys);
        }
    }

    fn toggle_mark(&mut self, then: isize) {
        let Some(key) = self.current().map(Repo::key) else {
            return;
        };
        if !self.marked.remove(&key) {
            self.marked.insert(key);
        }
        self.move_by(then);
    }

    fn good(&mut self, text: String) -> Step {
        self.notice = Some(Notice { text, bad: false });
        Step::Continue
    }

    fn bad(&mut self, text: String) -> Step {
        self.notice = Some(Notice { text, bad: true });
        Step::Continue
    }

    fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = None;
        self.refresh();
    }

    fn page(&self) -> usize {
        (self.list_area.height as usize).max(1)
    }

    fn move_by(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.hits.len() as isize - 1) as usize;
        self.selected = Some(self.repos[self.hits[next].index].key());
    }

    fn mouse(&mut self, mouse: MouseEvent) -> Step {
        let on = |r: Rect| {
            mouse.row >= r.y
                && mouse.row < r.y + r.height
                && mouse.column >= r.x
                && mouse.column < r.x + r.width
        };
        if matches!(mouse.kind, MouseEventKind::Moved) {
            self.hover = Some((mouse.column, mouse.row));
            return Step::Continue;
        }
        let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        // The open profiles take a click on a row; one anywhere else closes them.
        if let Some((names, _)) = &self.profiles {
            if click && self.naming.is_none() {
                let row = mouse.row.saturating_sub(self.profile_rows.y) as usize;
                let inside = on(self.profile_rows);
                if inside && row == names.len() {
                    self.naming = Some(String::new());
                    return Step::Continue;
                }
                let name = (inside && row < names.len()).then(|| names[row].clone());
                self.profiles = None;
                if let Some(name) = name {
                    self.switch_profile(name);
                }
            }
            return Step::Continue;
        }
        if !matches!(self.screen, Screen::List)
            || self.menu.is_some()
            || self.help
            || self.help_page
        {
            return Step::Continue;
        }
        if click && on(self.profile_area) {
            return self.run(Action::Profiles);
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let on = |r: Rect| {
                    mouse.row == r.y && mouse.column >= r.x && mouse.column < r.x + r.width
                };
                if let Some(tab) = self.tab_areas.iter().position(|&r| on(r)) {
                    self.set_view(View::TABS[tab]);
                    return Step::Continue;
                }
                let area = self.list_area;
                if mouse.row < area.y || mouse.row >= area.y + area.height {
                    return Step::Continue;
                }
                let row = self.offset + (mouse.row - area.y) as usize;
                let Some(hit) = self.hits.get(row) else {
                    return Step::Continue;
                };
                self.selected = Some(self.repos[hit.index].key());
                let now = Instant::now();
                let double = self.last_click.is_some_and(|(at, r)| {
                    r == row && now.duration_since(at) < Duration::from_millis(400)
                });
                self.last_click = Some((now, row));
                if double {
                    return Step::Exit(Outcome::Cd(self.repos[hit.index].path.clone()));
                }
            }
            _ => {}
        }
        Step::Continue
    }
}

/// When `config.toml` and the profile's file were last changed.
type Stamp = (Option<std::time::SystemTime>, Option<std::time::SystemTime>);

fn settings_stamp() -> Stamp {
    let modified = |path: Result<PathBuf, Error>| {
        std::fs::metadata(path.ok()?)
            .and_then(|m| m.modified())
            .ok()
    };
    let profile = config::profile().map(|n| config::profile_path(&n));
    (modified(config::path()), profile.and_then(modified))
}

/// Adds folders found by looking again, leaving out any that is, holds or
/// lies inside a folder already listed.
fn add_candidates(rows: &mut Vec<FolderRow>, candidates: Vec<Candidate>) {
    for candidate in candidates {
        let k = repo::key(&candidate.root);
        let overlaps = rows.iter().any(|r| {
            let r = repo::key(&r.path);
            repo::key_within(&k, &r) || repo::key_within(&r, &k)
        });
        if !overlaps {
            rows.push(FolderRow {
                path: candidate.root,
                on: false,
                repos: candidate.count,
                missing: false,
                new: true,
                dependency: candidate.dependency,
            });
        }
    }
}

/// One row up or down: the arrows, or Ctrl-K and Ctrl-J as in fzf. Plain
/// j and k are typed into the query.
fn step(key: &KeyEvent) -> Option<isize> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up => Some(-1),
        KeyCode::Down => Some(1),
        KeyCode::Char('k') if ctrl => Some(-1),
        KeyCode::Char('j') if ctrl => Some(1),
        _ => None,
    }
}

/// The action menu: the programs from `[open]`, then the built-in actions.
/// Each gets a letter, the first free one from its name.
fn menu_items(config: &Config) -> Vec<MenuItem> {
    let mut taken: HashSet<char> = ['y', 'h', 'p', 'r', 'o', 'd', 's', 'c', 'w', 'g']
        .into_iter()
        .collect();
    let mut items = Vec::new();
    for (i, launcher) in config.launchers.iter().enumerate() {
        let key = launcher
            .name
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .chain('a'..='z')
            .find(|c| c.is_ascii_alphanumeric() && !taken.contains(c));
        let Some(key) = key else {
            continue;
        };
        taken.insert(key);
        items.push(MenuItem {
            key,
            label: launcher.name.clone(),
            action: Action::Launch(i),
            terminal: launcher.mode == Mode::Exec,
        });
    }
    items.push(MenuItem {
        key: 'y',
        label: "copy path".into(),
        action: Action::Copy,
        terminal: false,
    });
    items.push(MenuItem {
        key: 'h',
        label: "hide from the list".into(),
        action: Action::Hide,
        terminal: false,
    });
    items
}

/// Runs the screen until a repository is chosen or the user leaves.
pub fn run(mut app: App) -> Result<Outcome, Error> {
    if matches!(app.screen, Screen::List) {
        app.start_scan();
    }
    app.start_asking();
    let title = app.title();
    let mut term = Tui::enter(&title)?;
    let outcome = loop {
        app.pump();
        app.prepare();
        term.draw(|frame| crate::view::draw(frame, &mut app))?;
        let wait = if app.busy() { 100 } else { 1000 };
        if !event::poll(Duration::from_millis(wait))? {
            app.tick = app.tick.wrapping_add(1);
            continue;
        }
        // Every event already waiting is handled before the next frame: a
        // held key that scrolls the whole list redraws every row, and one
        // frame per repeat falls behind and keeps moving after the key is up.
        let step = loop {
            let step = match event::read()? {
                Event::Key(key) => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(..) => {
                    term.clear()?;
                    Step::Continue
                }
                _ => Step::Continue,
            };
            if !matches!(step, Step::Continue) || !event::poll(Duration::ZERO)? {
                break step;
            }
        };
        match step {
            Step::Continue => {}
            Step::Exit(outcome) => break outcome,
            Step::Run(runs) => {
                drop(term);
                // One after another: the next starts when the last one exits.
                let failed = runs
                    .iter()
                    .find_map(|(argv, cwd)| launch::exec(argv, cwd).err().map(|e| (argv, e)));
                term = Tui::enter(&title)?;
                if let Some((argv, e)) = failed {
                    app.bad(format!("Could not start {}: {e}", argv[0]));
                }
                // lazygit and the like change what git would say.
                for (_, cwd) in &runs {
                    app.ask_again(cwd);
                }
            }
        }
    };
    drop(term);
    app.stop_scan();
    app.save_cache();
    Ok(outcome)
}

/// Raw mode, the alternate screen and mouse capture, undone on drop. kura
/// draws on stderr so stdout stays clean for the chosen path. The whole
/// window is used: drawn inline, terminals that pin the command line of
/// long output (VS Code's) covered the list.
struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    /// Whether the terminal's title was set, and so is to be put back.
    titled: bool,
}

impl Tui {
    /// Takes the terminal; `title`, unless empty, names its tab meanwhile.
    fn enter(title: &str) -> Result<Self, Error> {
        enable_raw_mode()?;
        let opened = (|| -> Result<Self, Error> {
            crossterm::execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture)?;
            let titled = !title.is_empty();
            if titled {
                // Save the title on the terminal's stack first (CSI 22 t),
                // so leaving can put back whatever the shell had.
                crossterm::execute!(
                    io::stderr(),
                    crossterm::style::Print("\x1b[22;0t"),
                    crossterm::terminal::SetTitle(title)
                )?;
            }
            let mut terminal = Terminal::with_options(
                CrosstermBackend::new(io::stderr()),
                TerminalOptions {
                    viewport: Viewport::Fullscreen,
                },
            )?;
            terminal.clear()?;
            Ok(Self { terminal, titled })
        })();
        if opened.is_err() {
            let _ = crossterm::execute!(io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
        opened
    }
}

impl std::ops::Deref for Tui {
    type Target = Terminal<CrosstermBackend<io::Stderr>>;
    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl std::ops::DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        if self.titled {
            let _ = crossterm::execute!(io::stderr(), crossterm::style::Print("\x1b[23;0t"));
        }
        let _ = crossterm::execute!(io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = io::stderr().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::parse(
            r#"
[open]
fork = 'Fork.exe "{path}"'
editor = 'code "{path}"'
explorer = 'explorer.exe "{path}"'
tig = { cmd = 'tig', mode = 'exec' }
"#,
        )
        .unwrap()
    }

    fn app_with(names: &[&str]) -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(Screen::List, Some(config()), String::new());
        app.zoxide.clear();
        for name in names {
            let path = tmp.path().join(name);
            std::fs::create_dir_all(path.join(".git")).unwrap();
            std::fs::write(
                path.join(".git/HEAD"),
                "ref: refs/heads/main
",
            )
            .unwrap();
            app.add(path);
        }
        app.refresh();
        (tmp, app)
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Step {
        app.key(KeyEvent::new(code, modifiers))
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn menu_letters_come_from_names_and_avoid_the_built_ins() {
        let items = menu_items(&config());
        let keys: Vec<(char, &str)> = items.iter().map(|i| (i.key, i.label.as_str())).collect();
        assert_eq!(
            keys,
            [
                ('f', "fork"),
                ('e', "editor"),
                ('x', "explorer"),
                ('t', "tig"),
                ('y', "copy path"),
                ('h', "hide from the list"),
            ]
        );
        assert!(items[3].terminal);
    }

    #[test]
    fn typing_filters_and_enter_chooses_the_selection() {
        let (_tmp, mut app) = app_with(&["baton", "tally", "kura"]);
        typed(&mut app, "tal");
        assert_eq!(app.hits.len(), 1);
        match press(&mut app, KeyCode::Enter, KeyModifiers::NONE) {
            Step::Exit(Outcome::Cd(path)) => assert!(path.ends_with("tally")),
            _ => panic!("expected a cd"),
        }
    }

    #[test]
    fn ctrl_j_and_k_move_while_plain_letters_search() {
        let (_tmp, mut app) = app_with(&["a", "b", "c"]);
        let first = app.current().unwrap().name.clone();
        press(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_ne!(app.current().unwrap().name, first);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(app.current().unwrap().name, first);
        typed(&mut app, "jk");
        assert_eq!(app.query, "jk");
        press(&mut app, KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(app.query, "j");
    }

    #[test]
    fn marked_repositories_are_what_actions_apply_to() {
        let (_tmp, mut app) = app_with(&["a", "b", "c"]);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.marked.len(), 2);
        let names: Vec<String> = app.targets().into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 2);
        // The cursor moved on to the third, which is not marked.
        assert!(!names.contains(&app.current().unwrap().name));
        // Shift-Tab unmarks and steps back up.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(app.marked.len(), 1);
        // Marks hold while the query hides them.
        typed(&mut app, "zzz");
        assert_eq!(app.targets().len(), 1);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        // Esc drops the marks before it quits.
        assert!(matches!(
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE),
            Step::Continue
        ));
        assert!(app.marked.is_empty());
        assert!(matches!(
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE),
            Step::Exit(Outcome::Cancelled)
        ));
    }

    #[test]
    fn the_folders_screen_lists_roots_and_offers_new_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        std::fs::create_dir_all(root.join("a/.git")).unwrap();
        std::fs::write(root.join("a/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let text = format!("[scan]\nroots = ['{}']\n", root.display());
        let mut app = App::new(
            Screen::List,
            Some(Config::parse(&text).unwrap()),
            String::new(),
        );
        app.add(root.join("a"));
        let (tx, rx) = mpsc::channel();
        app.open_folders(Some(rx));
        let candidate = |p: PathBuf, count| Candidate {
            root: p,
            count,
            dependency: false,
        };
        tx.send(vec![
            // Inside the configured folder: already searched.
            candidate(root.join("a"), 1),
            candidate(tmp.path().join("other"), 3),
        ])
        .unwrap();
        app.pump();
        let Screen::Folders(folders) = &app.screen else {
            panic!("expected the folders screen");
        };
        assert!(!folders.looking);
        let rows: Vec<(bool, usize, bool)> = folders
            .rows
            .iter()
            .map(|r| (r.on, r.repos, r.new))
            .collect();
        assert_eq!(rows, [(true, 1, false), (false, 3, true)]);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 14)).unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("Folders to search"), "{screen}");
        assert!(screen.contains("[x]") && screen.contains("[ ]") && screen.contains("new"));
        assert!(screen.contains("kura add ."));
        // Esc leaves without saving.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::List));
    }

    #[test]
    fn a_new_profile_name_is_typed_in_the_drop_down() {
        let (_tmp, mut app) = app_with(&["a"]);
        app.profiles = Some((vec![None], 0));
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.naming.as_deref(), Some(""));
        // Spaces and slashes cannot go into a file name.
        typed(&mut app, "wo rk/");
        assert_eq!(app.naming.as_deref(), Some("work"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.naming.is_none() && app.profiles.is_some());
    }

    #[test]
    fn ctrl_a_marks_everything_in_view_and_again_clears() {
        let (_tmp, mut app) = app_with(&["alpha", "beta", "gamma"]);
        typed(&mut app, "a");
        let shown = app.hits.len();
        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(app.marked.len(), shown);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(app.marked.is_empty());
    }

    #[test]
    fn a_terminal_program_runs_once_for_each_marked_repository() {
        let (_tmp, mut app) = app_with(&["a", "b"]);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
        match press(&mut app, KeyCode::Char('t'), KeyModifiers::NONE) {
            Step::Run(runs) => assert_eq!(runs.len(), 2),
            _ => panic!("expected both to run"),
        }
        assert!(app.marked.is_empty());
    }

    #[test]
    fn escape_clears_the_query_before_it_quits() {
        let (_tmp, mut app) = app_with(&["baton"]);
        typed(&mut app, "x");
        assert!(matches!(
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE),
            Step::Continue
        ));
        assert!(app.query.is_empty());
        assert!(matches!(
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE),
            Step::Exit(Outcome::Cancelled)
        ));
    }

    #[test]
    fn the_selection_stays_on_its_repository_when_others_arrive() {
        let (tmp, mut app) = app_with(&["b", "c"]);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let chosen = app.current().unwrap().name.clone();
        let path = tmp.path().join("a");
        std::fs::create_dir_all(path.join(".git")).unwrap();
        std::fs::write(
            path.join(".git/HEAD"),
            "ref: refs/heads/main
",
        )
        .unwrap();
        app.add(path);
        app.refresh();
        assert_eq!(app.current().unwrap().name, chosen);
    }

    #[test]
    fn hidden_repositories_have_a_list_of_their_own() {
        let (tmp, mut app) = app_with(&["baton", "junk"]);
        app.hidden.hide(&tmp.path().join("junk"));
        app.refresh();
        assert_eq!(app.total(), 1);
        press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.view, View::Hidden);
        assert_eq!(app.current().unwrap().name, "junk");
        assert_eq!(app.label(&app.items[5]), "show in the list");
        press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.current().unwrap().name, "baton");
        assert_eq!((app.count(View::All), app.count(View::Hidden)), (1, 1));
    }

    #[test]
    fn the_changed_tab_holds_repositories_with_work_to_deal_with() {
        let (tmp, mut app) = app_with(&["clean", "dirty", "waiting"]);
        let key = |n: &str| repo::key(&tmp.path().join(n));
        app.statuses.insert(key("clean"), Some(Status::default()));
        app.statuses.insert(
            key("dirty"),
            Some(Status {
                dirty: 2,
                ..Status::default()
            }),
        );
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.view, View::Changed);
        assert_eq!(app.hits.len(), 1);
        assert_eq!(app.current().unwrap().name, "dirty");
        assert_eq!(app.count(View::Changed), 1);
        // One repository has not been answered yet.
        assert_eq!(app.checking(), 1);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.view, View::All);
    }

    #[test]
    fn ctrl_x_flips_what_follows_a_program_and_exec_hands_over_the_terminal() {
        let (_tmp, mut app) = app_with(&["baton"]);
        press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(app.menu, Some(0));
        assert_eq!(app.after_label(), "as set");
        press(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(app.after_label(), "stay");
        match press(&mut app, KeyCode::Char('t'), KeyModifiers::NONE) {
            Step::Run(runs) => {
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].0, ["tig"]);
                assert!(runs[0].1.ends_with("baton"));
            }
            _ => panic!("expected the terminal to be handed over"),
        }
    }
}
