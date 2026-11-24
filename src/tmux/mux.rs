//! What negi asks of tmux, as typed calls over a [`Link`].

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::tmux::link::{Error, Link};
use crate::tmux::proto::Ask;
use crate::tmux::{Tmux, is_id, quote};

/// Unit separator between listing fields. tmux passes it through untouched under `-u`.
const SEP: char = '\u{1f}';

/// Bytes per `send-keys -H` command. tmux's parser overflows its stack on a command with
/// thousands of arguments ("yacc stack overflow"), so text goes in small pieces.
const HEX_CHUNK: usize = 256;

/// Pause between a text and the Enter that submits it, so a full-screen program has read
/// the text before the key arrives. A guess, not a measurement.
const SUBMIT_SETTLE: Duration = Duration::from_millis(150);

pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_HISTORY: u32 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    /// Shown by a real terminal right now. Control clients, negi's own included, do
    /// not count, although tmux's `session_attached` counts them.
    pub attached: bool,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pane {
    pub id: String,
    pub index: u32,
    pub active: bool,
    pub cols: u16,
    pub rows: u16,
    pub command: String,
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub cx: u16,
    pub cy: u16,
    pub cursor: bool,
    /// What the program asked for (block, bar, underline); empty before tmux 3.5 or when it
    /// asked for nothing.
    pub shape: String,
    /// On the alternate screen (an editor, a full-screen program): there is no history.
    pub alt: bool,
    /// One string per row, with SGR colour sequences as tmux printed them.
    pub lines: Vec<String>,
}

/// Opens a control client for a one-off caller and proves it answers.
pub async fn open(tmux: &Tmux) -> Result<Link, Error> {
    let (tx, _rx) = mpsc::unbounded_channel();
    let link = Link::attach(tmux, None, tx)?;
    session_of(&link).await?;
    Ok(link)
}

/// The session a link is attached to.
pub async fn session_of(link: &Link) -> Result<String, Error> {
    let out = link
        .run("display-message -p '#{session_id}'".to_owned())
        .await?;
    let id = text(&out).trim().to_owned();
    if is_id('$', &id) {
        Ok(id)
    } else {
        Err(Error::Tmux(format!("unexpected session id {id:?}")))
    }
}

pub async fn version(link: &Link) -> Result<String, Error> {
    let out = link
        .run("display-message -p '#{version}'".to_owned())
        .await?;
    Ok(text(&out).trim().to_owned())
}

pub async fn tree(link: &Link) -> Result<Vec<Session>, Error> {
    let f = |fields: &[&str]| format!("'{}'", fields.join(&SEP.to_string()));
    let lines = vec![
        format!(
            "list-sessions -F {}",
            f(&["#{session_id}", "#{session_name}"])
        ),
        format!(
            "list-windows -a -F {}",
            f(&[
                "#{session_id}",
                "#{window_id}",
                "#{window_index}",
                "#{window_active}",
                "#{window_name}"
            ])
        ),
        format!(
            "list-panes -a -F {}",
            f(&[
                "#{window_id}",
                "#{pane_id}",
                "#{pane_index}",
                "#{pane_active}",
                "#{pane_width}",
                "#{pane_height}",
                "#{pane_current_command}",
                "#{pane_current_path}",
                "#{pane_title}",
            ])
        ),
        format!(
            "list-clients -F {}",
            f(&["#{session_id}", "#{client_control_mode}"])
        ),
    ];
    let mut out = Vec::with_capacity(4);
    for reply in link.batch(lines).await? {
        out.push(listing(&reply?));
    }
    Ok(parse_tree(&out[0], &out[1], &out[2], &out[3]))
}

fn parse_tree(sessions: &str, windows: &str, panes: &str, clients: &str) -> Vec<Session> {
    let shown: HashSet<&str> = clients
        .lines()
        .filter_map(|l| l.split_once(SEP))
        .filter(|(_, control)| *control != "1")
        .map(|(session, _)| session)
        .collect();

    let mut by_window: HashMap<&str, Vec<Pane>> = HashMap::new();
    for line in panes.lines() {
        // The title is free text, so it goes last and keeps any separator it contains.
        let f: Vec<&str> = line.splitn(9, SEP).collect();
        if f.len() < 9 {
            continue;
        }
        let list = by_window.entry(f[0]).or_default();
        // A window linked into two sessions lists its panes twice.
        if list.iter().any(|p| p.id == f[1]) {
            continue;
        }
        list.push(Pane {
            id: f[1].to_owned(),
            index: f[2].parse().unwrap_or(0),
            active: f[3] == "1",
            cols: f[4].parse().unwrap_or(0),
            rows: f[5].parse().unwrap_or(0),
            command: f[6].to_owned(),
            path: f[7].to_owned(),
            title: f[8].to_owned(),
        });
    }

    let mut by_session: HashMap<&str, Vec<Window>> = HashMap::new();
    for line in windows.lines() {
        let f: Vec<&str> = line.splitn(5, SEP).collect();
        if f.len() < 5 {
            continue;
        }
        by_session.entry(f[0]).or_default().push(Window {
            id: f[1].to_owned(),
            index: f[2].parse().unwrap_or(0),
            active: f[3] == "1",
            name: f[4].to_owned(),
            panes: by_window.get(f[1]).cloned().unwrap_or_default(),
        });
    }

    sessions
        .lines()
        .filter_map(|l| l.split_once(SEP))
        .map(|(id, name)| {
            let mut windows = by_session.remove(id).unwrap_or_default();
            windows.sort_by_key(|w| w.index);
            Session {
                id: id.to_owned(),
                name: name.to_owned(),
                attached: shown.contains(id),
                windows,
            }
        })
        .collect()
}

pub async fn screen(link: &Link, pane: &str) -> Result<Screen, Error> {
    need('%', pane)?;
    let meta =
        "#{pane_width} #{pane_height} #{cursor_x} #{cursor_y} #{cursor_flag} #{alternate_on} #{cursor_shape}";
    let mut replies = link
        .batch(vec![
            format!("display-message -p -t {pane} '{meta}'"),
            format!("capture-pane -p -e -t {pane}"),
        ])
        .await?
        .into_iter();
    let meta = text(&replies.next().expect("two replies")?);
    let body = replies.next().expect("two replies")?;
    let words: Vec<&str> = meta.split_whitespace().collect();
    let n: Vec<u16> = words.iter().take(6).map(|v| v.parse().unwrap_or(0)).collect();
    if n.len() < 6 {
        return Err(Error::Tmux(format!("unexpected pane state {meta:?}")));
    }
    Ok(Screen {
        cols: n[0],
        rows: n[1],
        cx: n[2],
        cy: n[3],
        cursor: n[4] == 1,
        alt: n[5] == 1,
        shape: words.get(6).filter(|s| **s != "default").map_or_else(String::new, |s| (*s).to_owned()),
        lines: rows(&body),
    })
}

/// Which of `panes` show an agent at work on screen. An agent waiting on a long command
/// can print nothing for a minute while its status line still says it is working.
pub async fn working(link: &Link, panes: &[String]) -> Result<Vec<String>, Error> {
    if panes.is_empty() {
        return Ok(Vec::new());
    }
    let replies = link
        .batch(panes.iter().map(|p| format!("capture-pane -p -t {p}")).collect())
        .await?;
    Ok(panes
        .iter()
        .zip(replies)
        .filter(|(_, reply)| reply.as_ref().is_ok_and(|body| agent_at_work(&String::from_utf8_lossy(body))))
        .map(|(pane, _)| pane.clone())
        .collect())
}

/// An agent's "still going" line in the last 40 lines (a task list can sit under it): Claude
/// Code's `✽ Gesticulating… (7m 6s` from the first column, or Codex's `esc to interrupt`.
pub fn agent_at_work(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(40)..].iter().any(|line| {
        if line.to_ascii_lowercase().contains("esc to interrupt") {
            return true;
        }
        let mut words = line.splitn(3, ' ');
        let (Some(glyph), Some(verb), Some(rest)) = (words.next(), words.next(), words.next()) else {
            return false;
        };
        glyph.chars().count() == 1
            && !glyph.chars().all(char::is_alphanumeric)
            && verb.ends_with('…')
            && rest.starts_with('(')
            && rest[1..].starts_with(|c: char| c.is_ascii_digit())
    })
}

/// The `count` lines above the screen, oldest first. Empty on the alternate screen.
pub async fn history(link: &Link, pane: &str, count: u32) -> Result<Vec<String>, Error> {
    need('%', pane)?;
    let count = count.clamp(1, MAX_HISTORY);
    let body = link
        .run(format!("capture-pane -p -e -t {pane} -S -{count} -E -1"))
        .await?;
    Ok(rows(&body))
}

/// Whether the pane shows the alternate screen: a full-screen program such as an editor.
/// What tmux keeps as the pane's history then belongs to the shell underneath, not to it.
pub async fn alternate(link: &Link, pane: &str) -> Result<bool, Error> {
    need('%', pane)?;
    let on = link
        .run(format!("display-message -p -t {pane} '#{{alternate_on}}'"))
        .await?;
    Ok(on.first() == Some(&b'1'))
}

/// Presses named keys: `Enter`, `Escape`, `C-c`, `M-x`, `Up`, a single character.
pub async fn keys(link: &Link, pane: &str, keys: &[String]) -> Result<(), Error> {
    need('%', pane)?;
    if keys.is_empty() || keys.len() > 64 {
        return Err(Error::Invalid("between 1 and 64 keys"));
    }
    let mut line = format!("send-keys -t {pane} --");
    for key in keys {
        if !is_key(key) {
            return Err(Error::Invalid("not a key name"));
        }
        line.push(' ');
        line.push_str(&quote(key).ok_or(Error::Invalid("not a key name"))?);
    }
    link.run(line).await.map(drop)
}

/// Types text as if at the keyboard, then Enter when `enter` is set. It goes as hex bytes
/// (`send-keys -H`), so tmux parses none of it; text with line breaks is bracketed as one
/// paste when the program asked for that.
pub async fn text_in(link: &Link, pane: &str, input: &str, enter: bool) -> Result<(), Error> {
    need('%', pane)?;
    if input.len() > MAX_TEXT {
        return Err(Error::Invalid("text too long"));
    }
    let mut bytes = typed(input);
    if bytes.is_empty() && !enter {
        return Err(Error::Invalid("nothing to type"));
    }
    if bytes.contains(&b'\r') {
        let flag = link
            .run(format!(
                "display-message -p -t {pane} '#{{bracket_paste_flag}}'"
            ))
            .await?;
        match text(&flag).trim() {
            "1" => bytes = [b"\x1b[200~", bytes.as_slice(), b"\x1b[201~"].concat(),
            "0" => {}
            // tmux 3.5a and older have no such format: their own paste-buffer -p brackets the
            // text when, and only when, the program asked for bracketed paste.
            _ => {
                paste(link, pane, &bytes).await?;
                bytes.clear();
            }
        }
    }
    let sent = !bytes.is_empty();
    if sent {
        let lines = bytes
            .chunks(HEX_CHUNK)
            .map(|chunk| {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                format!("send-keys -t {pane} -H {}", hex.join(" "))
            })
            .collect();
        for reply in link.batch(lines).await? {
            reply?;
        }
    }
    if enter {
        if sent {
            tokio::time::sleep(SUBMIT_SETTLE).await;
        }
        link.run(format!("send-keys -t {pane} Enter")).await?;
    }
    Ok(())
}

/// Pastes `bytes` with tmux's own `paste-buffer -p`, through a file only this user can read,
/// removed at once.
async fn paste(link: &Link, pane: &str, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut tag = [0u8; 8];
    getrandom::fill(&mut tag).map_err(|e| Error::Tmux(e.to_string()))?;
    let name = format!("negi-{}", tag.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let path = std::env::temp_dir().join(&name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| file.write_all(bytes));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(Error::Tmux(e.to_string()));
    }
    let replies = link
        .batch(vec![
            format!("load-buffer -b {name} '{}'", path.display()),
            format!("paste-buffer -p -d -b {name} -t {pane}"),
        ])
        .await;
    let _ = std::fs::remove_file(&path);
    for reply in replies? {
        reply?;
    }
    Ok(())
}

/// What a keyboard would send for this text: line breaks become carriage returns, and
/// control characters other than tab are dropped, the paste terminator among them.
fn typed(input: &str) -> Vec<u8> {
    let unified = input.replace("\r\n", "\n");
    let mut out = String::with_capacity(unified.len());
    for c in unified.chars() {
        match c {
            '\n' | '\r' => out.push('\r'),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.into_bytes()
}

pub async fn new_session(
    link: &Link,
    name: Option<&str>,
    cwd: Option<&str>,
) -> Result<String, Error> {
    let mut line = "new-session -d -P -F '#{pane_id}'".to_owned();
    if let Some(name) = name {
        line.push_str(&format!(" -s {}", label(name)?));
    }
    if let Some(cwd) = cwd {
        line.push_str(&format!(" -c {}", folder(cwd)?));
    }
    created(link.run(line).await?)
}

/// Arguments for a plain `tmux new-session`, for when no server runs yet and there is no
/// control client to ask.
pub fn new_session_args(name: Option<&str>, cwd: Option<&str>) -> Result<Vec<String>, Error> {
    let mut args: Vec<String> = ["new-session", "-d", "-P", "-F", "#{pane_id}"]
        .map(String::from)
        .to_vec();
    if let Some(name) = name {
        label(name)?;
        args.extend(["-s".to_owned(), literal(name)]);
    }
    if let Some(cwd) = cwd {
        folder(cwd)?;
        args.extend(["-c".to_owned(), literal(cwd)]);
    }
    Ok(args)
}

pub async fn new_window(
    link: &Link,
    session: &str,
    name: Option<&str>,
    cwd: Option<&str>,
) -> Result<String, Error> {
    need('$', session)?;
    // The trailing colon makes tmux pick the next free index in that session.
    let mut line = format!("new-window -d -P -F '#{{pane_id}}' -t '{session}:'");
    if let Some(name) = name {
        line.push_str(&format!(" -n {}", label(name)?));
    }
    if let Some(cwd) = cwd {
        line.push_str(&format!(" -c {}", folder(cwd)?));
    }
    created(link.run(line).await?)
}

pub async fn rename_session(link: &Link, session: &str, name: &str) -> Result<(), Error> {
    need('$', session)?;
    link.run(format!("rename-session -t '{session}' -- {}", label(name)?))
        .await
        .map(drop)
}

pub async fn rename_window(link: &Link, window: &str, name: &str) -> Result<(), Error> {
    need('@', window)?;
    link.run(format!("rename-window -t {window} -- {}", label(name)?))
        .await
        .map(drop)
}

pub async fn select_window(link: &Link, window: &str) -> Result<(), Error> {
    need('@', window)?;
    link.run(format!("select-window -t {window}"))
        .await
        .map(drop)
}

pub async fn kill_session(link: &Link, session: &str) -> Result<(), Error> {
    need('$', session)?;
    link.run(format!("kill-session -t '{session}'"))
        .await
        .map(drop)
}

pub async fn kill_window(link: &Link, window: &str) -> Result<(), Error> {
    need('@', window)?;
    link.run(format!("kill-window -t {window}")).await.map(drop)
}

/// Clicks at column `x`, row `y` (from 0) if the pane's program asked for the mouse, and
/// says whether it had. tmux has no click command, so the program gets a terminal's press
/// and release, in SGR form or the older one.
pub async fn click(link: &Link, pane: &str, x: u16, y: u16) -> Result<bool, Error> {
    // Button 0, pressed and released.
    mouse(link, pane, x, y, &[(0, true), (0, false)]).await
}

/// Turns the wheel `lines` notches (up when negative) over `x`, `y`, for a program that asked
/// for the mouse: how a full-screen program with its own scrollback is scrolled.
pub async fn wheel(link: &Link, pane: &str, x: u16, y: u16, lines: i16) -> Result<bool, Error> {
    // Wheel up is button 64, wheel down 65; a notch is a press with no release.
    let button = if lines < 0 { 64 } else { 65 };
    let notches = vec![(button, true); usize::from(lines.unsigned_abs().clamp(1, 30))];
    mouse(link, pane, x, y, &notches).await
}

/// Sends mouse events, each a button and whether it is a press, at one cell.
async fn mouse(
    link: &Link,
    pane: &str,
    x: u16,
    y: u16,
    events: &[(u8, bool)],
) -> Result<bool, Error> {
    need('%', pane)?;
    let flags = link
        .run(format!(
            "display-message -p -t {pane} '#{{mouse_any_flag}}#{{mouse_sgr_flag}}'"
        ))
        .await?;
    let flags = text(flags.strip_suffix(b"\n").unwrap_or(&flags));
    if !flags.starts_with('1') {
        return Ok(false);
    }
    let (col, row) = (u32::from(x.min(998)) + 1, u32::from(y.min(998)) + 1);
    let sgr = flags.ends_with('1');
    if !sgr && (col > 223 || row > 223) {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    for &(button, press) in events {
        if sgr {
            let end = if press { 'M' } else { 'm' };
            bytes.extend(format!("\x1b[<{button};{col};{row}{end}").into_bytes());
        } else {
            // The older form holds each number in one byte, offset by 32, and has one code for every release.
            let code = if press { button } else { 3 };
            bytes.extend([
                0x1b,
                b'[',
                b'M',
                code + 32,
                (col + 32) as u8,
                (row + 32) as u8,
            ]);
        }
    }
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    link.run(format!("send-keys -H -t {pane} {}", hex.join(" ")))
        .await?;
    Ok(true)
}

/// Sizes `pane`'s window to `cols` by `rows` for a phone. tmux then stops sizing it from its
/// clients, so [`unfit`] must follow.
pub async fn fit(link: &Link, pane: &str, cols: u16, rows: u16) -> Result<(), Error> {
    need('%', pane)?;
    let (cols, rows) = (cols.clamp(20, 400), rows.clamp(5, 200));
    link.run(format!("resize-window -t {pane} -x {cols} -y {rows}"))
        .await
        .map(drop)
}

/// Gives the window that holds `pane` back to tmux to size from its clients.
pub async fn unfit(link: &Link, pane: &str) -> Result<(), Error> {
    need('%', pane)?;
    // `resize-window` leaves `window-size manual` on the window; without the second
    // command it would stay at whatever size it has now.
    link.run(format!("resize-window -A -t {pane}")).await?;
    link.run(format!("set-option -w -u -t {pane} window-size"))
        .await
        .map(drop)
}

/// Makes the pane beside `pane` in direction `dir` (`L`, `R`, `U` or `D`) the active one, as
/// tmux's own prefix and arrow do, and answers with its id.
pub async fn select_pane(link: &Link, pane: &str, dir: &str) -> Result<String, Error> {
    need('%', pane)?;
    if !matches!(dir, "L" | "R" | "U" | "D") {
        return Err(Error::Invalid("not a direction"));
    }
    let window = text(&link.run(format!("display-message -p -t {pane} '#{{window_id}}'")).await?)
        .trim()
        .to_owned();
    need('@', &window)?;
    link.run(format!("select-pane -t {pane} -{dir}")).await?;
    created(link.run(format!("display-message -p -t {window} '#{{pane_id}}'")).await?)
}

/// Splits `pane` in its own folder: side by side when `across`, otherwise one above the other.
pub async fn split(link: &Link, pane: &str, across: bool) -> Result<String, Error> {
    need('%', pane)?;
    let side = if across { "-h" } else { "-v" };
    created(
        link.run(format!(
            "split-window {side} -P -F '#{{pane_id}}' -t {pane} -c '#{{pane_current_path}}'"
        ))
        .await?,
    )
}

/// The largest copy passed on to a phone; a bigger one is left on the host.
const MAX_COPY: usize = 256 * 1024;

/// A paste buffer's text, or `None` when it is too large to pass on.
pub async fn buffer(link: &Link, name: &str) -> Result<Option<String>, Error> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Ok(None);
    }
    let body = link.run(format!("show-buffer -b {name}")).await?;
    if body.len() > MAX_COPY {
        return Ok(None);
    }
    let mut text = String::from_utf8_lossy(&body).into_owned();
    // A reply block ends every line with a newline, the buffer's last one included.
    if text.ends_with('\n') {
        text.pop();
    }
    Ok(Some(text))
}

/// Answers what `pane` asked the terminal outside tmux, unless a terminal shows its window and
/// so got the question itself. Returns whether negi answered.
pub async fn answer(link: &Link, pane: &str, asks: &[Ask]) -> Result<bool, Error> {
    need('%', pane)?;
    let window = text(&link.run(format!("display-message -p -t {pane} '#{{window_id}}'")).await?)
        .trim()
        .to_owned();
    need('@', &window)?;
    let clients = text(&link.run("list-clients -F '#{client_control_mode} #{window_id}'".into()).await?);
    if clients.lines().any(|l| l == format!("0 {window}")) {
        return Ok(false);
    }
    let hex: Vec<String> = asks.iter().flat_map(|a| a.answer()).map(|b| format!("{b:02x}")).collect();
    link.run(format!("send-keys -t {pane} -H {}", hex.join(" "))).await?;
    Ok(true)
}

pub async fn kill_pane(link: &Link, pane: &str) -> Result<(), Error> {
    need('%', pane)?;
    link.run(format!("kill-pane -t {pane}")).await.map(drop)
}

fn need(sigil: char, id: &str) -> Result<(), Error> {
    if is_id(sigil, id) {
        Ok(())
    } else {
        Err(Error::Invalid("not a tmux id"))
    }
}

/// tmux expands formats in the names and folders these commands take: `#{pane_id}` would
/// become an id and `#(cmd)` would run a command. Doubling the hash keeps it literal.
fn literal(value: &str) -> String {
    value.replace('#', "##")
}

fn label(name: &str) -> Result<String, Error> {
    if name.trim().is_empty() || name.chars().count() > 64 {
        return Err(Error::Invalid("a name has 1 to 64 characters"));
    }
    quote(&literal(name)).ok_or(Error::Invalid("a name has no control characters"))
}

fn folder(path: &str) -> Result<String, Error> {
    if !path.starts_with('/') {
        return Err(Error::Invalid("a folder is an absolute path"));
    }
    quote(&literal(path)).ok_or(Error::Invalid("a folder has no control characters"))
}

fn created(out: Vec<u8>) -> Result<String, Error> {
    let id = text(&out).trim().to_owned();
    if is_id('%', &id) {
        Ok(id)
    } else {
        Err(Error::Tmux(format!("unexpected pane id {id:?}")))
    }
}

/// A key as tmux names it: modifiers `C-`, `M-`, `S-`, then a named key or one printable
/// ASCII character.
pub fn is_key(key: &str) -> bool {
    const NAMED: &[&str] = &[
        "Enter", "Escape", "Tab", "BTab", "BSpace", "Space", "Up", "Down", "Left", "Right", "Home",
        "End", "PPage", "NPage", "IC", "DC", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9",
        "F10", "F11", "F12",
    ];
    let mut rest = key;
    for _ in 0..3 {
        match rest.get(..2) {
            Some("C-" | "M-" | "S-") if rest.len() > 2 => rest = &rest[2..],
            _ => break,
        }
    }
    NAMED.contains(&rest) || matches!(rest.as_bytes(), [b] if b.is_ascii_graphic())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A `-F` listing with its fields apart again. tmux 3.5a and older (3.4 on Ubuntu 24.04)
/// write the separator as the four characters `\037`; 3.8 writes the byte.
fn listing(bytes: &[u8]) -> String {
    text(bytes).replace("\\037", &SEP.to_string())
}

fn rows(body: &[u8]) -> Vec<String> {
    let mut lines: Vec<String> = body.split(|b| *b == b'\n').map(text).collect();
    // The block ends with a newline, which is not an extra row.
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(fields: &[&str]) -> String {
        fields.join(&SEP.to_string())
    }

    #[test]
    fn an_older_tmux_listing_becomes_the_same_tree() {
        // As tmux 3.4 and 3.5a send it: the separator spelled out as `\037`.
        let old = |fields: &[&str]| fields.join("\\037");
        let sessions = listing(old(&["$0", "probe"]).as_bytes());
        let windows = listing(old(&["$0", "@0", "0", "1", "cat"]).as_bytes());
        let panes = listing(old(&["@0", "%0", "0", "1", "100", "30", "cat", "/home/m", "probe"]).as_bytes());
        let tree = parse_tree(&sessions, &windows, &panes, "");
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "probe");
        assert_eq!(tree[0].windows[0].panes[0].id, "%0");
    }

    #[test]
    fn a_listing_becomes_a_tree() {
        let sessions = [join(&["$0", "al pha"]), join(&["$1", "beta"])].join("\n");
        let windows = [
            join(&["$0", "@1", "2", "0", "logs"]),
            join(&["$0", "@0", "1", "1", "w 'x' é"]),
            join(&["$1", "@2", "0", "1", "zsh"]),
        ]
        .join("\n");
        let panes = [
            join(&["@0", "%0", "0", "1", "80", "24", "zsh", "/Users/me", "host"]),
            join(&["@0", "%3", "1", "0", "40", "24", "vim", "/tmp/a b", "notes"]),
            join(&["@2", "%2", "0", "1", "100", "30", "cat", "/", ""]),
        ]
        .join("\n");
        let clients = [join(&["$0", "1"]), join(&["$1", "1"]), join(&["$1", "0"])].join("\n");

        let tree = parse_tree(&sessions, &windows, &panes, &clients);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].name, "al pha");
        assert!(!tree[0].attached, "only a control client is on it");
        assert!(tree[1].attached);
        let w: Vec<&str> = tree[0].windows.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(w, ["w 'x' é", "logs"], "ordered by index");
        assert!(tree[0].windows[0].active);
        assert_eq!(tree[0].windows[0].panes.len(), 2);
        assert_eq!(tree[0].windows[0].panes[1].path, "/tmp/a b");
        assert!(tree[0].windows[1].panes.is_empty());
        assert_eq!(tree[1].windows[0].panes[0].cols, 100);
    }

    #[test]
    fn a_title_keeps_its_own_separators() {
        let panes = join(&["@0", "%0", "0", "1", "80", "24", "zsh", "/", "a\u{1f}b"]);
        let tree = parse_tree(
            &join(&["$0", "s"]),
            &join(&["$0", "@0", "0", "1", "w"]),
            &panes,
            "",
        );
        assert_eq!(tree[0].windows[0].panes[0].title, "a\u{1f}b");
    }

    #[test]
    fn a_linked_window_lists_its_panes_once() {
        let line = join(&["@0", "%0", "0", "1", "80", "24", "zsh", "/", ""]);
        let panes = format!("{line}\n{line}");
        let windows = [
            join(&["$0", "@0", "0", "1", "w"]),
            join(&["$1", "@0", "0", "1", "w"]),
        ]
        .join("\n");
        let sessions = [join(&["$0", "a"]), join(&["$1", "b"])].join("\n");
        let tree = parse_tree(&sessions, &windows, &panes, "");
        assert_eq!(tree[0].windows[0].panes.len(), 1);
        assert_eq!(tree[1].windows[0].panes.len(), 1);
    }

    #[test]
    fn key_names() {
        for ok in [
            "Enter", "Escape", "C-c", "M-x", "S-Tab", "BTab", "C-M-Left", "a", ";", "~", "F12", "-",
        ] {
            assert!(is_key(ok), "{ok}");
        }
        for bad in [
            "",
            "enter",
            "C-",
            "ab",
            "Enter;kill-server",
            "C-M-S-M-a",
            " ",
            "é",
            "a b",
            "\n",
        ] {
            assert!(!is_key(bad), "{bad:?}");
        }
    }

    #[test]
    fn typed_text_is_what_a_keyboard_sends() {
        assert_eq!(typed("a\nb\r\nc"), b"a\rb\rc");
        assert_eq!(typed("tab\there"), b"tab\there");
        assert_eq!(typed("x\u{1b}[201~y\u{7f}\u{0}"), b"x[201~y");
        assert_eq!(typed("é✓"), "é✓".as_bytes());
    }

    #[test]
    fn rows_drop_only_the_final_newline() {
        assert_eq!(rows(b"a\n\nb\n\n"), ["a", "", "b", ""]);
        assert_eq!(rows(b""), Vec::<String>::new());
    }

    #[test]
    fn an_agent_at_work_is_read_from_its_status_line() {
        assert!(agent_at_work("text\n✽ Gesticulating… (7m 6s · ↓ 18.5k tokens)\n  ⎿  Tip"));
        assert!(agent_at_work("✢ Ideating… (5s · ↓ 1k tokens)"));
        assert!(agent_at_work("• Working (12s • Esc to interrupt)"));
        assert!(!agent_at_work("✻ Churned for 5s · done Friday 12:42 PM"));
        assert!(!agent_at_work("❯ \n  22:46:51 with rifuki in ~"));
        assert!(!agent_at_work("Loading… (please wait)"));
        assert!(!agent_at_work("a Thinking… (5s)"));
        // Quoted in a transcript: indented, or long scrolled up from the bottom.
        assert!(!agent_at_work("  ✽ Gesticulating… (7m 6s · ↓ 18.5k tokens)\n❯ "));
        let far = format!("✽ Gesticulating… (7m 6s)\n{}", "line\n".repeat(41));
        assert!(!agent_at_work(&far));
        let quoted = format!("it said esc to interrupt\n{}", "line\n".repeat(41));
        assert!(!agent_at_work(&quoted));
        // A task list under the spinner pushes it up from the bottom.
        let tasks = format!("✶ Twisting… (5m 10s · ↓ 6k tokens)\n{}❯ \n", "  ⎿  ☐ a task\n".repeat(12));
        assert!(agent_at_work(&tasks));
    }
}
