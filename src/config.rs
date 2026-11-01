//! The settings file: where to look for repositories, and which programs the
//! action menu offers.
//!
//! Profiles are files in `profiles\` beside it. One holds only what differs
//! for that profile, usually the folders to search and what to hide, and is
//! laid over `config.toml`, so the menu and the rest are written once.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

type Error = Box<dyn std::error::Error>;

pub const DEFAULT_EXCLUDE: &[&str] =
    &["node_modules", "vendor", "target", ".venv", "dist", "build"];
const DEFAULT_DEPTH: usize = 6;

#[derive(Clone, Debug)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub max_depth: usize,
    pub exclude: Vec<String>,
    pub hidden: crate::hidden::Rules,
    /// `hidden` as written, to carry into a profile's file on first change.
    pub hidden_entries: Vec<String>,
    pub launchers: Vec<Launcher>,
    /// Nerd Font icons; off unless asked for.
    pub icons: Option<bool>,
    /// The order the list opens in; Ctrl-S changes it for the session.
    pub sort: crate::rank::Sort,
    /// The name shown in the terminal's title; empty leaves the title alone.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Launcher {
    /// The key under `[open]`, shown in the menu.
    pub name: String,
    pub cmd: String,
    pub mode: Mode,
    pub after: After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Started on its own, for GUI programs.
    Detach,
    /// Given the terminal until it exits, for TUI programs.
    Exec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Quit,
    Stay,
}

#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    scan: ScanFile,
    #[serde(default)]
    open: toml::Table,
    #[serde(default)]
    ui: UiFile,
}

#[derive(Deserialize, Default)]
struct UiFile {
    icons: Option<bool>,
    sort: Option<String>,
    /// The profile kura starts in; read from `config.toml` only.
    #[allow(dead_code)]
    profile: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(default)]
struct ScanFile {
    roots: Vec<PathBuf>,
    max_depth: usize,
    exclude: Vec<String>,
    hidden: Vec<String>,
}

impl Default for ScanFile {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_depth: DEFAULT_DEPTH,
            exclude: DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect(),
            hidden: Vec::new(),
        }
    }
}

/// `KURA_CONFIG_DIR`, when set: one folder for the settings, profiles and
/// caches, to try changes on a copy, or to keep tests off real settings.
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("KURA_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

fn dir() -> Result<PathBuf, Error> {
    if let Some(dir) = override_dir() {
        return Ok(dir);
    }
    let dirs = directories::BaseDirs::new().ok_or("cannot find the home directory")?;
    Ok(dirs.config_dir().join("kura"))
}

/// `config.toml`, shared by every profile.
pub fn path() -> Result<PathBuf, Error> {
    Ok(dir()?.join("config.toml"))
}

pub fn profile_path(name: &str) -> Result<PathBuf, Error> {
    Ok(dir()?.join("profiles").join(format!("{name}.toml")))
}

/// The profile in use, for this run of kura.
static PROFILE: Mutex<Option<String>> = Mutex::new(None);

pub fn profile() -> Option<String> {
    PROFILE.lock().expect("profile lock").clone()
}

pub fn set_profile(name: Option<String>) {
    *PROFILE.lock().expect("profile lock") = name;
}

/// The file kura writes to when the menu hides a repository or the folders
/// change: the profile's own file, or `config.toml` without a profile.
pub fn target_path() -> Result<PathBuf, Error> {
    match profile() {
        Some(name) => profile_path(&name),
        None => path(),
    }
}

/// The profiles there are, by name: the files in `profiles\`.
pub fn profiles() -> Vec<String> {
    let Ok(dir) = dir().map(|d| d.join("profiles")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Settles which profile this run uses: `wanted` from the command line,
/// then `KURA_PROFILE`, then `[ui] profile` in `config.toml`. A profile
/// named but not there is an error, not a silent fall back to everything.
pub fn choose_profile(wanted: Option<String>) -> Result<(), Error> {
    let name = wanted
        .or_else(|| std::env::var("KURA_PROFILE").ok())
        .filter(|n| !n.trim().is_empty())
        .or_else(default_profile);
    if let Some(name) = &name
        && !profile_path(name)?.is_file()
    {
        let known = profiles();
        let known = if known.is_empty() {
            "none yet".to_string()
        } else {
            known.join(", ")
        };
        return Err(format!(
            "no profile named {name} (profiles: {known}); `kura --profile {name} config` makes one"
        )
        .into());
    }
    set_profile(name);
    Ok(())
}

impl Config {
    /// `None` when there is no settings file yet, which means a first run.
    /// With a profile in use, its file is laid over `config.toml`.
    pub fn load() -> Result<Option<Self>, Error> {
        let Some(table) = load_table(None)? else {
            return Ok(None);
        };
        Self::from_table(table).map(Some).map_err(|e| {
            format!(
                "{}: {e}",
                target_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )
            .into()
        })
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        Self::from_table(toml::from_str(text)?)
    }

    fn from_table(table: toml::Table) -> Result<Self, Error> {
        let file: File = toml::Value::Table(table).try_into()?;
        let mut launchers = Vec::new();
        for (name, value) in file.open {
            launchers.push(launcher(name, value)?);
        }
        Ok(Self {
            roots: file.scan.roots,
            max_depth: file.scan.max_depth,
            exclude: file.scan.exclude,
            hidden: crate::hidden::Rules::parse(&file.scan.hidden)?,
            hidden_entries: file.scan.hidden,
            launchers,
            icons: file.ui.icons,
            name: file.ui.name.unwrap_or_else(|| "kura".into()),
            sort: match file.ui.sort.as_deref() {
                None => crate::rank::Sort::Used,
                Some(text) => crate::rank::Sort::parse(text)
                    .ok_or_else(|| format!("[ui] sort is used, changed or name, not {text}"))?,
            },
        })
    }
}

/// Which file a change goes to, and so which text a check stands in for.
#[derive(Clone, Copy)]
enum Layer<'a> {
    Shared(&'a str),
    Profile(&'a str),
}

/// The settings in effect as one table: `config.toml`, with the profile in
/// use laid over it. `changed` stands in for one file's text, to check a
/// change before it is written.
fn load_table(changed: Option<Layer>) -> Result<Option<toml::Table>, Error> {
    let path = path()?;
    let base = match changed {
        Some(Layer::Shared(text)) => text.to_string(),
        _ => match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        },
    };
    let mut table: toml::Table =
        toml::from_str(&base).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(name) = profile() {
        let path = profile_path(&name)?;
        let text = match changed {
            Some(Layer::Profile(text)) => text.to_string(),
            _ => fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        };
        let over: toml::Table =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        lay_over(&mut table, over);
    }
    Ok(Some(table))
}

/// The settings `config get` and `config set` take, as `section.key`.
/// `[open]` takes any name, being the menu's programs.
const KEYS: &[&str] = &[
    "scan.roots",
    "scan.max_depth",
    "scan.exclude",
    "scan.hidden",
    "ui.icons",
    "ui.sort",
    "ui.name",
    "ui.profile",
];

fn split_key(key: &str) -> Result<(&str, &str), Error> {
    let known = || {
        format!(
            "unknown setting {key}; one of {}, or open.<program>",
            KEYS.join(", ")
        )
    };
    let (section, name) = key.split_once('.').ok_or_else(known)?;
    if section == "open" && !name.is_empty() && !name.contains('.') || KEYS.contains(&key) {
        Ok((section, name))
    } else {
        Err(known().into())
    }
}

/// One setting in effect, as TOML, such as `'notes'` or `['C:\\work']`.
pub fn get_value(key: &str) -> Result<Option<String>, Error> {
    let (section, name) = split_key(key)?;
    let table = load_table(None)?.ok_or("no settings yet; run `kura` once to set up")?;
    Ok(table
        .get(section)
        .and_then(|s| s.get(name))
        .map(|v| v.to_string()))
}

/// The file a setting is written to: `[scan]` belongs to the profile in
/// use, while `[ui]` and `[open]` are shared by every profile, and
/// `ui.profile` is only ever read from `config.toml`.
pub fn file_for(key: &str) -> Result<PathBuf, Error> {
    if key.starts_with("scan.") {
        target_path()
    } else {
        path()
    }
}

/// Sets one setting, in the file `file_for` names. The value is read as
/// TOML when it is one (`true`, `6`, `['a', 'b']`, `{ cmd = 'x', mode =
/// 'exec' }`) and as text otherwise, so `notes` needs no quotes. A change
/// that would leave settings kura cannot read is refused and nothing is
/// written.
pub fn set_value(key: &str, value: &str) -> Result<PathBuf, Error> {
    let (section, name) = split_key(key)?;
    let path = file_for(key)?;
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let updated = with_value(&text, section, name, value)?;
    let layer = if path == self::path()? {
        Layer::Shared(&updated)
    } else {
        Layer::Profile(&updated)
    };
    let table = load_table(Some(layer))?.ok_or("no settings yet; run `kura` once to set up")?;
    Config::from_table(table).map_err(|e| format!("{key} = {value} is not usable: {e}"))?;
    if key == "ui.profile" && value != "''" && !value.is_empty() {
        let name = value.trim_matches(['\'', '"']);
        if !profile_path(name)?.is_file() {
            return Err(format!("no profile named {name} (kura profile list shows them)").into());
        }
    }
    write_atomic(&path, &updated)?;
    Ok(path)
}

fn with_value(text: &str, section: &str, name: &str, value: &str) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let parsed = format!("v = {value}")
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|d| d.get("v").cloned());
    let item = parsed.unwrap_or_else(|| toml_edit::value(value));
    doc.entry(section)
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| format!("[{section}] is not a table"))?
        .insert(name, item);
    Ok(doc.to_string())
}

/// A profile over the shared settings: its sections add to theirs, and a
/// value it gives replaces theirs whole, so `roots` in a profile is the
/// profile's list, not an addition to the shared one.
fn lay_over(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(section)), toml::Value::Table(more)) => {
                for (k, v) in more {
                    section.insert(k, v);
                }
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Sets the profile kura starts in, `[ui] profile` in `config.toml`, or
/// clears it with `None`, keeping the rest of the file as it is.
pub fn set_default_profile(name: Option<&str>) -> Result<(), Error> {
    let path = path()?;
    let text = fs::read_to_string(&path)?;
    write_atomic(&path, &with_default_profile(&text, name)?)?;
    Ok(())
}

fn with_default_profile(text: &str, name: Option<&str>) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let ui = doc
        .entry("ui")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or("[ui] is not a table")?;
    match name {
        Some(name) => {
            ui.insert("profile", toml_edit::value(name));
        }
        None => {
            ui.remove("profile");
        }
    }
    Ok(doc.to_string())
}

/// The profile `[ui] profile` names, if any.
pub fn default_profile() -> Option<String> {
    let text = fs::read_to_string(path().ok()?).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table
        .get("ui")?
        .get("profile")?
        .as_str()
        .map(str::to_string)
}

/// Whether `name` can name a profile: it becomes a file name and a word on
/// the command line, so letters, digits, - and _ only.
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Makes a profile that starts from these folders, and never replaces one.
pub fn create_profile(name: &str, roots: &[PathBuf]) -> Result<PathBuf, Error> {
    if !valid_profile_name(name) {
        return Err(
            format!("not usable as a profile name: {name} (letters, digits, - and _)").into(),
        );
    }
    let file = profile_path(name)?;
    if file.exists() {
        return Err(format!("there is a profile named {name} already").into());
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(&file, &profile_text(name, roots))?;
    Ok(file)
}

/// The text of a new profile: the folders and hidden list it starts from,
/// to change from there.
pub fn profile_text(name: &str, roots: &[PathBuf]) -> String {
    let roots = roots
        .iter()
        .map(|r| format!("  {},\n", quote(&r.to_string_lossy())))
        .collect::<String>();
    format!(
        "\
# kura profile {name}. Anything here replaces the same setting in
# config.toml while this profile is in use; the rest comes from there.
# Copy this file under another name to make another profile.

[scan]
# Folders to look for repositories in, for this profile only.
roots = [
{roots}]
# Hidden in this profile only. Paths and .gitignore-style patterns.
hidden = []
"
    )
}

fn launcher(name: String, value: toml::Value) -> Result<Launcher, Error> {
    let bad = |what: &str| format!("[open] {name}: {what}");
    match value {
        toml::Value::String(cmd) => Ok(Launcher {
            name,
            cmd,
            mode: Mode::Detach,
            after: After::Quit,
        }),
        toml::Value::Table(table) => {
            let cmd = match table.get("cmd") {
                Some(toml::Value::String(s)) => s.clone(),
                _ => return Err(bad("needs cmd = '...'").into()),
            };
            let mode = match table.get("mode").and_then(|v| v.as_str()) {
                None | Some("detach") => Mode::Detach,
                Some("exec") => Mode::Exec,
                Some(other) => {
                    return Err(bad(&format!("mode is detach or exec, not {other}")).into());
                }
            };
            let after = match table.get("after").and_then(|v| v.as_str()) {
                None | Some("quit") => After::Quit,
                Some("stay") => After::Stay,
                Some(other) => {
                    return Err(bad(&format!("after is quit or stay, not {other}")).into());
                }
            };
            Ok(Launcher {
                name,
                cmd,
                mode,
                after,
            })
        }
        _ => Err(bad("expected a command string or a table").into()),
    }
}

/// What a new settings file should offer, from what is on this machine.
pub struct Programs {
    /// A Windows desktop rather than an SSH session or another system: GUI
    /// programs can open there.
    pub desktop: bool,
    pub fork: Option<PathBuf>,
    /// Names from `CLIENTS` whose program is on PATH.
    pub found: Vec<&'static str>,
}

/// Git clients offered in a new settings file when found on PATH: the
/// menu name, the program, the command line, and whether it runs in the
/// terminal. Fork is looked for separately, since it stays off PATH.
const CLIENTS: &[(&str, &str, &str, bool)] = &[
    ("sublime-merge", "smerge", "smerge \"{path}\"", false),
    ("github-desktop", "github", "github \"{path}\"", false),
    ("gitkraken", "gitkraken", "gitkraken -p \"{path}\"", false),
    ("lazygit", "lazygit", "lazygit", true),
    ("gitui", "gitui", "gitui", true),
    ("tig", "tig", "tig", true),
];

impl Programs {
    pub fn detect() -> Self {
        Self {
            desktop: cfg!(windows) && !crate::clip::over_ssh(),
            fork: fork_path(),
            found: CLIENTS
                .iter()
                .filter(|(_, program, _, _)| crate::launch::find(program).is_some())
                .map(|(name, _, _, _)| *name)
                .collect(),
        }
    }
}

/// One `[open]` line for a client from `CLIENTS`.
fn client_line(name: &str, cmd: &str, terminal: bool) -> String {
    if terminal {
        format!("{name} = {{ cmd = {}, mode = 'exec' }}\n", quote(cmd))
    } else {
        format!("{name} = {}\n", quote(cmd))
    }
}

/// The text of a new settings file with these roots. The git clients
/// found on this machine go in, whichever they are, and the others stay
/// as commented examples, so the menu is not tied to any one client. Over
/// SSH the menu holds terminal programs, since nothing with a window can
/// open there.
pub fn initial_text(roots: &[PathBuf], programs: &Programs, icons: bool) -> String {
    let roots = roots
        .iter()
        .map(|r| format!("  {},\n", quote(&r.to_string_lossy())))
        .collect::<String>();
    let exclude = DEFAULT_EXCLUDE
        .iter()
        .map(|s| quote(s))
        .collect::<Vec<_>>()
        .join(", ");
    let found = |name: &str| programs.found.contains(&name);
    let mut clients = String::new();
    let mut examples = String::new();
    if programs.desktop {
        match &programs.fork {
            Some(p) => clients.push_str(&format!(
                "fork = {}\n",
                quote(&format!("{} \"{{path}}\"", p.display()))
            )),
            None => examples.push_str("# fork = 'C:\\path\\to\\Fork.exe \"{path}\"'\n"),
        }
    }
    for (name, _, cmd, terminal) in CLIENTS {
        // Over SSH only terminal clients can open.
        if !programs.desktop && !terminal {
            continue;
        }
        let line = client_line(name, cmd, *terminal);
        if found(name) {
            clients.push_str(&line);
        } else {
            examples.push_str(&format!("# {line}"));
        }
    }
    if programs.desktop {
        // Not on PATH when installed, so never found; check the path.
        examples.push_str(
            "# tortoisegit = 'TortoiseGitProc.exe /command:log /path:\"{path}\"'\n\
             # sourcetree = 'C:\\path\\to\\SourceTree.exe -f \"{path}\"'\n",
        );
    }
    let tools = if programs.desktop {
        "editor = 'code \"{path}\"'\n\
         terminal = 'wt -w 0 nt -d \"{path}\"'\n\
         explorer = 'explorer.exe \"{path}\"'\n"
    } else {
        "# $EDITOR is your editor ($VISUAL, then $EDITOR, then vi).\n\
         editor = { cmd = '$EDITOR .', mode = 'exec' }\n"
    };
    format!(
        "\
# kura settings. `kura config` opens this file.

[scan]
# Folders to look for repositories in.
roots = [
{roots}]
max_depth = {DEFAULT_DEPTH}
exclude = [{exclude}]
# Repositories moved to the hidden tab (Ctrl-O). \"Hide\" in the action menu
# adds full paths here. Patterns work as in .gitignore: * within a folder
# name, ** across folders, and a pattern without a drive matches anywhere.
#   'C:\\work\\C\\**'   everything under that folder
#   'old_*'           any repository whose folder starts with old_
hidden = []

# Programs in the action menu (Ctrl-P), in this order. The name is what the
# menu shows, and its first free letter runs it. {{path}} is the repository's
# folder, {{name}} its name. A plain string starts the program on its own and
# quits kura. As a table, mode = 'exec' hands it the terminal and comes back
# when it exits, and after = 'stay' keeps kura open after starting it.
# Any git client works: give its command line for opening a repository.
[open]
{clients}{tools}
# Other clients, to uncomment and adjust:
{examples}

[ui]
# Language icons from a Nerd Font. Turn off if they show as boxes.
icons = {icons}
# The order the list opens in: used (zoxide history), changed or name.
# Ctrl-S switches it while kura is open.
sort = 'used'
# The name in the terminal's tab while open. Any text, such as 'notes';
# '' leaves the title as it is. The screen itself shows no name.
name = 'kura'
"
    )
}

/// A TOML string for `text`: a literal string when it can be one, since
/// Windows paths read better without doubled backslashes.
fn quote(text: &str) -> String {
    if !text.contains('\'') && !text.chars().any(char::is_control) {
        format!("'{text}'")
    } else {
        toml::Value::String(text.to_string()).to_string()
    }
}

/// Fork for Windows installs per user and stays off PATH.
fn fork_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let local = directories::BaseDirs::new()?.data_local_dir().to_path_buf();
    [
        local.join("Fork").join("current").join("Fork.exe"),
        local.join("Fork").join("Fork.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

pub fn write_initial(roots: &[PathBuf], icons: bool) -> Result<PathBuf, Error> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(&path, &initial_text(roots, &Programs::detect(), icons))?;
    Ok(path)
}

/// Adds repositories to `hidden` in the file in use, keeping its comments
/// and layout. `current` is the list in effect, which a profile's file
/// takes over the first time it gets one of its own.
pub fn add_hidden(current: &[String], repos: &[PathBuf]) -> Result<(), Error> {
    let path = target_path()?;
    let text = fs::read_to_string(&path)?;
    let updated = with_hidden(&text, current, repos)?;
    write_atomic(&path, &updated)?;
    Ok(())
}

/// Takes repositories out of `hidden`, however their paths were spelt there.
pub fn remove_hidden(current: &[String], repos: &[PathBuf]) -> Result<(), Error> {
    let path = target_path()?;
    let text = fs::read_to_string(&path)?;
    let updated = without_hidden(&text, current, repos)?;
    write_atomic(&path, &updated)?;
    Ok(())
}

fn without_hidden(text: &str, current: &[String], repos: &[PathBuf]) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let wanted: Vec<String> = repos.iter().map(|r| crate::repo::key(r)).collect();
    let hidden = seeded_list(&mut doc, "hidden", current)?;
    hidden.retain(|v| {
        v.as_str()
            .is_none_or(|s| !wanted.contains(&crate::repo::key(Path::new(s))))
    });
    one_per_line(hidden);
    Ok(doc.to_string())
}

/// `[scan]`'s list called `name`; if the file has none, a new one holding
/// `seed`, the list in effect until now, so a change does not drop it.
fn seeded_list<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    name: &str,
    seed: &[String],
) -> Result<&'a mut toml_edit::Array, Error> {
    let missing = doc.get("scan").and_then(|s| s.get(name)).is_none();
    let list = scan_list(doc, name)?;
    if missing {
        for entry in seed {
            list.push_formatted(path_value(entry));
        }
    }
    Ok(list)
}

fn with_hidden(text: &str, current: &[String], repos: &[PathBuf]) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let hidden = seeded_list(&mut doc, "hidden", current)?;
    for repo in repos {
        hidden.push_formatted(path_value(&repo.to_string_lossy()));
    }
    one_per_line(hidden);
    Ok(doc.to_string())
}

/// Replaces the folders to search in the file in use, keeping the rest of
/// the file as it is.
pub fn set_roots(roots: &[PathBuf]) -> Result<(), Error> {
    let path = target_path()?;
    let text = fs::read_to_string(&path)?;
    write_atomic(&path, &with_roots(&text, roots)?)?;
    Ok(())
}

fn with_roots(text: &str, roots: &[PathBuf]) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let list = scan_list(&mut doc, "roots")?;
    list.clear();
    for root in roots {
        list.push_formatted(path_value(&root.to_string_lossy()));
    }
    one_per_line(list);
    Ok(doc.to_string())
}

/// `[scan]`'s list called `name`, made if missing.
fn scan_list<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    name: &str,
) -> Result<&'a mut toml_edit::Array, Error> {
    doc.entry("scan")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or("[scan] is not a table")?
        .entry(name)
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or_else(|| format!("scan.{name} is not a list").into())
}

/// A path as a TOML value, a literal string where it can be one, so a
/// Windows path keeps single backslashes.
fn path_value(text: &str) -> toml_edit::Value {
    quote(text).parse().unwrap_or_else(|_| text.into())
}

/// One entry per line, as a new settings file is written.
fn one_per_line(list: &mut toml_edit::Array) {
    for value in list.iter_mut() {
        value.decor_mut().set_prefix("\n  ");
        value.decor_mut().set_suffix("");
    }
    list.set_trailing_comma(!list.is_empty());
    list.set_trailing(if list.is_empty() { "" } else { "\n" });
}

/// How to open the settings file: the editor from `[open]`, or else the
/// user's editor, given the terminal where nothing with a window can open.
pub fn editor_command(config: Option<&Config>) -> (String, Mode) {
    if let Some(editor) = config.and_then(|c| c.launchers.iter().find(|l| l.name == "editor")) {
        return (editor.cmd.clone(), editor.mode);
    }
    if cfg!(windows) && !crate::clip::over_ssh() {
        ("notepad \"{path}\"".into(), Mode::Detach)
    } else {
        ("$EDITOR \"{path}\"".into(), Mode::Exec)
    }
}

pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_initial_file_parses_back_with_its_roots() {
        let roots = vec![PathBuf::from(r"D:\dev"), PathBuf::from(r"C:\it's here")];
        let desktop = Programs {
            desktop: true,
            fork: Some(PathBuf::from(r"C:\Fork\Fork.exe")),
            found: vec!["sublime-merge", "gitui"],
        };
        let text = initial_text(&roots, &desktop, true);
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.roots, roots);
        assert_eq!(config.max_depth, DEFAULT_DEPTH);
        assert_eq!(config.icons, Some(true));
        let names: Vec<_> = config.launchers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "fork",
                "sublime-merge",
                "gitui",
                "editor",
                "terminal",
                "explorer"
            ]
        );
        // Clients not found stay as examples.
        assert!(text.contains("# gitkraken = 'gitkraken -p"));
        assert!(text.contains("# tortoisegit"));

        // Over SSH: terminal programs only, handed the terminal.
        let remote = Programs {
            desktop: false,
            fork: None,
            found: vec!["lazygit"],
        };
        let text = initial_text(&roots, &remote, false);
        let config = Config::parse(&text).unwrap();
        let names: Vec<_> = config.launchers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["lazygit", "editor"]);
        assert!(!text.contains("gitkraken") && text.contains("# tig = "));
        assert!(config.launchers.iter().all(|l| l.mode == Mode::Exec));
    }

    #[test]
    fn launchers_keep_their_order_and_options() {
        let config = Config::parse(
            r#"
[open]
zeta = 'z "{path}"'
alpha = { cmd = 'tig', mode = 'exec' }
mid = { cmd = 'code "{path}"', after = 'stay' }
"#,
        )
        .unwrap();
        let names: Vec<_> = config.launchers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["zeta", "alpha", "mid"]);
        assert_eq!(config.launchers[1].mode, Mode::Exec);
        assert_eq!(config.launchers[2].after, After::Stay);
        assert!(Config::parse("[open]\nx = { mode = 'exec' }").is_err());
        assert!(Config::parse("[open]\nx = { cmd = 'a', mode = 'run' }").is_err());
    }

    #[test]
    fn folders_are_replaced_one_per_line_and_comments_stay() {
        let text = "# mine\n[scan]\n# where\nroots = ['D:\\old']\nmax_depth = 6\n";
        let roots = [PathBuf::from(r"C:\work"), PathBuf::from(r"D:\it's")];
        let updated = with_roots(text, &roots).unwrap();
        assert!(updated.contains("# mine") && updated.contains("# where"));
        assert!(updated.contains("\n  'C:\\work',\n"), "{updated}");
        assert!(!updated.contains("old"));
        assert_eq!(Config::parse(&updated).unwrap().roots, roots);
    }

    #[test]
    fn a_profile_replaces_what_it_gives_and_keeps_the_rest() {
        let mut base: toml::Table = toml::from_str(
            "[scan]\nroots = ['/all']\nmax_depth = 3\n[open]\nfork = 'fork'\n[ui]\nsort = 'name'\n",
        )
        .unwrap();
        let over: toml::Table =
            toml::from_str("[scan]\nroots = ['/work']\nhidden = ['old_*']\n").unwrap();
        lay_over(&mut base, over);
        let config = Config::from_table(base).unwrap();
        assert_eq!(config.roots, [PathBuf::from("/work")]);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.launchers[0].name, "fork");
        assert_eq!(config.sort, crate::rank::Sort::Name);
        assert!(config.hidden.contains(Path::new("/x/old_one")));
        let made = profile_text("work", &[PathBuf::from(r"C:\work")]);
        assert_eq!(
            Config::parse(&made).unwrap().roots,
            [PathBuf::from(r"C:\work")]
        );
    }

    #[test]
    fn the_shown_name_defaults_to_kura_and_can_be_anything_or_nothing() {
        assert_eq!(Config::parse("").unwrap().name, "kura");
        assert_eq!(
            Config::parse(
                "[ui]
name = 'notes'"
            )
            .unwrap()
            .name,
            "notes"
        );
        assert_eq!(
            Config::parse(
                "[ui]
name = ''"
            )
            .unwrap()
            .name,
            ""
        );
    }

    #[test]
    fn one_setting_is_set_as_toml_or_as_text() {
        let text = "# mine\n[ui]\nicons = false\n";
        let set = with_value(text, "ui", "name", "notes").unwrap();
        assert!(set.contains("# mine") && set.contains("name = \"notes\""));
        let set = with_value(&set, "ui", "icons", "true").unwrap();
        assert_eq!(Config::parse(&set).unwrap().icons, Some(true));
        let set = with_value(
            &set,
            "open",
            "lazygit",
            "{ cmd = 'lazygit', mode = 'exec' }",
        )
        .unwrap();
        let config = Config::parse(&set).unwrap();
        assert_eq!(config.launchers[0].mode, Mode::Exec);
        assert!(
            split_key("ui.nmae")
                .unwrap_err()
                .to_string()
                .contains("unknown setting")
        );
        assert!(split_key("open.fork").is_ok());
        assert!(split_key("name").is_err());
    }

    #[test]
    fn the_start_profile_is_set_and_cleared_in_place() {
        let text = "# mine\n[ui]\nicons = false\n";
        let set = with_default_profile(text, Some("work")).unwrap();
        assert!(set.contains("# mine") && set.contains("profile = \"work\""));
        let cleared = with_default_profile(&set, None).unwrap();
        assert!(!cleared.contains("profile") && cleared.contains("icons = false"));
    }

    #[test]
    fn profile_names_are_file_and_command_line_safe() {
        assert!(valid_profile_name("work"));
        assert!(valid_profile_name("demo-2_仕事"));
        assert!(!valid_profile_name(""));
        assert!(!valid_profile_name("a b"));
        assert!(!valid_profile_name("../x"));
    }

    #[test]
    fn a_first_hide_in_a_profile_keeps_the_list_in_effect() {
        let profile = "[scan]\nroots = ['/work']\n";
        let current = vec!["old_*".to_string()];
        let updated = with_hidden(profile, &current, &[PathBuf::from("/work/junk")]).unwrap();
        let config = Config::parse(&updated).unwrap();
        assert!(config.hidden.contains(Path::new("/x/old_one")));
        assert!(config.hidden.contains(Path::new("/work/junk")));
    }

    #[test]
    fn hiding_keeps_comments_and_appends() {
        let text = "# mine\n[scan]\nroots = ['D:\\dev'] # here\nhidden = []\n";
        let updated = with_hidden(text, &[], &[PathBuf::from(r"D:\dev\junk")]).unwrap();
        assert!(updated.contains("# mine"));
        assert!(updated.contains("# here"));
        let config = Config::parse(&updated).unwrap();
        assert!(config.hidden.contains(Path::new(r"D:\dev\junk")));
        let restored = without_hidden(&updated, &[], &[PathBuf::from(r"D:\dev\junk")]).unwrap();
        assert!(restored.contains("# here"));
        let config = Config::parse(&restored).unwrap();
        assert!(!config.hidden.contains(Path::new(r"D:\dev\junk")));
        assert!(Config::parse("[scan]\nhidden = ['[bad']").is_err());
    }
}
