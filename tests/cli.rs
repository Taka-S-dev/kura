//! The commands an agent would run for "make a work profile that searches
//! only the company folder and hides old ones", against settings in a
//! temporary folder through KURA_CONFIG_DIR, never the real ones.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn kura(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kura"))
        .args(args)
        .env("KURA_CONFIG_DIR", dir.join("settings"))
        .env_remove("KURA_PROFILE")
        .env_remove("KURA_QUERY")
        .output()
        .expect("kura runs")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = kura(dir, args);
    assert!(
        out.status.success(),
        "kura {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo(path: &Path) {
    fs::create_dir_all(path.join(".git")).unwrap();
    fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[test]
fn an_agent_makes_and_manages_a_profile_without_the_screen() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let company = dir.join("company");
    let private = dir.join("private");
    repo(&company.join("billing"));
    repo(&company.join("old_portal"));
    repo(&private.join("diary"));
    fs::create_dir_all(dir.join("settings")).unwrap();
    fs::write(
        dir.join("settings/config.toml"),
        format!(
            "[scan]\nroots = [{}, {}]\n",
            quoted(&company),
            quoted(&private)
        ),
    )
    .unwrap();

    // Made with only the company folder, not a copy of every folder.
    let file = ok(
        dir,
        &[
            "profile",
            "new",
            "work",
            "--root",
            company.to_str().unwrap(),
        ],
    );
    assert!(file.trim().ends_with("work.toml"));
    let roots: serde_json::Value =
        serde_json::from_str(&ok(dir, &["--profile", "work", "roots", "--json"])).unwrap();
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(roots[0]["repos"], 0, "nothing scanned yet");

    ok(dir, &["--profile", "work", "hide", "old_*"]);
    let hidden = ok(dir, &["--profile", "work", "hidden"]);
    assert_eq!(hidden.trim(), "pattern\told_*");
    let listed = ok(dir, &["--profile", "work", "list"]);
    assert!(listed.contains("billing"), "{listed}");
    assert!(
        !listed.contains("old_portal") && !listed.contains("diary"),
        "{listed}"
    );
    // The shared settings stay as they were.
    assert!(ok(dir, &["list"]).contains("diary"));

    // kura starts in it, follows a rename, and forgets it once removed.
    ok(dir, &["profile", "default", "work"]);
    assert!(ok(dir, &["profile", "list"]).contains("* work"));
    ok(dir, &["profile", "rename", "work", "office"]);
    assert!(ok(dir, &["profile", "list"]).contains("* office"));
    ok(dir, &["profile", "remove", "office"]);
    assert!(ok(dir, &["profile", "list"]).contains("* default"));
    assert!(ok(dir, &["config", "check"]).starts_with("ok:"));

    // One setting at a time: the name in the terminal's title. Display
    // settings are shared, so they go to config.toml even inside a profile.
    ok(dir, &["profile", "new", "demo"]);
    let out = kura(
        dir,
        &["--profile", "demo", "config", "set", "ui.name", "notes"],
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("config.toml"));
    assert_eq!(ok(dir, &["config", "get", "ui.name"]).trim(), "\"notes\"");
    ok(dir, &["config", "set", "ui.name", "notes"]);
    assert_eq!(ok(dir, &["config", "get", "ui.name"]).trim(), "\"notes\"");
    assert_eq!(
        kura(dir, &["config", "set", "ui.sort", "sideways"])
            .status
            .code(),
        Some(2),
        "a value kura cannot use is refused"
    );
    assert_eq!(
        kura(dir, &["config", "set", "ui.nmae", "x"]).status.code(),
        Some(2)
    );

    // Mistakes are errors with exit code 2, and change nothing.
    let missing = kura(dir, &["--profile", "nope", "list"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no profile named nope"));
    assert_eq!(
        kura(dir, &["profile", "new", "bad name"]).status.code(),
        Some(2)
    );
    assert_eq!(kura(dir, &["hide", "[broken"]).status.code(), Some(2));
    assert!(ok(dir, &["hidden"]).trim().is_empty());
}
