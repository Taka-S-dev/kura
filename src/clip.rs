//! Copying to the clipboard, including over SSH. A server has no clipboard
//! of its own worth writing to, so there the text goes to the terminal on
//! the other end as an OSC 52 sequence, which Windows Terminal, WezTerm and
//! tmux (with `set -g set-clipboard on`) put on the local clipboard.

use std::io::Write;

/// Copies `text`. The answer says where it went, for the notice.
pub fn copy(text: &str) -> Result<&'static str, String> {
    if !over_ssh() {
        let local = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string()));
        if local.is_ok() {
            return Ok("Copied");
        }
    }
    osc52(text).map_err(|e| e.to_string())?;
    // The terminal gives no answer, so this cannot claim more than sending.
    Ok("Sent to the terminal's clipboard")
}

pub fn over_ssh() -> bool {
    ["SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
}

fn osc52(text: &str) -> std::io::Result<()> {
    let mut err = std::io::stderr();
    write!(err, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    err.flush()
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("/home/user/蔵".as_bytes()), "L2hvbWUvdXNlci/olLU=");
    }
}
