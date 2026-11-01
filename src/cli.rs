//! The commands that run without the screen, for scripts and agents as well
//! as people: results on stdout, problems on stderr, and the exit code says
//! which. Every change goes to the file of the profile in use.

use std::path::{Path, PathBuf};

use crate::config::{self, Config};
use crate::hidden::Rules;
use crate::{cache, launch, repo, scan};

type Error = Box<dyn std::error::Error>;

fn loaded() -> Result<Config, Error> {
    Config::load()?.ok_or_else(|| "no settings yet; run `kura` once to set up".into())
}

pub fn list(json: bool) -> Result<(), Error> {
    let config = loaded()?;
    let mut entries = cache::load();
    if entries.is_empty() {
        let options = scan::Options {
            max_depth: config.max_depth,
            exclude: config.exclude.clone(),
        };
        let (found, _) = scan::collect(&config.roots, &options);
        entries = cache::entries_from(&found, cache::now());
        cache::save(&entries)?;
    }
    let repos: Vec<repo::Repo> = entries
        .into_iter()
        .filter(|e| !config.hidden.contains(&e.path) && repo::is_repo(&e.path))
        .map(|e| repo::Repo::inspect(e.path))
        .collect();
    if json {
        let rows: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "path": r.path,
                    "branch": r.branch,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for repo in repos {
            println!("{}", repo.path.display());
        }
    }
    Ok(())
}

pub fn roots_list(json: bool) -> Result<(), Error> {
    let config = loaded()?;
    let cached = cache::load();
    let rows: Vec<(PathBuf, bool, usize)> = config
        .roots
        .iter()
        .map(|root| {
            let k = repo::key(root);
            let repos = cached
                .iter()
                .filter(|e| repo::key_within(&repo::key(&e.path), &k))
                .count();
            (root.clone(), root.is_dir(), repos)
        })
        .collect();
    if json {
        let rows: Vec<serde_json::Value> = rows
            .iter()
            .map(|(path, exists, repos)| {
                serde_json::json!({ "path": path, "exists": exists, "repos": repos })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for (path, exists, _) in rows {
            let note = if exists { "" } else { "\t(not found)" };
            println!("{}{note}", path.display());
        }
    }
    Ok(())
}

fn absolute(path: Option<PathBuf>) -> Result<PathBuf, Error> {
    Ok(std::path::absolute(match path {
        Some(path) => path,
        None => std::env::current_dir()?,
    })?)
}

pub fn roots_add(path: Option<PathBuf>) -> Result<(), Error> {
    let config = loaded()?;
    let path = absolute(path)?;
    if !path.is_dir() {
        return Err(format!("not a folder: {}", path.display()).into());
    }
    let k = repo::key(&path);
    if let Some(root) = config
        .roots
        .iter()
        .find(|r| repo::key_within(&k, &repo::key(r)))
    {
        eprintln!("Already searched, as part of {}", root.display());
        return Ok(());
    }
    let mut roots = config.roots.clone();
    roots.push(path.clone());
    config::set_roots(&roots)?;
    eprintln!("Added {}", path.display());
    Ok(())
}

pub fn roots_remove(path: PathBuf) -> Result<(), Error> {
    let config = loaded()?;
    let k = repo::key(&absolute(Some(path.clone()))?);
    let kept: Vec<PathBuf> = config
        .roots
        .iter()
        .filter(|r| repo::key(r) != k)
        .cloned()
        .collect();
    if kept.len() == config.roots.len() {
        return Err(format!(
            "not a folder kura searches: {} (kura roots lists them)",
            path.display()
        )
        .into());
    }
    config::set_roots(&kept)?;
    eprintln!("Removed {}", path.display());
    Ok(())
}

/// A path to hide or show as given on the command line: a pattern stays as
/// written, a path is made absolute, so `kura hide .` means this folder.
fn target(given: &str) -> Result<PathBuf, Error> {
    if given.contains(['*', '?', '[', '{']) {
        Ok(PathBuf::from(given))
    } else {
        absolute(Some(PathBuf::from(given)))
    }
}

pub fn hide(given: &[String]) -> Result<(), Error> {
    let config = loaded()?;
    let mut new = Vec::new();
    for g in given {
        let t = target(g)?;
        let written = config
            .hidden_entries
            .iter()
            .any(|e| repo::key(Path::new(e)) == repo::key(&t));
        if written {
            eprintln!("Already hidden: {}", t.display());
        } else {
            new.push(t);
        }
    }
    if new.is_empty() {
        return Ok(());
    }
    // A broken pattern is refused before anything is written.
    let mut entries = config.hidden_entries.clone();
    entries.extend(new.iter().map(|p| p.to_string_lossy().into_owned()));
    Rules::parse(&entries)?;
    config::add_hidden(&config.hidden_entries, &new)?;
    for t in new {
        eprintln!("Hid {}", t.display());
    }
    Ok(())
}

pub fn show(given: &[String]) -> Result<(), Error> {
    let config = loaded()?;
    let targets: Vec<PathBuf> = given.iter().map(|g| target(g)).collect::<Result<_, _>>()?;
    config::remove_hidden(&config.hidden_entries, &targets)?;
    // What a pattern still hides is said, since only the pattern can undo it.
    let after = loaded()?;
    for t in targets {
        match after.hidden.pattern(&t) {
            Some(pattern) => eprintln!("Still hidden by the pattern {pattern}: {}", t.display()),
            None => eprintln!("Showing {}", t.display()),
        }
    }
    Ok(())
}

pub fn config_get(key: &str) -> Result<(), Error> {
    match config::get_value(key)? {
        Some(value) => println!("{value}"),
        None => eprintln!("{key} is not set; kura uses its default"),
    }
    Ok(())
}

pub fn config_set(key: &str, value: &str) -> Result<(), Error> {
    let file = config::set_value(key, value)?;
    eprintln!("Set {key} in {}", file.display());
    Ok(())
}

/// The settings file in use, for an agent or script to edit directly.
pub fn config_path() -> Result<(), Error> {
    println!("{}", config::target_path()?.display());
    Ok(())
}

/// Reads the settings as kura would and reports what is wrong. Errors stop
/// kura from starting and exit 2; warnings are things it works around.
pub fn config_check() -> Result<(), Error> {
    let config = loaded()?;
    let mut warnings = Vec::new();
    for root in &config.roots {
        if !root.is_dir() {
            warnings.push(format!("folder not found: {}", root.display()));
        }
    }
    for launcher in &config.launchers {
        let Ok(argv) = launch::command_line(&launcher.cmd, Path::new("."), "x") else {
            continue;
        };
        let program = &argv[0];
        let found = program == "$EDITOR"
            || launch::find(program).is_some()
            || Path::new(program).is_file()
            // App execution aliases such as wt.exe resolve only when started.
            || (cfg!(windows) && !program.contains(['\\', '/']));
        if !found {
            warnings.push(format!(
                "[open] {}: program not found: {program}",
                launcher.name
            ));
        }
    }
    println!("ok: {}", config::target_path()?.display());
    for warning in &warnings {
        println!("warning: {warning}");
    }
    Ok(())
}

pub fn profile_list(json: bool) -> Result<(), Error> {
    let active = config::profile();
    let names = config::profiles();
    if json {
        let mut rows = vec![serde_json::json!({
            "name": null,
            "active": active.is_none(),
            "path": config::path()?,
        })];
        for name in &names {
            rows.push(serde_json::json!({
                "name": name,
                "active": active.as_deref() == Some(name.as_str()),
                "path": config::profile_path(name)?,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        let mark = |on: bool| if on { "* " } else { "  " };
        println!("{}default (config.toml only)", mark(active.is_none()));
        for name in names {
            println!("{}{name}", mark(active.as_deref() == Some(name.as_str())));
        }
    }
    Ok(())
}

/// Makes a profile and prints its file. It searches `roots` when given,
/// and otherwise the folders of the profile in use.
pub fn profile_new(name: &str, roots: Vec<PathBuf>) -> Result<(), Error> {
    let roots = if roots.is_empty() {
        loaded()?.roots
    } else {
        let roots = roots
            .into_iter()
            .map(|r| absolute(Some(r)))
            .collect::<Result<Vec<_>, _>>()?;
        for root in &roots {
            if !root.is_dir() {
                eprintln!("Note: {} is not there now", root.display());
            }
        }
        roots
    };
    let file = config::create_profile(name, &roots)?;
    println!("{}", file.display());
    Ok(())
}

fn existing_profile(name: &str) -> Result<PathBuf, Error> {
    let file = config::profile_path(name)?;
    if !file.is_file() {
        return Err(format!("no profile named {name} (kura profile list shows them)").into());
    }
    Ok(file)
}

/// Sets the profile kura starts in; `none` goes back to config.toml alone.
pub fn profile_default(name: &str) -> Result<(), Error> {
    if name == "none" {
        config::set_default_profile(None)?;
        eprintln!("kura starts without a profile");
        return Ok(());
    }
    existing_profile(name)?;
    config::set_default_profile(Some(name))?;
    eprintln!("kura starts in profile {name}");
    Ok(())
}

pub fn profile_remove(name: &str) -> Result<(), Error> {
    let file = existing_profile(name)?;
    std::fs::remove_file(&file)?;
    let _ = std::fs::remove_file(cache::path_for(Some(name))?);
    // A start profile that is gone would stop kura from starting.
    if config::default_profile().as_deref() == Some(name) {
        config::set_default_profile(None)?;
        eprintln!("kura no longer starts in it");
    }
    eprintln!("Removed profile {name}");
    Ok(())
}

pub fn profile_rename(old: &str, new: &str) -> Result<(), Error> {
    let from = existing_profile(old)?;
    if !config::valid_profile_name(new) {
        return Err(
            format!("not usable as a profile name: {new} (letters, digits, - and _)").into(),
        );
    }
    let to = config::profile_path(new)?;
    if to.exists() {
        return Err(format!("there is a profile named {new} already").into());
    }
    std::fs::rename(&from, &to)?;
    let _ = std::fs::rename(cache::path_for(Some(old))?, cache::path_for(Some(new))?);
    if config::default_profile().as_deref() == Some(old) {
        config::set_default_profile(Some(new))?;
    }
    eprintln!("Renamed profile {old} to {new}");
    Ok(())
}

/// What the profile in use hides: full paths, which the menu and `kura
/// show` undo, and patterns, which only an edit of the settings undoes.
pub fn hidden(json: bool) -> Result<(), Error> {
    let config = loaded()?;
    let pattern = |e: &str| e.contains(['*', '?', '[', '{']);
    if json {
        let rows: Vec<serde_json::Value> = config
            .hidden_entries
            .iter()
            .map(|e| serde_json::json!({ "entry": e, "pattern": pattern(e) }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for entry in &config.hidden_entries {
            let kind = if pattern(entry) { "pattern" } else { "path" };
            println!("{kind}\t{entry}");
        }
    }
    Ok(())
}

pub fn keys() {
    let width = crate::keys::LIST
        .iter()
        .map(|(k, _)| k.len())
        .max()
        .unwrap_or(0);
    for (key, what) in crate::keys::LIST {
        println!("{key:<width$}  {what}");
    }
}

/// Opens the settings in use with the editor from [open]: the profile's
/// file with --profile, made from the current folders the first time.
pub fn config_edit(profile: Option<String>) -> Result<(), Error> {
    if !config::path()?.exists() {
        return Err(format!(
            "no settings yet at {}; run `kura` once to set up",
            config::path()?.display()
        )
        .into());
    }
    if let Some(name) = &profile
        && !config::profile_path(name)?.exists()
    {
        let roots = Config::load()?.map(|c| c.roots).unwrap_or_default();
        config::create_profile(name, &roots)?;
        eprintln!("Made profile {name}");
    }
    config::choose_profile(profile)?;
    let path = config::target_path()?;
    eprintln!("{}", path.display());
    let (editor, mode) = config::editor_command(Config::load()?.as_ref());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let argv = launch::command_line(&editor, &path, &name)?;
    let dir = path.parent().unwrap_or(&path).to_path_buf();
    if mode == config::Mode::Exec {
        launch::exec(&argv, &dir)?;
    } else {
        launch::detach(&argv, &dir)?;
    }
    Ok(())
}
