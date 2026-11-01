//! Nerd Font icons for the language column, shown only when `[ui] icons`
//! asks for them. Which font the terminal draws with cannot be asked from
//! inside it, and Windows Terminal shrinks these icons to one cell, where
//! the coloured two-letter tags read better.
//! Codepoints: <https://github.com/ryanoasis/nerd-fonts/blob/master/glyphnames.json>

/// The icon for a language tag from `Repo::lang`. An unknown language gets
/// none: every row is a Git repository, so a Git icon would say nothing and
/// only crowd out the icons that do.
pub fn glyph(lang: Option<&str>) -> &'static str {
    match lang {
        Some("rs") => "\u{e7a8}",
        Some("go") => "\u{e627}",
        Some("ts") => "\u{e628}",
        Some("js") => "\u{e74e}",
        Some("py") => "\u{e73c}",
        Some("cs") => "\u{e648}",
        Some("c") => "\u{e61e}",
        Some("md") => "\u{e73e}",
        Some("cp") => "\u{e61d}",
        Some("ja") => "\u{e738}",
        Some("kt") => "\u{e634}",
        Some("rb") => "\u{e739}",
        Some("ph") => "\u{e73d}",
        Some("sw") => "\u{e755}",
        Some("lu") => "\u{e620}",
        Some("ps") => "\u{ebc7}",
        Some("sh") => "\u{e795}",
        Some("ht") => "\u{e736}",
        _ => "",
    }
}

/// Whether a Nerd Font seems to be at hand, to suggest trying the icons on
/// first run. Over SSH the font is on the other end, so the answer is no.
pub fn detect() -> bool {
    !crate::clip::over_ssh() && installed()
}

#[cfg(windows)]
fn installed() -> bool {
    let mut dirs = vec![
        std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("Fonts"),
    ];
    if let Some(base) = directories::BaseDirs::new() {
        // Fonts installed for one user only.
        dirs.push(
            base.data_local_dir()
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    dirs.iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| is_nerd_font_file(&e.file_name().to_string_lossy()))
        })
    })
}

#[cfg(not(windows))]
fn installed() -> bool {
    let mut command = std::process::Command::new("fc-list");
    command.arg(":family");
    command.output().is_ok_and(|out| {
        String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains("nerd font")
    })
}

/// Nerd Fonts name their files `<Family>NerdFont-<Style>`, with `Mono` or
/// `Propo` variants, and Cascadia ships its own as `CaskaydiaCove`.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_nerd_font_file(name: &str) -> bool {
    let name = name.to_lowercase();
    let font = [".ttf", ".otf"].iter().any(|ext| name.ends_with(ext));
    font && (name.contains("nerdfont") || name.starts_with("caskaydia"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nerd_font_files_are_told_from_others() {
        assert!(is_nerd_font_file("JetBrainsMonoNLNerdFont-Bold.ttf"));
        assert!(is_nerd_font_file("HackNerdFontMono-Regular.TTF"));
        assert!(is_nerd_font_file("CaskaydiaCoveNerdFont-Regular.ttf"));
        assert!(!is_nerd_font_file("CascadiaCode.ttf"));
        assert!(!is_nerd_font_file("nerdfont-readme.txt"));
    }

    #[test]
    fn every_language_has_its_own_icon() {
        let tags = ["rs", "go", "ts", "js", "py", "cs", "c", "md"];
        let mut seen: Vec<&str> = tags.iter().map(|t| glyph(Some(t))).collect();
        assert_eq!(glyph(None), "");
        let count = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), count);
    }
}
