mod app;
mod cache;
mod cli;
mod clip;
mod config;
mod first_run;
mod gitstat;
mod hidden;
mod icons;
mod keys;
mod lang;
mod launch;
mod rank;
mod repo;
mod scan;
mod shim;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use app::{App, Outcome};
use config::Config;

/// Finds the Git repositories on this PC and jumps to them.
///
/// Without a command, kura opens the list. The other commands change the
/// settings without the screen, for scripts and agents: they print results
/// on stdout and problems on stderr, and exit 0 when they succeed.
#[derive(Parser)]
#[command(name = "kura", version, about, long_about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// The profile to use: a file in profiles\ beside config.toml, with
    /// its own folders to search and hidden list. KURA_PROFILE does the
    /// same, and [ui] profile in config.toml sets the one to start in.
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Choose a repository and print its path, for the shell function to cd into.
    Pick {
        /// Initial query. The KURA_QUERY environment variable takes precedence.
        query: Vec<String>,
    },
    /// Print every repository in the list, one path per line.
    List {
        /// Print name, path and branch as JSON instead.
        #[arg(long)]
        json: bool,
    },
    /// The folders searched for repositories. Without an action, lists them.
    Roots {
        #[command(subcommand)]
        action: Option<RootsAction>,
        /// List as JSON, with whether each exists and how many repositories it holds.
        #[arg(long)]
        json: bool,
    },
    /// Add a folder to search; the current one by default. Same as `roots add`.
    Add { path: Option<PathBuf> },
    /// Move repositories to the hidden tab: paths, or .gitignore-style
    /// patterns such as 'C:\work\old\**' or 'old_*'.
    Hide {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// List what the profile in use hides: paths and patterns.
    Hidden {
        #[arg(long)]
        json: bool,
    },
    /// Bring hidden repositories back into the list.
    Show {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// The settings file. Without an action, opens it in the editor from [open].
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Profiles: each a file with its own folders to search and hidden list.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Print the keys of the list screen.
    Keys,
    /// Print shell integration code: a function that cds into the chosen
    /// repository. Add `Invoke-Expression (& kura init powershell | Out-String)`
    /// to $PROFILE, or `eval "$(kura init bash)"` to ~/.bashrc.
    Init {
        #[arg(value_enum)]
        shell: Shell,
        /// The name of the shell function.
        #[arg(long, default_value = "kr")]
        name: String,
    },
}

#[derive(Subcommand)]
enum RootsAction {
    /// List the folders searched.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a folder; the current one by default.
    Add { path: Option<PathBuf> },
    /// Stop searching a folder.
    Remove { path: PathBuf },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the path of the settings file in use, to edit it directly.
    Path,
    /// Read the settings as kura would and report what is wrong.
    Check,
    /// Open the settings file in the editor from [open].
    Edit,
    /// Print one setting in effect, as TOML. KEY is section.key, such as ui.name.
    Get { key: String },
    /// Change one setting in the file in use, such as `ui.name notes` or
    /// `ui.sort changed`. VALUE is TOML, or plain text for a string.
    /// Nothing is written if the result would not load.
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum ProfileAction {
    /// List the profiles; the one in use is marked.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Make a profile and print its file. It searches the --root folders,
    /// or without any, the folders of the profile in use.
    New {
        name: String,
        /// A folder for the new profile to search; give it once per folder.
        #[arg(long = "root", value_name = "PATH")]
        roots: Vec<PathBuf>,
    },
    /// Set the profile kura starts in, or `none` for config.toml alone.
    Default { name: String },
    /// Delete a profile and its cache.
    Remove { name: String },
    /// Rename a profile, keeping its cache and the start setting.
    Rename { old: String, new: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum Shell {
    Powershell,
    Bash,
    Zsh,
}

const EXIT_CANCELLED: u8 = 1;
const EXIT_ERROR: u8 = 2;

type Error = Box<dyn std::error::Error>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let profile = cli.profile;
    let done = |result: Result<(), Error>| result.map(|()| Outcome::Done);
    let result = match cli.command.unwrap_or(Command::Pick { query: Vec::new() }) {
        Command::Pick { query } => pick(query, profile),
        Command::Init { shell, name } => done(init(shell, &name)),
        Command::Keys => {
            cli::keys();
            Ok(Outcome::Done)
        }
        Command::Config {
            action: None | Some(ConfigAction::Edit),
        } => done(cli::config_edit(profile)),
        // Managing profiles must work even when the start profile is the
        // one that is broken or gone, so these do not choose one first.
        Command::Profile {
            action: ProfileAction::Default { name },
        } => done(cli::profile_default(&name)),
        Command::Profile {
            action: ProfileAction::Remove { name },
        } => done(cli::profile_remove(&name)),
        Command::Profile {
            action: ProfileAction::Rename { old, new },
        } => done(cli::profile_rename(&old, &new)),
        command => done(
            config::choose_profile(profile).and_then(|()| match command {
                Command::List { json } => cli::list(json),
                Command::Roots { action: None, json } => cli::roots_list(json),
                Command::Roots {
                    action: Some(RootsAction::List { json }),
                    ..
                } => cli::roots_list(json),
                Command::Roots {
                    action: Some(RootsAction::Add { path }),
                    ..
                }
                | Command::Add { path } => cli::roots_add(path),
                Command::Roots {
                    action: Some(RootsAction::Remove { path }),
                    ..
                } => cli::roots_remove(path),
                Command::Hide { targets } => cli::hide(&targets),
                Command::Show { targets } => cli::show(&targets),
                Command::Config {
                    action: Some(ConfigAction::Path),
                } => cli::config_path(),
                Command::Config {
                    action: Some(ConfigAction::Check),
                } => cli::config_check(),
                Command::Config {
                    action: Some(ConfigAction::Get { key }),
                } => cli::config_get(&key),
                Command::Config {
                    action: Some(ConfigAction::Set { key, value }),
                } => cli::config_set(&key, &value),
                Command::Profile {
                    action: ProfileAction::List { json },
                } => cli::profile_list(json),
                Command::Profile {
                    action: ProfileAction::New { name, roots },
                } => cli::profile_new(&name, roots),
                Command::Hidden { json } => cli::hidden(json),
                Command::Pick { .. }
                | Command::Init { .. }
                | Command::Keys
                | Command::Config { .. }
                | Command::Profile { .. } => {
                    unreachable!("handled above")
                }
            }),
        ),
    };
    match result {
        Ok(Outcome::Cd(path)) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Ok(Outcome::Done) => ExitCode::SUCCESS,
        Ok(Outcome::Cancelled) => ExitCode::from(EXIT_CANCELLED),
        Err(err) => {
            eprintln!("kura: {err}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn pick(words: Vec<String>, profile: Option<String>) -> Result<Outcome, Error> {
    let query = std::env::var("KURA_QUERY").unwrap_or_else(|_| words.join(" "));
    // The shell function hands everything over as the query, so
    // `kr --profile work bat` arrives here as text.
    let (profile, query) = match query.trim_start().strip_prefix("--profile ") {
        Some(rest) => {
            let rest = rest.trim_start();
            let (name, query) = rest.split_once(' ').unwrap_or((rest, ""));
            (profile.or(Some(name.to_string())), query.to_string())
        }
        None => (profile, query),
    };
    config::choose_profile(profile)?;
    let app = match Config::load()? {
        None => {
            eprintln!("kura: first run, looking for repositories on this PC…");
            let (candidates, found) = first_run::discover();
            App::setup(candidates, found, query)
        }
        Some(config) => {
            let app = App::list(config, cache::load(), query);
            // `kr baton` goes straight there when nothing else could be meant.
            if let Some(path) = app.direct() {
                return Ok(Outcome::Cd(path));
            }
            app
        }
    };
    app::run(app)
}

fn init(shell: Shell, name: &str) -> Result<(), Error> {
    match shell {
        Shell::Powershell => print!("{}", shim::powershell(name)?),
        Shell::Bash | Shell::Zsh => print!("{}", shim::posix(name)?),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn commands_parse_as_documented() {
        assert!(parse(&["kura"]).command.is_none());
        assert!(matches!(
            parse(&["kura", "roots", "remove", "D:/x"]).command,
            Some(Command::Roots {
                action: Some(RootsAction::Remove { .. }),
                ..
            })
        ));
        assert!(matches!(
            parse(&["kura", "--profile", "work", "hide", "old_*", "C:/a"]).command,
            Some(Command::Hide { ref targets }) if targets.len() == 2
        ));
        assert_eq!(
            parse(&["kura", "config", "path", "--profile", "w"])
                .profile
                .as_deref(),
            Some("w")
        );
        assert!(Cli::try_parse_from(["kura", "hide"]).is_err());
        assert!(matches!(
            parse(&["kura", "profile", "new", "demo"]).command,
            Some(Command::Profile {
                action: ProfileAction::New { .. }
            })
        ));
    }
}
