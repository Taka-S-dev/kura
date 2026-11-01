# kura

kura finds the Git repositories on your PC, wherever they are, and lists them in one place: what language each is in, whether it has work not committed, pushed or pulled, and what was committed lately. Pick one to `cd` into it, or open it in Fork, an editor or lazygit.

It runs in the terminal and needs no window of its own, so it is built to work over SSH as well: there the action menu offers terminal programs such as lazygit.

## Install

```sh
cargo install --path .
```

Then add the shell function, so that choosing a repository changes the directory of the shell you are in:

| Shell | Add this line to | Line |
| --- | --- | --- |
| PowerShell | `$PROFILE` | `Invoke-Expression (& kura init powershell \| Out-String)` |
| bash | `~/.bashrc` | `eval "$(kura init bash)"` |
| zsh | `~/.zshrc` | `eval "$(kura init zsh)"` |

Put it after `zoxide init` if you use zoxide; kura orders the list by zoxide's history.

## Use

```sh
kr            # open the list; Enter cds into the selected repository
kr baton      # cd straight into baton when nothing else matches
kura          # the list without the shell function: open repositories from the menu
```

The first start looks for repositories in your home folder and on your drives and asks which folders to search from then on.

In the list, type to search. `rs:` keeps only Rust repositories, and two words search folder paths too, so `obsidian notes` tells two repositories called `notes` apart. F1 shows everything else; `kura keys` prints the keys.

## Settings

`kura config` opens the settings in your editor. `[scan]` sets the folders to search and what to hide, `[open]` the programs in the action menu, `[ui]` the look. Changes apply to an open kura as soon as you save.

### Profiles

A profile is a file in `profiles\` beside `config.toml` with its own folders to search and its own hidden list. Use one to keep some repositories off the screen, say while sharing it. Choose it with `--profile NAME`, `KURA_PROFILE`, `[ui] profile`, or the drop-down at the left of the tabs.

Profiles keep repositories off the screen. They are not access control: the settings and the cache list every profile's paths in plain text.

## For scripts and agents

Every setting can be changed without the screen and without rebuilding: kura reads its settings each time it starts. Results go to stdout, problems to stderr, and the exit code is 0 on success and 2 on an error. `--profile NAME` works with every command and acts on that profile's file.

| Command | Does |
| --- | --- |
| `kura config get KEY` | Prints one setting in effect, as TOML; KEY is `section.key`, such as `ui.name` |
| `kura config set KEY VALUE` | Changes one setting; VALUE is TOML, or plain text for a string. Refused if the result would not load |
| `kura config path` | Prints the settings file in use, to edit it directly |
| `kura config check` | Reads the settings as kura would; prints `ok:` and any `warning:` lines, or fails with the reason |
| `kura roots [--json]` | Lists the folders searched |
| `kura roots add [PATH]` / `remove PATH` | Adds a folder (the current one by default) or stops searching one |
| `kura hide TARGET...` / `show TARGET...` | Hides repositories, by path or pattern such as `'old_*'`, or brings them back |
| `kura hidden [--json]` | Lists what is hidden: paths and patterns |
| `kura profile list [--json]` | Lists the profiles, marking the one in use |
| `kura profile new NAME [--root PATH]...` | Makes a profile searching the given folders, or the current ones without `--root` |
| `kura profile default NAME` | Sets the profile kura starts in; `none` for no profile |
| `kura profile rename OLD NEW` / `remove NAME` | Renames or deletes a profile |
| `kura list [--json]` | Lists the repositories |
| `kura keys` | Prints the keys of the list screen |

`config set` writes `scan.*` to the profile in use, and `ui.*` and `open.*` to `config.toml`, which every profile shares. The settings `config get` and `config set` take are `scan.roots`, `scan.max_depth`, `scan.exclude`, `scan.hidden`, `ui.icons`, `ui.sort` (`used`, `changed` or `name`), `ui.name`, `ui.profile` and `open.<program>`.

Setting `KURA_CONFIG_DIR` to a folder makes kura keep its settings, profiles and caches there instead, which is the way to try a change on a copy first.

### Requests and the commands for them

"Make a work profile that searches only C:\work\company and hides the old_ projects, and open kura in it from now on":

```sh
kura profile new work --root 'C:\work\company'
kura --profile work hide 'old_*'
kura profile default work
kura --profile work config check
```

"Show 'notes' instead of kura in the terminal's tab":

```sh
kura config set ui.name notes      # '' leaves the title alone
```

"Add lazygit to the menu":

```sh
kura config set open.lazygit "{ cmd = 'lazygit', mode = 'exec' }"
```

"Stop searching OneDrive":

```sh
kura roots                          # find the exact path
kura roots remove 'C:\Users\me\OneDrive'
```

To change something no command covers, edit the file from `kura config path` and run `kura config check` afterwards. The file is TOML with a comment above each setting.
