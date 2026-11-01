//! Starting the programs in the action menu: the command line from the
//! settings is split here and handed over as separate arguments, never
//! through cmd.exe, so `&` or `^` in a path cannot break it.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// Splits a command template into words. Double or single quotes group a
/// word with spaces in it; backslashes are ordinary characters, as in
/// Windows paths.
pub fn split(template: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in template.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(format!("unclosed quote in: {template}"));
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err("empty command".into());
    }
    Ok(words)
}

/// The command line for one repository. Placeholders are filled in after
/// splitting, so a path with spaces or quotes stays one argument.
pub fn command_line(template: &str, repo: &Path, name: &str) -> Result<Vec<String>, String> {
    let path = repo.to_string_lossy();
    let mut argv = Vec::new();
    for word in split(template)? {
        // $EDITOR stands for the user's editor, with any arguments it has,
        // so one settings file works on every machine it is copied to.
        if word == "$EDITOR" {
            argv.extend(split(&editor())?);
            continue;
        }
        argv.push(word.replace("{path}", &path).replace("{name}", name));
    }
    Ok(argv)
}

fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty()))
        .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.into())
}

/// Finds a program on PATH the way a shell would, including `.cmd` files
/// such as VS Code's `code`, which `Command` alone does not look for.
pub fn find(program: &str) -> Option<PathBuf> {
    let given = Path::new(program);
    if given.components().count() > 1 {
        return given.is_file().then(|| given.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let bare = !cfg!(windows) || given.extension().is_some();
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if bare && dir.join(program).is_file() {
            return Some(dir.join(program));
        }
        for ext in &exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve(program: &str) -> PathBuf {
    // App execution aliases such as wt.exe are not regular files; leave
    // the lookup to the system.
    find(program).unwrap_or_else(|| PathBuf::from(program))
}

fn build(argv: &[String], cwd: &Path) -> Command {
    let mut command = Command::new(resolve(&argv[0]));
    command.args(&argv[1..]).current_dir(cwd);
    command
}

/// Starts a GUI program and leaves it running on its own.
pub fn detach(argv: &[String], cwd: &Path) -> std::io::Result<()> {
    let mut command = build(argv, cwd);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A .cmd wrapper such as code.cmd would otherwise flash a console window.
    no_window(&mut command);
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Runs a terminal program in this terminal and waits for it.
pub fn exec(argv: &[String], cwd: &Path) -> std::io::Result<ExitStatus> {
    build(argv, cwd).status()
}

#[cfg(windows)]
pub fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn no_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_group_words_and_backslashes_stay() {
        assert_eq!(
            split(r#"C:\Fork\Fork.exe "{path}""#).unwrap(),
            [r"C:\Fork\Fork.exe", "{path}"]
        );
        assert_eq!(
            split(r#"wt -w 0 nt -d '{path}'"#).unwrap(),
            ["wt", "-w", "0", "nt", "-d", "{path}"]
        );
        assert_eq!(
            split(r#""C:\Program Files\x.exe" a"#).unwrap(),
            [r"C:\Program Files\x.exe", "a"]
        );
        assert!(split(r#"code "{path}"#).is_err());
        assert!(split("   ").is_err());
    }

    #[test]
    fn editor_stands_in_for_the_users_editor_with_its_arguments() {
        let argv = command_line("$EDITOR .", Path::new("/r"), "r").unwrap();
        assert_eq!(argv.last().map(String::as_str), Some("."));
        assert_eq!(argv[..argv.len() - 1], split(&editor()).unwrap()[..]);
    }

    #[test]
    fn a_path_with_spaces_and_ampersands_stays_one_argument() {
        let argv = command_line(
            r#"code "{path}" --name {name}"#,
            Path::new(r"D:\a & b\x ^y"),
            "x ^y",
        )
        .unwrap();
        assert_eq!(argv, ["code", r"D:\a & b\x ^y", "--name", "x ^y"]);
    }
}
