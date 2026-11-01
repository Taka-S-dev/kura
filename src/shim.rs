//! Shell integration printed by `kura init`. A child process cannot change
//! the parent shell's directory, so the shell gets a function that runs
//! `kura pick`, reads the path it prints and cds there.

type Error = Box<dyn std::error::Error>;

pub fn powershell(name: &str) -> Result<String, Error> {
    check_name(name)?;
    let exe = std::env::current_exe()?;
    // Single quotes are the only thing that needs escaping inside '...'.
    let exe = exe.to_string_lossy().replace('\'', "''");
    Ok(format!(
        r#"# kura: {name} for PowerShell.
# Add this line to $PROFILE (after zoxide init, if you use zoxide):
#   Invoke-Expression (& kura init powershell | Out-String)

function global:{name} {{
    # The query travels in an environment variable, so that nothing in it
    # is reparsed on the way.
    $previousQuery = $env:KURA_QUERY
    $env:KURA_QUERY = ($args -join ' ')
    $encoding = [Console]::OutputEncoding
    $exitCode = 2
    try {{
        # kura prints UTF-8; make sure paths with non-ASCII characters
        # come back intact on consoles that default to a legacy code page.
        [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
        $path = & '{exe}' pick
        $exitCode = $LASTEXITCODE
        if ($exitCode -eq 0 -and $path) {{
            Set-Location -LiteralPath $path -ErrorAction Stop
            if (Get-Command zoxide -ErrorAction SilentlyContinue) {{ zoxide add -- $path 2>$null }}
        }}
    }} catch {{
        $exitCode = 2
        Write-Error $_
    }} finally {{
        [Console]::OutputEncoding = $encoding
        $env:KURA_QUERY = $previousQuery
        $global:LASTEXITCODE = $exitCode
    }}
}}
"#
    ))
}

/// The same function for bash and zsh. `dir`, not `path`: zsh ties a
/// variable called path to PATH.
pub fn posix(name: &str) -> Result<String, Error> {
    check_name(name)?;
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy().replace('\'', r"'\''");
    Ok(format!(
        r#"# kura: {name} for bash and zsh.
# Add this line to ~/.bashrc or ~/.zshrc (after zoxide init, if you use zoxide):
#   eval "$(kura init bash)"

{name}() {{
    local dir code
    dir="$(KURA_QUERY="$*" '{exe}' pick)"
    code=$?
    if [ "$code" -eq 0 ] && [ -n "$dir" ]; then
        builtin cd -- "$dir" || return 2
        if command -v zoxide >/dev/null 2>&1; then zoxide add -- "$dir" 2>/dev/null; fi
    fi
    return "$code"
}}
"#
    ))
}

fn check_name(name: &str) -> Result<(), Error> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(format!("not usable as a command name: {name}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_function_carries_the_chosen_name() {
        let text = powershell("kr").unwrap();
        assert!(text.contains("function global:kr {"));
        assert!(text.contains("Set-Location -LiteralPath $path"));
        assert!(powershell("k r").is_err());
        let posix = posix("kr").unwrap();
        assert!(posix.contains("kr() {"));
        assert!(posix.contains("builtin cd -- \"$dir\""));
        assert!(powershell("").is_err());
    }
}
