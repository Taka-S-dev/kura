//! Which key events count as typed text rather than shortcuts.

use crossterm::event::{KeyEvent, KeyModifiers};

/// The list screen's keys and what they do, for the help page and for
/// `kura keys`, so the two cannot disagree.
pub const LIST: &[(&str, &str)] = &[
    ("enter", "cd into the repository"),
    ("ctrl-p", "action menu: open in Fork, an editor, lazygit…"),
    ("tab  shift-tab", "mark, then act on every marked one"),
    ("ctrl-a", "mark all in view; again to clear"),
    ("ctrl-j  ctrl-k", "move down, up"),
    (
        "ctrl-d",
        "the changed tab: work not committed, pushed or pulled",
    ),
    ("ctrl-o", "the hidden tab"),
    ("ctrl-g", "switch profile"),
    ("ctrl-s", "order by use, last change or name"),
    ("ctrl-u", "clear the query"),
    ("ctrl-space", "quick keys: one letter runs an action"),
    ("f1", "help"),
    ("f5", "scan again"),
    ("esc", "clear the query, then the marks, then quit"),
];

/// Whether a `KeyCode::Char` event belongs in the query.
///
/// Alt is a shortcut prefix in terminal applications, so Alt+f must not type
/// an `f`.
///
/// AltGr arrives as Ctrl+Alt on Windows, so characters typed with it do not
/// reach the query either.
pub fn is_typed_text(key: &KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn shift_is_text_but_alt_and_ctrl_are_not() {
        let plain = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let shifted = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT);
        let alt = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT);
        let ctrl = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        assert!(is_typed_text(&plain));
        assert!(is_typed_text(&shifted));
        assert!(!is_typed_text(&alt));
        assert!(!is_typed_text(&ctrl));
    }
}
