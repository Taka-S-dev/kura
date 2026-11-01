//! Drawing. Everything here reads the app and paints a frame; the only
//! state it writes back is where the list is, for scrolling and clicks.

use std::path::Path;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Folders, Screen, Setup, View};
use crate::gitstat::Status;
use crate::repo::Repo;

/// fzf's dark palette in 256 colours, readable on any dark background.
mod theme {
    use ratatui::style::{Color, Modifier, Style};

    pub const BORDER: Style = Style::new().fg(Color::Indexed(240));
    pub const PROMPT: Style = Style::new().fg(Color::Indexed(110));
    pub const INFO: Style = Style::new().fg(Color::Indexed(144));
    pub const POINTER: Style = Style::new().fg(Color::Indexed(161));
    pub const MATCH: Style = Style::new().fg(Color::Indexed(108));
    /// Context rather than the subject: parent folders, main branches, old dates.
    pub const QUIET: Style = Style::new().fg(Color::Indexed(244));
    pub const NOTICE: Style = Style::new()
        .fg(Color::Indexed(109))
        .add_modifier(Modifier::BOLD);
    pub const NOTICE_BAD: Style = Style::new()
        .fg(Color::Indexed(161))
        .add_modifier(Modifier::BOLD);
    /// The tab in view: the prompt's blue as a background, unlike the grey
    /// of the selected row, so the two are never confused.
    pub const TAB: Style = Style::new()
        .fg(Color::Indexed(235))
        .bg(Color::Indexed(110))
        .add_modifier(Modifier::BOLD);
    /// The check on a marked row, and the tints of marked rows.
    pub const MARK: Style = Style::new()
        .fg(Color::Indexed(117))
        .add_modifier(Modifier::BOLD);
    pub const MARKED_BG: Color = Color::Indexed(24);
    pub const MARKED_CURRENT_BG: Color = Color::Indexed(31);
    /// Uncommitted changes, commits to push, commits to pull.
    pub const DIRTY: Style = Style::new().fg(Color::Indexed(179));
    pub const AHEAD: Style = Style::new().fg(Color::Indexed(114));
    pub const BEHIND: Style = Style::new().fg(Color::Indexed(80));
    pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
    /// The profile's name in the header: set apart from everything else,
    /// so which repositories are on show is never in doubt.
    pub const PROFILE: Style = Style::new()
        .fg(Color::Indexed(235))
        .bg(Color::Indexed(179))
        .add_modifier(Modifier::BOLD);
    /// Switches not in use, the tabs and the profile: plain, a shade
    /// brighter than labels. A grey fill at rest matched the cursor row's
    /// grey and the two blurred, so the fill only comes under the mouse.
    pub const SWITCH: Style = Style::new().fg(Color::Indexed(250));
    pub const SWITCH_HOVER: Style = Style::new().fg(Color::Indexed(255)).bg(Color::Indexed(239));
    pub const CURRENT: Style = Style::new()
        .fg(Color::Indexed(255))
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
    pub fn lang(tag: &str) -> Style {
        let color = match tag {
            "rs" => 173,
            "go" => 80,
            "ts" => 75,
            "js" => 186,
            "py" => 221,
            "cs" => 140,
            "c" => 110,
            "cp" => 68,
            "ja" => 172,
            "kt" => 141,
            "rb" => 167,
            "ph" => 104,
            "sw" => 209,
            "lu" => 69,
            "ps" => 74,
            "sh" => 114,
            "ht" => 166,
            "sv" => 202,
            "vu" => 72,
            _ => 244,
        };
        Style::new().fg(Color::Indexed(color))
    }
}

const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.height < 6 || area.width < 20 {
        frame.render_widget(Paragraph::new("kura: window too small"), area);
        return;
    }
    if let Screen::Setup(setup) = &app.screen {
        draw_setup(frame, area, setup, app);
        return;
    }
    if let Screen::Folders(folders) = &app.screen {
        draw_folders(frame, area, folders);
        return;
    }
    if app.help_page {
        draw_help_page(frame, area, app);
        return;
    }
    let rows = |y: u16, h: u16| Rect::new(area.x, area.y + y, area.width, h);
    let list_height = area.height - 5;
    // From the widest choice to the narrowest: which repositories (profile
    // and tabs), then the query, right above the list it narrows.
    draw_tabs(frame, rows(0, 1), app);
    draw_header(frame, rows(1, 1), app);
    let (list, preview) = split(rows(2, list_height));
    app.list_area = list;
    draw_list(frame, list, app);
    if let Some(preview) = preview {
        draw_preview(frame, preview, app);
    }
    rule(frame, rows(2 + list_height, 1));
    draw_status(frame, rows(3 + list_height, 1), app);
    let hints = match app.view {
        View::Hidden => {
            "enter cd  tab mark  ctrl-p menu (show in the list)  f1 c: patterns  ctrl-o back  esc quit"
        }
        View::Changed => "enter cd  tab mark  ctrl-p menu  ctrl-d back  f1 help  esc quit",
        View::All => {
            "enter cd  tab mark  ctrl-p menu  ctrl-d changed  ctrl-o hidden  f1 help  esc quit"
        }
    };
    frame.render_widget(
        Paragraph::new(Line::styled(format!(" {hints}"), theme::BORDER)),
        rows(4 + list_height, 1),
    );
    if app.profiles.is_some() {
        draw_profiles(frame, area, app);
    } else if app.menu.is_some() {
        draw_menu(frame, area, app);
    } else if app.help {
        draw_help(frame, area, app);
    }
}

fn rule(frame: &mut Frame, area: Rect) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(Paragraph::new(Line::styled(line, theme::BORDER)), area);
}

/// A switch at rest, or lit while the mouse is over it.
fn switch_style(app: &App, area: Rect) -> Style {
    let over = app
        .hover
        .is_some_and(|(x, y)| y == area.y && x >= area.x && x < area.x + area.width);
    if over {
        theme::SWITCH_HOVER
    } else {
        theme::SWITCH
    }
}

/// The list and, from 120 columns, the preview beside it. Below that the
/// list keeps the whole width, which its columns need more.
fn split(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < 120 {
        return (area, None);
    }
    let preview = (area.width * 2 / 5).clamp(44, 72);
    let list = Rect::new(area.x, area.y, area.width - preview, area.height);
    (
        list,
        Some(Rect::new(list.right(), area.y, preview, area.height)),
    )
}

/// The tabs sit in the rule under the query, each with its count, so the
/// repositories with work waiting and the hidden ones are never out of mind.
fn draw_tabs(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    // First the profile, as a drop-down: it decides which repositories
    // there are, and the tabs then narrow them, so the row reads from the
    // widest choice to the narrowest. A click or Ctrl-G opens it; with no
    // profiles yet it offers to make the first.
    let profile = crate::config::profile();
    let text = match &profile {
        Some(name) => format!(" {name} ▾ "),
        None => " default ▾ ".to_string(),
    };
    app.profile_area = Rect::new(x, area.y, text.width() as u16, 1);
    let style = match profile {
        Some(_) => theme::PROFILE,
        None => switch_style(app, app.profile_area),
    };
    x += text.width() as u16 + 1;
    spans.push(Span::styled(text, style));
    spans.push(Span::styled("│", theme::BORDER));
    for (i, view) in View::TABS.into_iter().enumerate() {
        let text = format!(" {} {} ", view.label(), app.count(view));
        let width = text.width() as u16;
        app.tab_areas[i] = Rect::new(x, area.y, width.min(area.right().saturating_sub(x)), 1);
        let style = if view == app.view {
            theme::TAB
        } else {
            switch_style(app, app.tab_areas[i])
        };
        spans.push(Span::styled(text, style));
        // A thin line between the tabs marks them as one set of switches,
        // with less weight than brackets or fills would add.
        if i + 1 < View::TABS.len() {
            spans.push(Span::styled("│", theme::BORDER));
        } else {
            spans.push(Span::raw(" "));
        }
        x += width + 1;
    }
    spans.push(Span::raw(" "));
    // The order in use sits at the far end, with the key that changes it.
    let order = format!(" {} · ctrl-s ", app.sort.label());
    let used: usize = spans.iter().map(|s| s.content.width()).sum::<usize>() + order.width();
    spans.push(Span::styled(
        "─".repeat((area.width as usize).saturating_sub(used)),
        theme::BORDER,
    ));
    if used < area.width as usize {
        spans.push(Span::styled(order, theme::QUIET));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    // No name of the program here: whoever opened it knows, and its name
    // goes in the terminal's title, as [ui] name sets it. The profile sits
    // with the tabs below, so this line is the query alone.
    let mut left = vec![Span::raw(" ")];
    left.push(Span::styled(" > ", theme::PROMPT));
    left.push(Span::raw(app.query.clone()));
    let cursor_x = area.x + left.iter().map(|s| s.content.width() as u16).sum::<u16>();
    frame.render_widget(Paragraph::new(Line::from(left)), area);
    if app.menu.is_none() && !app.help {
        frame.set_cursor_position((cursor_x.min(area.right() - 1), area.y));
    }

    let mut right = Vec::new();
    if !app.marked.is_empty() {
        right.push(Span::styled(
            format!("✓ {} marked  ", app.marked.len()),
            theme::MARK,
        ));
    }
    right.push(Span::styled(
        format!("{}/{}", app.hits.len(), app.total()),
        theme::INFO,
    ));
    let spin = SPINNER[app.tick % SPINNER.len()];
    if let Some(found) = app.scanning() {
        right.push(Span::styled(
            format!("  {spin} scanning {found}"),
            theme::INFO,
        ));
    } else if app.checking() > 0 {
        right.push(Span::styled(
            format!("  {spin} checking {}", app.checking()),
            theme::INFO,
        ));
    }
    right.push(Span::raw(" "));
    frame.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        area,
    );
}

/// Which columns fit, dropped in this order as the window narrows:
/// parent folder, age, branch, language, state. Name always stays.
struct Columns {
    lang: bool,
    icons: bool,
    name: usize,
    status: bool,
    branch: Option<usize>,
    parent: Option<usize>,
    age: bool,
}

const POINTER: usize = 3;
const LANG: usize = 3;
/// Room for "●12 ↑3 ↓4"; fixed, so answers arriving do not move columns.
const STATUS: usize = 10;
const AGE: usize = 5;
const GAP: usize = 2;
/// Widths past which a name or branch is cut with an ellipsis. Columns are
/// sized for every repository, so one long name would otherwise push the
/// rest of the row far to the right for all of them.
const NAME_MAX: usize = 24;
const BRANCH_MAX: usize = 12;

fn columns(width: usize, name: usize, branch: Option<usize>) -> Columns {
    let mut c = Columns {
        lang: true,
        icons: false,
        name,
        status: true,
        branch,
        parent: None,
        age: true,
    };
    let used = |c: &Columns| {
        POINTER
            + if c.lang { LANG } else { 0 }
            + c.name
            + if c.status { GAP + STATUS } else { 0 }
            + c.branch.map_or(0, |b| b + GAP)
            + if c.age { AGE } else { 0 }
    };
    let room = width.saturating_sub(used(&c) + GAP);
    if room >= 12 {
        c.parent = Some(room);
        return c;
    }
    // Without the folder the rest may fit; the age goes only if not.
    if used(&c) <= width {
        return c;
    }
    c.age = false;
    if used(&c) > width {
        c.branch = None;
    }
    if used(&c) > width {
        c.lang = false;
    }
    if used(&c) > width {
        c.status = false;
    }
    let others = used(&c) - c.name;
    c.name = c.name.min(width.saturating_sub(others));
    c
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let height = area.height as usize;
    if app.hits.is_empty() {
        let text = if app.view == View::Hidden {
            "  No hidden repositories. Hide from the menu, or add patterns: F1, then c."
        } else if app.view == View::Changed && app.checking() > 0 {
            "  Asking git about each repository…"
        } else if app.view == View::Changed && app.query.is_empty() {
            "  Nothing to commit, push or pull."
        } else if app.total() > 0 {
            "  No match."
        } else if app.scanning().is_some() {
            "  Looking for repositories…"
        } else {
            "  No repositories yet. Add folders to roots with: kura config"
        };
        frame.render_widget(Paragraph::new(Line::styled(text, theme::QUIET)), area);
        return;
    }
    let selected = app.selected_index().unwrap_or(0);
    if selected < app.offset {
        app.offset = selected;
    } else if selected >= app.offset + height {
        app.offset = selected + 1 - height;
    }
    app.offset = app.offset.min(app.hits.len().saturating_sub(height));

    let shown = &app.hits[app.offset..(app.offset + height).min(app.hits.len())];
    // Widths come from every repository, not the rows in view, so the
    // columns stay put while scrolling or typing.
    let name_w = app
        .repos
        .iter()
        .map(name_width)
        .max()
        .unwrap_or(4)
        .clamp(4, NAME_MAX);
    // main and master are left blank, so the column only speaks up for
    // work on another branch; with none, it takes no room at all.
    let branch_w = app
        .repos
        .iter()
        .filter_map(|r| r.branch.as_deref().filter(|b| !is_default_branch(b)))
        .map(UnicodeWidthStr::width)
        .max()
        .map(|w| w.min(BRANCH_MAX));
    let mut cols = columns(area.width as usize, name_w, branch_w);
    cols.icons = app.icons;
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let roots = app.roots().to_vec();
    let now = SystemTime::now();

    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let repo = &app.repos[hit.index];
            let current = app.offset + i == selected;
            // Two repositories with one name: the folder is what tells them apart.
            let twin = app
                .hits
                .iter()
                .filter(|h| app.repos[h.index].name == repo.name)
                .nth(1)
                .is_some();
            let folder = folder(repo, &roots, home.as_deref(), twin);
            let state = state(app.checked(repo), app.status(repo));
            row(
                repo,
                &hit.name_hits,
                current,
                app.marked.contains(&repo.key()),
                twin,
                &cols,
                &folder,
                state,
                now,
                area.width,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn name_width(repo: &Repo) -> usize {
    repo.name.width() + if repo.worktree { 3 } else { 0 }
}

#[allow(clippy::too_many_arguments)]
fn row(
    repo: &Repo,
    hits: &[u32],
    current: bool,
    marked: bool,
    twin: bool,
    cols: &Columns,
    folder: &str,
    state: Vec<Span<'static>>,
    now: SystemTime,
    width: u16,
) -> Line<'static> {
    // The bar is the cursor, the dot a mark from Tab, as in fzf.
    let mut spans = vec![
        Span::styled(if current { "▌" } else { " " }, theme::POINTER),
        Span::styled(if marked { "✓ " } else { "  " }, theme::MARK),
    ];
    if cols.lang {
        let tag = repo.lang.unwrap_or("");
        let shown = if cols.icons {
            crate::icons::glyph(repo.lang)
        } else {
            tag
        };
        spans.push(Span::styled(pad(shown, LANG), theme::lang(tag)));
    }
    let name = fit(&repo.name, cols.name);
    spans.extend(highlight(&name, hits));
    let mut used = name.width();
    if repo.worktree && used + 3 <= cols.name {
        spans.push(Span::styled(" wt", theme::INFO));
        used += 3;
    }
    spans.push(Span::raw(" ".repeat(cols.name - used)));
    if cols.status {
        let used: usize = state.iter().map(|s| s.content.width()).sum();
        spans.push(Span::raw("  "));
        spans.extend(state);
        spans.push(Span::raw(" ".repeat(STATUS.saturating_sub(used))));
    }
    if let Some(w) = cols.branch {
        let branch = repo.branch.as_deref().filter(|b| !is_default_branch(b));
        spans.push(Span::raw("  "));
        spans.push(Span::raw(pad(&fit(branch.unwrap_or(""), w), w)));
    }
    // The age sits with the other short columns, next to the name it
    // belongs to; the folder, the longest and least read, comes last.
    if cols.age {
        let (text, fresh) = age(repo.modified, now);
        let style = if fresh { Style::new() } else { theme::QUIET };
        spans.push(Span::styled(format!("{text:>AGE$}"), style));
    }
    if let Some(w) = cols.parent {
        let style = if twin { Style::new() } else { theme::QUIET };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(pad(&fit_left(folder, w), w), style));
    }
    let mut line = Line::from(spans);
    // A marked row is tinted blue across its width, like the tab in view,
    // so marks show at a glance; the cursor row is grey, or a lighter blue
    // when it is marked too.
    let background = match (current, marked) {
        (true, true) => Some(theme::MARKED_CURRENT_BG),
        (false, true) => Some(theme::MARKED_BG),
        (true, false) => theme::CURRENT.bg,
        (false, false) => None,
    };
    if let Some(bg) = background {
        let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
        line.spans
            .push(Span::raw(" ".repeat((width as usize).saturating_sub(used))));
        line = line.patch_style(Style::new().bg(bg));
    }
    line
}

fn is_default_branch(branch: &str) -> bool {
    matches!(branch, "main" | "master")
}

/// The state column: uncommitted changes, commits to push and to pull.
/// Empty while git is being asked and when there is nothing to do, so the
/// rows that need a look are the only ones that speak.
fn state(checked: bool, status: Option<&Status>) -> Vec<Span<'static>> {
    let Some(status) = status else {
        return if checked {
            vec![Span::styled("?", theme::QUIET)]
        } else {
            Vec::new()
        };
    };
    let mut spans = Vec::new();
    let mut part = |text: String, style: Style| {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(text, style));
    };
    if status.dirty > 0 {
        part(format!("●{}", status.dirty), theme::DIRTY);
    }
    if status.ahead > 0 {
        part(format!("↑{}", status.ahead), theme::AHEAD);
    }
    if status.behind > 0 {
        part(format!("↓{}", status.behind), theme::BEHIND);
    }
    spans
}

/// The folder a repository sits in, from the root it was found under:
/// `~\src\C\linux` under `~\src` reads `C`, and one directly
/// under a root reads nothing. The full path is on the line below the
/// list. Twins, two repositories of one name, get the full folder, since
/// both may sit directly under different roots.
fn folder(repo: &Repo, roots: &[std::path::PathBuf], home: Option<&Path>, twin: bool) -> String {
    let Some(parent) = repo.path.parent() else {
        return String::new();
    };
    if !twin {
        let k = crate::repo::key(parent);
        let root = roots
            .iter()
            .filter(|r| crate::repo::key_within(&k, &crate::repo::key(r)))
            .max_by_key(|r| r.components().count());
        if let Some(root) = root {
            let rest: std::path::PathBuf = parent
                .components()
                .skip(root.components().count())
                .collect();
            return rest.to_string_lossy().into_owned();
        }
    }
    shorten(parent, home)
}

/// The panel beside the list, for the selected repository, top to bottom:
/// what it is (its README), what it is written in, where it stands with
/// git, how lively it has been, and what was committed lately, which takes
/// whatever height is left. The path is on the line below the list.
fn draw_preview(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(theme::BORDER);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(repo) = app.current() else {
        return;
    };
    let width = (inner.width as usize).saturating_sub(2);
    let height = inner.height as usize;
    let key = repo.key();
    let untrusted = app.untrusted.contains(&key);
    let field = |name: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!(" {name:<9}"), theme::QUIET)];
        spans.extend(value);
        Line::from(spans)
    };
    let mut lines = vec![Line::styled(
        format!(" {}", fit(&repo.name, width)),
        theme::BOLD,
    )];

    // What it is: the README's first paragraph, the thing most easily
    // forgotten about a repository not opened in months.
    match app.abouts.get(&key) {
        Some(Some(about)) => {
            let wrapped = wrap(about, width);
            let more = wrapped.len() > ABOUT_LINES;
            for (i, line) in wrapped.into_iter().take(ABOUT_LINES).enumerate() {
                let line = if more && i == ABOUT_LINES - 1 {
                    fit(&format!("{line}…"), width)
                } else {
                    line
                };
                lines.push(Line::raw(format!(" {line}")));
            }
        }
        Some(None) => {}
        None => lines.push(Line::styled(" …", theme::QUIET)),
    }

    // What it is written in, as GitHub's bar shows it.
    if let Some(mix) = app.mixes.get(&key).filter(|m| !m.is_empty()) {
        lines.push(Line::raw(""));
        lines.push(language_bar(mix, width));
        lines.push(language_legend(mix));
    }

    // Where it stands with git.
    lines.push(Line::raw(""));
    let status = app.status(repo);
    let mut branch = vec![Span::raw(repo.branch.clone().unwrap_or_default())];
    if let Some(upstream) = status.and_then(|s| s.upstream.as_ref()) {
        branch.push(Span::styled(format!(" → {upstream}"), theme::QUIET));
    } else if status.is_some() {
        branch.push(Span::styled("  no upstream", theme::QUIET));
    }
    lines.push(field("branch", branch));
    let words = match (app.checked(repo), status) {
        (false, _) => vec![Span::styled("checking…", theme::QUIET)],
        (true, None) if untrusted => vec![Span::styled(
            "not asked: its git config runs programs",
            theme::NOTICE_BAD,
        )],
        (true, None) => vec![Span::styled("git did not answer in time", theme::QUIET)],
        (true, Some(s)) if !s.needs_attention() => {
            vec![Span::styled(
                "nothing to commit, push or pull",
                theme::QUIET,
            )]
        }
        (true, Some(s)) => {
            let mut spans = Vec::new();
            let mut part = |text: String, style: Style| {
                if !spans.is_empty() {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(text, style));
            };
            if s.dirty > 0 {
                part(format!("●{} changed", s.dirty), theme::DIRTY);
            }
            if s.ahead > 0 {
                part(format!("↑{} to push", s.ahead), theme::AHEAD);
            }
            if s.behind > 0 {
                part(format!("↓{} to pull", s.behind), theme::BEHIND);
            }
            spans
        }
    };
    lines.push(field("state", words));
    let log = app.logs.get(&key).and_then(|l| l.as_ref());
    if let Some(remote) = log.and_then(|l| l.remote.as_deref()) {
        lines.push(field(
            "remote",
            vec![Span::raw(fit(
                &short_remote(remote),
                width.saturating_sub(9),
            ))],
        ));
    }
    if let Some(log) = log
        && log.weeks.iter().any(|n| *n > 0)
    {
        let total: u32 = log.weeks.iter().sum();
        let mut spans = sparkline(&log.weeks);
        spans.push(Span::styled(
            format!("  {total} in {} weeks", log.weeks.len()),
            theme::QUIET,
        ));
        lines.push(field("activity", spans));
    }

    // Work in the tree, then the other branches.
    if let Some(s) = status.filter(|s| !s.files.is_empty()) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(" changes", theme::BOLD));
        let shown = s.files.len().min(CHANGES_SHOWN);
        for file in &s.files[..shown] {
            let (xy, path) = file.split_at(2.min(file.len()));
            let style = if xy == "??" {
                theme::QUIET
            } else {
                theme::DIRTY
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {xy}"), style),
                Span::raw(fit_left(path, width.saturating_sub(4))),
            ]));
        }
        if s.dirty > shown {
            lines.push(Line::styled(
                format!("  … {} more", s.dirty - shown),
                theme::QUIET,
            ));
        }
    }
    let current = repo.branch.as_deref();
    let others: Vec<&crate::gitstat::Branch> = log
        .map(|l| {
            l.branches
                .iter()
                .filter(|b| Some(b.name.as_str()) != current)
                .collect()
        })
        .unwrap_or_default();
    if !others.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(" branches", theme::BOLD));
        let now = SystemTime::now();
        for branch in others.iter().take(BRANCHES_SHOWN) {
            let mut spans = vec![Span::raw(format!(
                "  {}",
                fit(&branch.name, width.saturating_sub(16))
            ))];
            if branch.ahead > 0 {
                spans.push(Span::styled(format!(" ↑{}", branch.ahead), theme::AHEAD));
            }
            if branch.behind > 0 {
                spans.push(Span::styled(format!(" ↓{}", branch.behind), theme::BEHIND));
            }
            if branch.gone {
                spans.push(Span::styled(" upstream gone", theme::QUIET));
            }
            let (when, _) = age(
                now.checked_sub(std::time::Duration::from_secs(branch.age)),
                now,
            );
            spans.push(Span::styled(format!("  {when}"), theme::QUIET));
            lines.push(Line::from(spans));
        }
        if others.len() > BRANCHES_SHOWN {
            lines.push(Line::styled(
                format!("  … {} more", others.len() - BRANCHES_SHOWN),
                theme::QUIET,
            ));
        }
    }

    // What was committed lately, in whatever room is left.
    lines.push(Line::raw(""));
    lines.push(Line::styled(" recent", theme::BOLD));
    match app.logs.get(&key) {
        None => lines.push(Line::styled("  …", theme::QUIET)),
        Some(None) if untrusted => lines.push(Line::styled("  not asked", theme::QUIET)),
        Some(None) => lines.push(Line::styled("  git did not answer", theme::QUIET)),
        Some(Some(l)) if l.commits.is_empty() => {
            lines.push(Line::styled("  no commits yet", theme::QUIET));
        }
        Some(Some(l)) => {
            let room = height.saturating_sub(lines.len()).max(1);
            for (hash, when, subject) in l.commits.iter().take(room) {
                let lead = format!("  {hash} {when:>3} ");
                lines.push(Line::from(vec![
                    Span::styled(format!("  {hash}"), theme::INFO),
                    Span::styled(format!(" {when:>3} "), theme::QUIET),
                    Span::raw(fit(subject, width.saturating_sub(lead.width()))),
                ]));
            }
        }
    }
    let text = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(1),
        inner.height,
    );
    frame.render_widget(Paragraph::new(lines), text);
}

/// Lines of README shown before the rest of the panel.
const ABOUT_LINES: usize = 3;
const CHANGES_SHOWN: usize = 6;
const BRANCHES_SHOWN: usize = 3;

/// `text` broken into lines of at most `width` columns, at spaces where
/// there are any; text without spaces, such as Japanese, breaks anywhere.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            match line.rfind(' ').filter(|_| c != ' ') {
                Some(at) if at > 0 => {
                    let rest = line[at + 1..].to_string();
                    line.truncate(at);
                    lines.push(std::mem::take(&mut line));
                    used = rest.width();
                    line = rest;
                }
                _ => {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
            }
            if c == ' ' {
                continue;
            }
        }
        line.push(c);
        used += w;
    }
    if !line.trim().is_empty() {
        lines.push(line);
    }
    lines
}

/// The languages as one bar, each its colour and share of the width.
fn language_bar(mix: &[(&'static str, u8)], width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 0;
    for (i, (tag, share)) in mix.iter().enumerate() {
        let last = i + 1 == mix.len();
        let cells = if last {
            width.saturating_sub(used)
        } else {
            (width * *share as usize)
                .div_ceil(100)
                .min(width.saturating_sub(used))
        };
        if cells > 0 {
            spans.push(Span::styled("━".repeat(cells), theme::lang(tag)));
            used += cells;
        }
    }
    Line::from(spans)
}

/// The bar's key: the largest languages with their share.
fn language_legend(mix: &[(&'static str, u8)]) -> Line<'static> {
    const SHOWN: usize = 4;
    let mut spans = vec![Span::raw(" ")];
    for (tag, share) in mix.iter().take(SHOWN) {
        spans.push(Span::styled(tag.to_string(), theme::lang(tag)));
        spans.push(Span::styled(format!(" {share}%  "), theme::QUIET));
    }
    if mix.len() > SHOWN {
        spans.push(Span::styled(
            format!("+{} more", mix.len() - SHOWN),
            theme::QUIET,
        ));
    }
    Line::from(spans)
}

/// Commits per week as small bars; a dot for a week without any.
fn sparkline(weeks: &[u32]) -> Vec<Span<'static>> {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let top = weeks.iter().copied().max().unwrap_or(0).max(1);
    weeks
        .iter()
        .map(|&n| {
            if n == 0 {
                Span::styled("·", theme::QUIET)
            } else {
                let level = (n as usize * (BARS.len() - 1)).div_ceil(top as usize);
                Span::styled(BARS[level].to_string(), theme::PROMPT)
            }
        })
        .collect()
}

/// `github.com/owner/name` from the forms remotes come in.
fn short_remote(url: &str) -> String {
    let url = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("ssh://")
        .trim_start_matches("git@")
        .trim_end_matches(".git");
    url.replacen(':', "/", 1)
}

fn highlight(text: &str, hits: &[u32]) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            if hits.contains(&(i as u32)) {
                Span::styled(c.to_string(), theme::MATCH.add_modifier(Modifier::BOLD))
            } else {
                Span::raw(c.to_string())
            }
        })
        .collect()
}

/// `~` for the home folder, so the useful end of the path fits.
fn shorten(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        let rest = rest.to_string_lossy();
        let sep = std::path::MAIN_SEPARATOR;
        return if rest.is_empty() {
            "~".into()
        } else {
            format!("~{sep}{rest}")
        };
    }
    path.to_string_lossy().into_owned()
}

fn age(modified: Option<SystemTime>, now: SystemTime) -> (String, bool) {
    let Some(secs) = modified
        .and_then(|m| now.duration_since(m).ok())
        .map(|d| d.as_secs())
    else {
        return (String::new(), false);
    };
    let text = match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        86_400..1_209_600 => format!("{}d", secs / 86_400),
        1_209_600..5_184_000 => format!("{}w", secs / 604_800),
        5_184_000..31_536_000 => format!("{}mo", secs / 2_592_000),
        _ => format!("{}y", secs / 31_536_000),
    };
    (text, secs < 86_400)
}

/// Cuts `text` to `width` columns, marking the cut with an ellipsis.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Like `fit`, but keeps the end, which is the telling part of a path.
fn fit_left(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        kept.push(c);
        used += w;
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

fn pad(text: &str, width: usize) -> String {
    let w = text.width();
    format!("{text}{}", " ".repeat(width.saturating_sub(w)))
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(notice) = &app.notice {
        let style = if notice.bad {
            theme::NOTICE_BAD
        } else {
            theme::NOTICE
        };
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {}", notice.text), style)),
            area,
        );
        return;
    }
    if let Some(repo) = app.current() {
        let path = fit_left(&repo.path.to_string_lossy(), area.width as usize - 1);
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {path}"), theme::QUIET)),
            area,
        );
    }
}

/// A box in the lower right, over the list.
fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.right().saturating_sub(width + 2),
        area.bottom().saturating_sub(height + 2),
        width,
        height,
    )
}

fn draw_menu(frame: &mut Frame, area: Rect, app: &App) {
    let Some(index) = app.menu else {
        return;
    };
    let title = match app.marked.len() {
        0 | 1 => app.current().map(|r| r.name.clone()).unwrap_or_default(),
        n => format!("{n} repositories"),
    };
    let label_w = app
        .items
        .iter()
        .map(|i| app.label(i).width())
        .max()
        .unwrap_or(0);
    let inner_w = (4 + label_w + 12).max(34);
    let mut lines: Vec<Line> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let tag = if item.terminal { "[terminal]" } else { "" };
            let label = format!("  {}", pad(app.label(item), label_w));
            let rest = inner_w.saturating_sub(2 + label.width() + 1);
            let line = Line::from(vec![
                Span::styled(
                    format!(" {}", item.key),
                    theme::PROMPT.add_modifier(Modifier::BOLD),
                ),
                Span::raw(label),
                Span::styled(format!("{tag:>rest$} "), theme::QUIET),
            ]);
            if i == index {
                line.patch_style(theme::CURRENT)
            } else {
                line
            }
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!(" after: {:<7} ctrl-x switches", app.after_label()),
        theme::QUIET,
    ));
    boxed(frame, area, &title, lines, inner_w as u16);
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let mut rows: Vec<(char, String, String)> = vec![
        ('p', "action menu".into(), "ctrl-p".into()),
        (
            'd',
            if app.view == View::Changed {
                "back to the list"
            } else {
                "repositories with changes"
            }
            .into(),
            "ctrl-d".into(),
        ),
        (
            'o',
            if app.view == View::Hidden {
                "back to the list"
            } else {
                "hidden repositories"
            }
            .into(),
            "ctrl-o".into(),
        ),
        (
            's',
            format!("order: {}", app.sort.next().label()),
            "ctrl-s".into(),
        ),
        ('r', "scan again".into(), "f5".into()),
        ('w', "folders to search".into(), String::new()),
        ('g', "switch profile".into(), "ctrl-g".into()),
        ('c', "edit settings (hidden patterns)".into(), String::new()),
    ];
    rows.extend(
        app.items
            .iter()
            .map(|i| (i.key, app.label(i).to_string(), format!("alt-{}", i.key))),
    );
    let width = rows.iter().map(|r| r.1.width()).max().unwrap_or(0);
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(key, label, short)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key}"),
                    theme::PROMPT.add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {}  ", pad(&label, width))),
                Span::styled(short, theme::QUIET),
            ])
        })
        .chain([
            Line::raw(""),
            Line::styled(
                " a letter runs it  ·  esc closes  ·  f1 full help",
                theme::QUIET,
            ),
        ])
        .collect();
    let inner = lines.iter().map(Line::width).max().unwrap_or(0) + 1;
    boxed(frame, area, "keys", lines, inner.max(30) as u16);
}

/// One line of the help page: what to type, and what it does.
type HelpRow = (Vec<Span<'static>>, String);

/// The help page (F1): everything kura understands, by topic, on a
/// screen of its own. The key list (Ctrl-Space) keeps only what runs.
fn draw_help_page(frame: &mut Frame, area: Rect, app: &mut App) {
    let langs = crate::rank::LANGS
        .iter()
        .map(|l| format!("{l}:"))
        .collect::<Vec<_>>()
        .join(" ");
    let sections: Vec<(&str, Vec<HelpRow>)> = vec![
        (
            "Search",
            vec![
                (key_spans("baton"), "names that match, best first".into()),
                (
                    key_spans("obsidian notes"),
                    "two words: folders are searched too".into(),
                ),
                (key_spans("rs: tad"), "only Rust, then match tad".into()),
                (Vec::new(), format!("languages: {langs}")),
            ],
        ),
        (
            "Tabs",
            vec![
                (key_spans("repos"), "every repository".into()),
                (
                    key_spans("changed  ctrl-d"),
                    "work not committed, pushed or pulled".into(),
                ),
                (
                    key_spans("hidden   ctrl-o"),
                    "hidden from the menu or by a pattern".into(),
                ),
            ],
        ),
        (
            "State",
            vec![
                (
                    vec![Span::styled("●3", theme::DIRTY)],
                    "3 files changed and not committed".into(),
                ),
                (
                    vec![Span::styled("↑2", theme::AHEAD)],
                    "2 commits to push".into(),
                ),
                (
                    vec![Span::styled("↓1", theme::BEHIND)],
                    "1 commit to pull (as of the last fetch)".into(),
                ),
                (
                    vec![Span::styled("?", theme::QUIET)],
                    "git did not answer in time, or was not asked: the preview says why".into(),
                ),
                (Vec::new(), "blank: nothing to do".into()),
            ],
        ),
        (
            "Keys",
            crate::keys::LIST
                .iter()
                .map(|(key, what)| (key_spans(key), what.to_string()))
                .collect(),
        ),
        (
            "Settings",
            vec![
                (
                    key_spans("w"),
                    "folders to search: check, add, remove".into(),
                ),
                (
                    key_spans("ctrl-g"),
                    "switch profile: its own folders and hidden list".into(),
                ),
                (
                    Vec::new(),
                    "profiles are files in profiles\\ beside config.toml".into(),
                ),
                (
                    key_spans("c"),
                    "open them now: folders, hidden patterns, programs".into(),
                ),
                (
                    Vec::new(),
                    "saving them applies the change while kura is open".into(),
                ),
            ],
        ),
    ];
    let label_w = sections
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(spans, _)| spans.iter().map(|s| s.content.width()).sum::<usize>())
        .max()
        .unwrap_or(0)
        + 3;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Help", theme::BOLD),
            Span::styled(
                "   ↑↓ scroll  ·  w folders  ·  c settings  ·  any other key closes",
                theme::QUIET,
            ),
        ]),
        Line::raw(""),
    ];
    for (title, rows) in sections {
        lines.push(Line::styled(format!(" {title}"), theme::BOLD));
        for (spans, text) in rows {
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            let mut line = vec![Span::raw("   ")];
            line.extend(spans);
            line.push(Span::raw(" ".repeat(label_w - used)));
            line.push(Span::raw(text));
            lines.push(Line::from(line));
        }
        lines.push(Line::raw(""));
    }
    let most = (lines.len() as u16).saturating_sub(area.height);
    app.help_scroll = app.help_scroll.min(most);
    frame.render_widget(Paragraph::new(lines).scroll((app.help_scroll, 0)), area);
}

fn key_spans(text: &str) -> Vec<Span<'static>> {
    vec![Span::styled(text.to_string(), theme::PROMPT)]
}

/// Ctrl-G: the profiles, the one in use marked.
fn draw_profiles(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some((names, at)) = &app.profiles else {
        return;
    };
    let current = crate::config::profile();
    let mut lines: Vec<Line> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mark = if *name == current { "● " } else { "  " };
            let label = name.as_deref().unwrap_or("default (config.toml only)");
            let line = Line::from(vec![
                Span::styled(format!(" {mark}"), theme::INFO),
                Span::raw(label.to_string()),
            ]);
            if i == *at {
                line.patch_style(theme::CURRENT)
            } else {
                line
            }
        })
        .collect();
    // The row after the profiles makes a new one, from the folders in use.
    let new_row = match &app.naming {
        Some(name) => Line::from(vec![
            Span::styled(" + name: ", theme::PROMPT),
            Span::raw(name.clone()),
            Span::styled("▏", theme::PROMPT),
        ]),
        None => Line::styled(" + new profile", theme::PROMPT),
    };
    lines.push(if *at == names.len() {
        new_row.patch_style(Style::new().bg(theme::CURRENT.bg.expect("has a background")))
    } else {
        new_row
    });
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        if app.naming.is_some() {
            " enter make it  esc back"
        } else if names.len() == 1 {
            " a profile has its own folders and hidden list"
        } else {
            " enter switch  esc close"
        },
        theme::QUIET,
    ));
    let inner = (lines.iter().map(Line::width).max().unwrap_or(0) + 2).max(30) as u16;
    // Dropped down from the profile's name, like a menu opened from it.
    let anchor = app.profile_area;
    let width = (inner + 2).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(anchor.y + 1));
    let x = anchor.x.min(area.right().saturating_sub(width));
    let rect = Rect::new(x, anchor.y + 1, width, height);
    app.profile_rows = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        names_len(app),
    );
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::BORDER)
        .title(Span::styled(" profiles ", theme::BOLD));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The rows that take a click: the profiles and the "+ new profile" row.
fn names_len(app: &App) -> u16 {
    app.profiles
        .as_ref()
        .map_or(0, |(names, _)| names.len() as u16 + 1)
}

fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>, inner_w: u16) {
    let rect = popup(area, inner_w + 2, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::BORDER)
        .title(Span::styled(
            format!(" {title} "),
            Style::new().add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The folders searched for repositories: the checked ones are searched,
/// new ones found by looking again wait unchecked below them.
fn draw_folders(frame: &mut Frame, area: Rect, folders: &Folders) {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let mut lines = vec![
        Line::styled(" Folders to search", theme::BOLD),
        Line::styled(
            "   Repositories are looked for in the checked folders.",
            theme::QUIET,
        ),
        Line::raw(""),
    ];
    let path_w = folders
        .rows
        .iter()
        .map(|r| shorten(&r.path, home.as_deref()).width())
        .max()
        .unwrap_or(0);
    for (i, row) in folders.rows.iter().enumerate() {
        let current = i == folders.selected;
        let text = if row.on { Style::new() } else { theme::QUIET };
        let unit = if row.repos == 1 { "repo " } else { "repos" };
        let mut note = Vec::new();
        if row.missing {
            note.push("not found");
        }
        if row.new {
            note.push("new");
        }
        if row.dependency {
            note.push("downloaded dependencies");
        }
        let line = Line::from(vec![
            Span::styled(if current { "▌ " } else { "  " }, theme::POINTER),
            Span::styled(if row.on { "[x] " } else { "[ ] " }, text),
            Span::styled(pad(&shorten(&row.path, home.as_deref()), path_w), text),
            Span::styled(format!("{:>6} {unit}", row.repos), theme::INFO),
            Span::styled(
                if note.is_empty() {
                    String::new()
                } else {
                    format!("   {}", note.join(" · "))
                },
                if row.missing {
                    theme::NOTICE_BAD
                } else {
                    theme::QUIET
                },
            ),
        ]);
        lines.push(if current {
            line.patch_style(Style::new().bg(theme::CURRENT.bg.expect("has a background")))
        } else {
            line
        });
    }
    if folders.looking {
        lines.push(Line::styled("     looking for more folders…", theme::QUIET));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("   Another folder: cd there and run ", theme::QUIET),
        Span::styled("kura add .", theme::PROMPT),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " space toggle  enter save  esc cancel",
        theme::BORDER,
    ));
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_setup(frame: &mut Frame, area: Rect, setup: &Setup, app: &App) {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" kura ", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" Found repositories in these folders. Search them from now on?"),
        ]),
        Line::raw(""),
    ];
    if setup.candidates.is_empty() {
        lines.push(Line::styled(
            "   None found in your home folder or on your drives.",
            theme::QUIET,
        ));
        lines.push(Line::styled(
            "   Enter saves empty settings to fill in later with: kura config",
            theme::QUIET,
        ));
    }
    let path_w = setup
        .candidates
        .iter()
        .map(|c| shorten(&c.root, home.as_deref()).width())
        .max()
        .unwrap_or(0);
    for (i, candidate) in setup.candidates.iter().enumerate() {
        let on = setup.on[i];
        let current = i == setup.selected;
        let mut spans = vec![
            if current {
                Span::styled("▌ ", theme::POINTER)
            } else {
                Span::raw("  ")
            },
            Span::styled(
                if on { "[x] " } else { "[ ] " },
                if on { Style::new() } else { theme::QUIET },
            ),
            Span::styled(
                pad(&shorten(&candidate.root, home.as_deref()), path_w),
                if on { Style::new() } else { theme::QUIET },
            ),
            Span::styled(format!("{:>5} repos", candidate.count), theme::INFO),
        ];
        if candidate.dependency {
            spans.push(Span::styled(
                "   downloaded dependencies, off by default",
                theme::QUIET,
            ));
        }
        let line = Line::from(spans);
        lines.push(if current {
            line.patch_style(Style::new().bg(theme::CURRENT.bg.expect("has a background")))
        } else {
            line
        });
    }
    lines.push(Line::raw(""));
    let sample: Vec<Span> = ["rs", "go", "ts", "py", "md"]
        .iter()
        .map(|t| Span::styled(format!("{} ", crate::icons::glyph(Some(t))), theme::lang(t)))
        .collect();
    let mut icons = vec![
        Span::raw("  "),
        Span::raw(if setup.icons { "[x] " } else { "[ ] " }),
        Span::raw("Icons   "),
    ];
    icons.extend(sample);
    icons.push(Span::styled(
        if setup.nerd_font {
            "  a Nerd Font is installed; press i to try icons"
        } else {
            "  press i if your terminal uses a Nerd Font"
        },
        theme::QUIET,
    ));
    lines.push(Line::from(icons));
    lines.push(Line::raw(""));
    if let Ok(path) = crate::config::path() {
        lines.push(Line::styled(
            format!("   Saved to {}", path.display()),
            theme::QUIET,
        ));
    }
    lines.push(Line::raw(""));
    match &app.notice {
        Some(notice) => lines.push(Line::styled(format!(" {}", notice.text), theme::NOTICE_BAD)),
        None => lines.push(Line::styled(
            " space toggle  i icons  enter save  esc cancel",
            theme::BORDER,
        )),
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_in_order_as_the_window_narrows() {
        let wide = columns(100, 12, Some(6));
        assert!(wide.parent.is_some() && wide.age && wide.branch.is_some() && wide.lang);
        // No room for the folder, but the age still fits and stays.
        let no_folder = columns(62, 24, Some(12));
        assert!(no_folder.parent.is_none() && no_folder.age && no_folder.branch.is_some());
        let mid = columns(40, 12, Some(6));
        assert!(mid.parent.is_none() && !mid.age && mid.branch.is_some());
        let narrow = columns(16, 12, Some(6));
        assert!(narrow.branch.is_none() && !narrow.lang);
        assert!(POINTER + narrow.name <= 16);
    }

    #[test]
    fn long_text_is_cut_at_the_right_end() {
        assert_eq!(fit("dayplanner-notify", 8), "dayplan…");
        assert_eq!(fit_left(r"~\src\github\playground", 12), "…\\playground");
        assert_eq!(fit("短い", 4), "短い");
        assert_eq!(fit("日本語の名前", 7), "日本語…");
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

    fn sample() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for name in ["baton", "tally", "dayplanner-notify", "日本語のメモ"] {
            let path = tmp.path().join(name);
            std::fs::create_dir_all(path.join(".git")).unwrap();
            std::fs::write(
                path.join(".git/HEAD"),
                "ref: refs/heads/feat/tree
",
            )
            .unwrap();
            entries.push(crate::cache::Entry {
                path,
                root: tmp.path().into(),
                last_seen: 0,
                langs: None,
            });
        }
        let config = crate::config::Config::parse(
            "[open]
fork = 'Fork.exe \"{path}\"'
tig = { cmd = 'tig', mode = 'exec' }",
        )
        .unwrap();
        (tmp, App::list(config, entries, String::new()))
    }

    #[test]
    fn every_screen_draws_at_every_width() {
        let (_tmp, mut app) = sample();
        for (w, h) in [(120, 30), (60, 12), (34, 8), (20, 6), (12, 4)] {
            let screen = render(&mut app, w, h);
            if h >= 12 {
                assert!(screen.contains("baton"), "{screen}");
            }
            app.menu = Some(1);
            render(&mut app, w, h);
            app.menu = None;
            app.help = true;
            render(&mut app, w, h);
            app.help = false;
        }
    }

    #[test]
    fn the_tabs_show_both_lists_with_their_counts() {
        let (_tmp, mut app) = sample();
        let screen = render(&mut app, 80, 10);
        let tabs = screen.lines().next().unwrap();
        assert!(
            tabs.contains(" repos 4 ") && tabs.contains(" hidden 0 "),
            "{tabs}"
        );
        assert_eq!(app.tab_areas[0].y, 0);
        assert!(app.tab_areas[0].right() < app.tab_areas[1].x);
    }

    #[test]
    fn columns_stay_put_while_scrolling() {
        let (_tmp, mut app) = sample();
        let branch_column = |screen: &str| {
            let row = screen.lines().nth(2).unwrap();
            let byte = row.find("feat/tree").expect("a branch on the first row");
            row[..byte].chars().count()
        };
        let top = render(&mut app, 80, 6);
        let last = app.hits.last().map(|h| app.repos[h.index].key());
        app.selected = last;
        let bottom = render(&mut app, 80, 6);
        assert_ne!(
            top.lines().nth(2),
            bottom.lines().nth(2),
            "the list scrolled"
        );
        assert_eq!(branch_column(&top), branch_column(&bottom));
    }

    #[test]
    fn the_query_syntax_is_shown_until_typing_starts_and_in_the_help() {
        let (_tmp, mut app) = sample();
        let top = render(&mut app, 120, 30);
        // The program's name is not on the screen.
        assert!(!top.lines().take(2).any(|l| l.contains("kura")), "{top}");
        // The way to the help is with the other keys, at the bottom.
        assert!(top.lines().last().unwrap().contains("f1 help"), "{top}");
        assert!(!top.lines().nth(1).unwrap().contains("help"));
        // The quick key list keeps to actions.
        app.help = true;
        let quick = render(&mut app, 120, 40);
        assert!(
            quick.contains("action menu") && !quick.contains("only Rust"),
            "{quick}"
        );
        app.help = false;
        // The help page has the rest, under headings.
        app.help_page = true;
        let page = render(&mut app, 120, 50);
        assert!(page.contains("only Rust, then match tad"), "{page}");
        assert!(page.contains("go:"), "{page}");
        assert!(page.contains("2 commits to push"), "{page}");
        // Every description starts in one column.
        let starts: Vec<usize> = ["only Rust", "every repository", "move down"]
            .iter()
            .map(|t| {
                page.lines()
                    .find(|l| l.contains(t))
                    .unwrap()
                    .find(t)
                    .unwrap()
            })
            .collect();
        assert!(starts.windows(2).all(|w| w[0] == w[1]), "{starts:?}");
    }

    #[test]
    fn the_age_comes_before_the_folder() {
        let (tmp, mut app) = sample();
        let screen = render(&mut app, 120, 10);
        let row = screen.lines().nth(2).unwrap();
        let folder = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let age = row.find("now").expect("an age");
        assert!(age < row.find(&folder).expect("a folder"), "{row}");
    }

    #[test]
    fn state_and_preview_show_what_git_said() {
        let (_tmp, mut app) = sample();
        let baton = app
            .repos
            .iter()
            .find(|r| r.name == "baton")
            .unwrap()
            .clone();
        app.statuses.insert(
            baton.key(),
            Some(Status {
                dirty: 3,
                ahead: 2,
                upstream: Some("origin/main".into()),
                files: vec![" M src/app.rs".into(), "?? notes.md".into()],
                ..Status::default()
            }),
        );
        let mut weeks = vec![0; crate::gitstat::WEEKS];
        weeks[11] = 5;
        weeks[9] = 2;
        app.logs.insert(
            baton.key(),
            Some(crate::gitstat::Log {
                commits: (0..30)
                    .map(|i| {
                        (
                            format!("{i:07}"),
                            "2h".into(),
                            format!("fix: keep columns {i}"),
                        )
                    })
                    .collect(),
                remote: Some("git@github.com:me/baton.git".into()),
                weeks,
                branches: vec![crate::gitstat::Branch {
                    name: "feat/posture".into(),
                    age: 3 * 86_400,
                    ahead: 3,
                    ..Default::default()
                }],
            }),
        );
        app.abouts.insert(
            baton.key(),
            Some("A desktop companion that reacts to your posture and notifications.".into()),
        );
        app.mixes
            .insert(baton.key(), vec![("ts", 72), ("rs", 21), ("ht", 7)]);
        app.selected = Some(baton.key());
        app.refresh_for_tests();
        let screen = render(&mut app, 140, 30);
        assert!(screen.contains("A desktop companion"), "{screen}");
        assert!(screen.contains("ts 72%  rs 21%  ht 7%"), "{screen}");
        assert!(screen.contains("━━━"), "{screen}");
        assert!(screen.contains("7 in 12 weeks"), "{screen}");
        assert!(screen.contains("feat/posture ↑3"), "{screen}");
        // Recent commits fill the panel to its last line, and no further.
        let last = screen.lines().nth(26).unwrap();
        assert!(last.contains("fix: keep columns"), "{screen}");
        assert!(screen.contains("●3 ↑2"), "{screen}");
        assert!(screen.contains("●3 changed  ↑2 to push"), "{screen}");
        assert!(screen.contains("→ origin/main"), "{screen}");
        assert!(screen.contains(" M src/app.rs"), "{screen}");
        assert!(screen.contains("github.com/me/baton"), "{screen}");
        // Narrow windows give the list the whole width.
        assert!(!render(&mut app, 100, 24).contains("recent"));
    }

    #[test]
    fn about_wraps_at_spaces_and_anywhere_without_them() {
        assert_eq!(
            wrap("a desktop companion app", 10),
            ["a desktop", "companion", "app"]
        );
        assert_eq!(
            wrap("日本語の説明文です", 8),
            ["日本語の", "説明文で", "す"]
        );
        let bar = language_bar(&[("ts", 72), ("rs", 21), ("ht", 7)], 20);
        assert_eq!(bar.width(), 21, "one space and the full width");
        let line: String = sparkline(&[0, 1, 4])
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(line, "·▃█");
    }

    #[test]
    fn folders_read_from_their_root_and_twins_in_full() {
        let repo = |p: &str| crate::repo::Repo::inspect(std::path::PathBuf::from(p));
        let roots = [std::path::PathBuf::from("/work")];
        assert_eq!(folder(&repo("/work/C/linux"), &roots, None, false), "C");
        assert_eq!(folder(&repo("/work/scratch"), &roots, None, false), "");
        let elsewhere = folder(&repo("/other/x"), &roots, None, false);
        assert_eq!(elsewhere.replace('\\', "/"), "/other");
        let twin = folder(&repo("/work/C/linux"), &roots, None, true);
        assert_eq!(twin.replace('\\', "/"), "/work/C");
    }

    #[test]
    fn the_profile_opens_as_a_drop_down_under_its_name() {
        let (_tmp, mut app) = sample();
        let header = render(&mut app, 100, 16);
        // At the head of the tab row, the top line, before repos; the query
        // line below it, right above the list, is bare.
        let tabs = header.lines().next().unwrap();
        assert!(tabs.find("default ▾") < tabs.find("repos"), "{header}");
        let query = header.lines().nth(1).unwrap();
        assert!(
            query.trim_start().starts_with('>') && !query.contains("default"),
            "{header}"
        );
        app.profiles = Some((vec![None, Some("work".into())], 1));
        let screen = render(&mut app, 100, 16);
        let lines: Vec<&str> = screen.lines().collect();
        // Columns, not bytes: the cursor bar before the box is multi-byte.
        let column = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        let name_at = column(lines[0], "default").unwrap();
        // The box opens on the next line, starting at the name.
        assert_eq!(column(lines[1], "┌"), Some(name_at - 1), "{screen}");
        assert!(screen.contains("work"));
        assert!(screen.contains("+ new profile"));
        assert_eq!(app.profile_rows.y, 2);
        assert_eq!(app.profile_rows.height, 3);
    }

    #[test]
    fn tabs_light_up_under_the_mouse_and_rest_unfilled() {
        let (_tmp, mut app) = sample();
        let bg_at = |app: &mut App, x: u16, y: u16| {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 12)).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            terminal.backend().buffer()[(x, y)].bg
        };
        render(&mut app, 100, 12);
        let hidden = app.tab_areas[2];
        let at_rest = bg_at(&mut app, hidden.x + 1, hidden.y);
        // No fill at rest, so the tabs never look like the cursor row.
        assert_eq!(at_rest, ratatui::style::Color::Reset);
        let tabs = render(&mut app, 100, 12);
        assert!(
            tabs.lines().next().unwrap().contains(" │ changed "),
            "{tabs}"
        );
        app.hover = Some((hidden.x + 1, hidden.y));
        assert_eq!(
            Some(bg_at(&mut app, hidden.x + 1, hidden.y)),
            theme::SWITCH_HOVER.bg
        );
        // The tab in use keeps its colour.
        let repos = app.tab_areas[0];
        assert_eq!(Some(bg_at(&mut app, repos.x + 1, repos.y)), theme::TAB.bg);
    }

    #[test]
    fn the_menu_shows_its_letters_and_what_follows() {
        let (_tmp, mut app) = sample();
        app.menu = Some(0);
        let screen = render(&mut app, 90, 16);
        assert!(screen.contains("f  fork"), "{screen}");
        assert!(screen.contains("[terminal]"), "{screen}");
        assert!(screen.contains("after: as set"), "{screen}");
    }

    #[test]
    fn setup_lists_the_candidates() {
        let candidates = vec![crate::first_run::Candidate {
            root: "/work".into(),
            count: 41,
            dependency: false,
        }];
        let mut app = App::setup(candidates, Vec::new(), String::new());
        let screen = render(&mut app, 90, 10);
        assert!(screen.contains("[x] /work"), "{screen}");
        assert!(screen.contains("41 repos"), "{screen}");
    }

    #[test]
    fn ages_read_at_a_glance() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100_000_000);
        let ago = |s: u64| age(Some(now - std::time::Duration::from_secs(s)), now);
        assert_eq!(ago(30), ("now".into(), true));
        assert_eq!(ago(7200), ("2h".into(), true));
        assert_eq!(ago(3 * 86_400), ("3d".into(), false));
        assert_eq!(ago(90 * 86_400), ("3mo".into(), false));
    }
}
