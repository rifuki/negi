//! One paired device's WebSocket: what it may ask for, and how each request is answered.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::fits::After;
use super::{App, VERSION};
use crate::store::Device;
use crate::tmux::link::{Error, Link};
use crate::tmux::{is_id, mux};

/// A socket that has not said who it is by now is closed.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How often an open socket is pinged and its device checked against the store, so a
/// revoked phone stops seeing panes within this long.
const CHECK_EVERY: Duration = Duration::from_secs(15);
const SILENCE_LIMIT: Duration = Duration::from_secs(50);
const MAX_WATCHED: usize = 64;
const MAX_QUEUED: usize = 64;

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
enum In {
    Watch {
        pane: String,
        #[serde(default)]
        open: bool,
    },
    Unwatch {
        pane: String,
    },
    Keys {
        id: u64,
        pane: String,
        keys: Vec<String>,
    },
    Text {
        id: u64,
        pane: String,
        text: String,
        #[serde(default)]
        enter: bool,
    },
    History {
        id: u64,
        pane: String,
        lines: u32,
    },
    NewSession {
        id: u64,
        name: Option<String>,
        cwd: Option<String>,
    },
    NewWindow {
        id: u64,
        session: String,
        name: Option<String>,
        cwd: Option<String>,
    },
    RenameSession {
        id: u64,
        session: String,
        name: String,
    },
    RenameWindow {
        id: u64,
        window: String,
        name: String,
    },
    SelectWindow {
        id: u64,
        window: String,
    },
    KillSession {
        id: u64,
        session: String,
    },
    KillWindow {
        id: u64,
        window: String,
    },
    KillPane {
        id: u64,
        pane: String,
    },
    /// Move to the pane beside this one, as tmux's prefix and an arrow do.
    SelectPane {
        id: u64,
        pane: String,
        dir: String,
    },
    Split {
        id: u64,
        pane: String,
        #[serde(default)]
        across: bool,
    },
    /// A tap on the pane, passed on as a mouse click to a program that takes the mouse.
    Click {
        id: u64,
        pane: String,
        x: u16,
        y: u16,
    },
    /// A drag over a full-screen program, passed on as turns of the mouse wheel.
    Wheel {
        id: u64,
        pane: String,
        x: u16,
        y: u16,
        lines: i16,
    },
    /// Size the pane's window for this phone.
    Fit {
        id: u64,
        pane: String,
        cols: u16,
        rows: u16,
    },
    /// Give the pane's window back to tmux to size.
    Unfit {
        id: u64,
        pane: String,
    },
}

pub(super) async fn session(app: Arc<App>, mut ws: WebSocket) {
    let Some((token, device)) = hello(&app, &mut ws).await else {
        let _ = say(&mut ws, &json!({ "t": "bye", "reason": "unpaired" })).await;
        return;
    };
    if say(&mut ws, &json!({ "t": "ready", "device": device.label, "version": VERSION, "os": std::env::consts::OS, "build": super::build() })).await.is_err() {
        return;
    }
    let Some((client, mut outbox)) = app.hub.join().await else {
        return;
    };
    // Requests run on their own task, one at a time and in order, so a slow write never
    // holds back the screen updates going the other way.
    let (work, mut queue) = mpsc::channel::<In>(MAX_QUEUED);
    let (done, mut replies) = mpsc::channel::<Value>(MAX_QUEUED);
    let worker = tokio::spawn({
        let (app, device) = (app.clone(), device.clone());
        async move {
            let mut watched = HashSet::new();
            while let Some(request) = queue.recv().await {
                if let Some(reply) =
                    handle(&app, &device, client, &mut watched, request).await
                    && done.send(reply).await.is_err()
                {
                    break;
                }
            }
        }
    });
    let mut heard = Instant::now();
    let mut check = tokio::time::interval(CHECK_EVERY);
    check.reset();
    loop {
        tokio::select! {
            out = outbox.recv() => match out {
                Some(text) => {
                    if ws.send(Message::Text(text.as_ref().into())).await.is_err() {
                        break;
                    }
                }
                // The hub dropped this phone for falling behind; it reconnects.
                None => break,
            },
            Some(reply) = replies.recv() => {
                if say(&mut ws, &reply).await.is_err() {
                    break;
                }
            }
            msg = ws.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    heard = Instant::now();
                    let Ok(request) = serde_json::from_str::<In>(&text) else { continue };
                    // A phone with this many requests unanswered is not a phone.
                    if work.try_send(request).is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => heard = Instant::now(),
            },
            _ = check.tick() => {
                if app.store.device(&token).is_none() {
                    let _ = say(&mut ws, &json!({ "t": "bye", "reason": "unpaired" })).await;
                    break;
                }
                if heard.elapsed() >= SILENCE_LIMIT || ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    worker.abort();
    for pane in app.fits.held_by(client) {
        let _ = let_go(&app, &pane, client).await;
    }
    app.hub.leave(client);
}

/// The first message must be `{"t":"hello","token":"..."}`. The token travels in the
/// socket, not in the URL, so it never lands in a log or a history.
async fn hello(app: &App, ws: &mut WebSocket) -> Option<(String, Device)> {
    let first = tokio::time::timeout(HELLO_TIMEOUT, ws.recv())
        .await
        .ok()??
        .ok()?;
    let Message::Text(text) = first else {
        return None;
    };
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("t")?.as_str()? != "hello" {
        return None;
    }
    let token = value.get("token")?.as_str()?.to_owned();
    let device = app.store.device(&token)?;
    Some((token, device))
}

async fn say(ws: &mut WebSocket, value: &Value) -> Result<(), axum::Error> {
    ws.send(Message::Text(value.to_string().into())).await
}

/// `client` stops showing `pane` fitted: the window takes the size of a phone still showing
/// it, or goes back to tmux when none is left.
async fn let_go(app: &App, pane: &str, client: u64) -> Result<Value, Error> {
    match app.fits.release(pane, client) {
        After::Keep => Ok(json!({})),
        After::Size(cols, rows) => {
            with_link(app, async |l| {
                mux::fit(&l, pane, cols, rows).await.map(|()| json!({}))
            })
            .await
        }
        After::Free => {
            with_link(app, async |l| mux::unfit(&l, pane).await.map(|()| json!({})))
                .await
        }
    }
}

async fn handle(
    app: &App,
    device: &Device,
    client: u64,
    watched: &mut HashSet<String>,
    request: In,
) -> Option<Value> {
    let (id, result) = match request {
        In::Watch { pane, open } => {
            if is_id('%', &pane) && (watched.len() < MAX_WATCHED || watched.contains(&pane)) {
                watched.insert(pane.clone());
                app.hub.watch(client, pane, open);
            }
            return None;
        }
        In::Unwatch { pane } => {
            watched.remove(&pane);
            app.hub.unwatch(client, pane);
            return None;
        }
        In::Keys { id, pane, keys } => {
            app.store
                .audit(&device.id, &format!("keys {pane} n={}", keys.len()));
            (
                id,
                with_link(app, async |l| {
                    mux::keys(&l, &pane, &keys).await.map(|()| json!({}))
                })
                .await,
            )
        }
        In::Text {
            id,
            pane,
            text,
            enter,
        } => {
            app.store.audit(
                &device.id,
                &format!("text {pane} bytes={} enter={enter}", text.len()),
            );
            (
                id,
                with_link(app, async |l| {
                    mux::text_in(&l, &pane, &text, enter)
                        .await
                        .map(|()| json!({}))
                })
                .await,
            )
        }
        In::History { id, pane, lines } => (
            id,
            with_link(app, async |l| {
                // Under a full-screen program the history is the shell's from before it
                // started; shown above the program it would read as the program's own.
                if mux::alternate(&l, &pane).await? {
                    return Ok(json!({ "lines": [], "alt": true }));
                }
                mux::history(&l, &pane, lines)
                    .await
                    .map(|h| json!({ "lines": h, "alt": false }))
            })
            .await,
        ),
        In::NewSession { id, name, cwd } => {
            app.store.audit(&device.id, "new-session");
            (id, new_session(app, name.as_deref(), cwd.as_deref()).await)
        }
        In::NewWindow {
            id,
            session,
            name,
            cwd,
        } => {
            app.store
                .audit(&device.id, &format!("new-window {session}"));
            let made = match folder(cwd.as_deref()) {
                Ok(cwd) => {
                    with_link(app, async |l| {
                        mux::new_window(&l, &session, name.as_deref(), cwd.as_deref())
                            .await
                            .map(|p| json!({ "pane": p }))
                    })
                    .await
                }
                Err(e) => Err(e),
            };
            (id, made)
        }
        In::RenameSession { id, session, name } => {
            app.store
                .audit(&device.id, &format!("rename-session {session}"));
            (
                id,
                with_link(app, async |l| {
                    mux::rename_session(&l, &session, &name)
                        .await
                        .map(|()| json!({}))
                })
                .await,
            )
        }
        In::RenameWindow { id, window, name } => {
            app.store
                .audit(&device.id, &format!("rename-window {window}"));
            (
                id,
                with_link(app, async |l| {
                    mux::rename_window(&l, &window, &name)
                        .await
                        .map(|()| json!({}))
                })
                .await,
            )
        }
        In::SelectWindow { id, window } => (
            id,
            with_link(app, async |l| {
                mux::select_window(&l, &window).await.map(|()| json!({}))
            })
            .await,
        ),
        In::KillSession { id, session } => {
            app.store
                .audit(&device.id, &format!("kill-session {session}"));
            (
                id,
                with_link(app, async |l| {
                    mux::kill_session(&l, &session).await.map(|()| json!({}))
                })
                .await,
            )
        }
        In::KillWindow { id, window } => {
            app.store
                .audit(&device.id, &format!("kill-window {window}"));
            (
                id,
                with_link(app, async |l| {
                    mux::kill_window(&l, &window).await.map(|()| json!({}))
                })
                .await,
            )
        }
        In::Wheel {
            id,
            pane,
            x,
            y,
            lines,
        } => (
            id,
            with_link(app, async |l| {
                mux::wheel(&l, &pane, x, y, lines)
                    .await
                    .map(|sent| json!({ "sent": sent }))
            })
            .await,
        ),
        In::Click { id, pane, x, y } => (
            id,
            with_link(app, async |l| {
                mux::click(&l, &pane, x, y)
                    .await
                    .map(|sent| json!({ "sent": sent }))
            })
            .await,
        ),
        In::Fit {
            id,
            pane,
            cols,
            rows,
        } => {
            let result = with_link(app, async |l| {
                mux::fit(&l, &pane, cols, rows).await.map(|()| json!({}))
            })
            .await;
            if result.is_ok() {
                app.fits.hold(&pane, client, cols, rows);
            }
            (id, result)
        }
        In::Unfit { id, pane } => (id, let_go(app, &pane, client).await),
        In::SelectPane { id, pane, dir } => (
            id,
            with_link(app, async |l| {
                mux::select_pane(&l, &pane, &dir)
                    .await
                    .map(|p| json!({ "pane": p }))
            })
            .await,
        ),
        In::Split { id, pane, across } => {
            app.store.audit(&device.id, &format!("split {pane}"));
            (
                id,
                with_link(app, async |l| {
                    mux::split(&l, &pane, across)
                        .await
                        .map(|p| json!({ "pane": p }))
                })
                .await,
            )
        }
        In::KillPane { id, pane } => {
            app.store.audit(&device.id, &format!("kill-pane {pane}"));
            (
                id,
                with_link(app, async |l| {
                    mux::kill_pane(&l, &pane).await.map(|()| json!({}))
                })
                .await,
            )
        }
    };
    app.hub.refresh();
    Some(match result {
        Ok(mut data) => {
            data["t"] = json!("ack");
            data["id"] = json!(id);
            data["ok"] = json!(true);
            data
        }
        Err(e) => json!({ "t": "ack", "id": id, "ok": false, "error": e.to_string() }),
    })
}

async fn with_link<F>(app: &App, run: F) -> Result<Value, Error>
where
    F: AsyncFnOnce(Arc<Link>) -> Result<Value, Error>,
{
    match app.hub.link().await {
        Some(link) => run(link).await,
        None => Err(Error::Closed("tmux is not running".to_owned())),
    }
}

/// With a server running the session is made through a control client. Without one
/// there is nothing to ask, so tmux is started the plain way.
async fn new_session(app: &App, name: Option<&str>, cwd: Option<&str>) -> Result<Value, Error> {
    let cwd = folder(cwd)?;
    if let Some(link) = app.hub.link().await {
        return mux::new_session(&link, name, cwd.as_deref())
            .await
            .map(|p| json!({ "pane": p }));
    }
    let args = mux::new_session_args(name, cwd.as_deref())?;
    let mut command = app.tmux.command();
    command.args(args);
    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }
    let out = tokio::task::spawn_blocking(move || command.output())
        .await
        .map_err(|e| Error::Closed(e.to_string()))?
        .map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Tmux(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(json!({ "pane": String::from_utf8_lossy(&out.stdout).trim() }))
}

/// A folder a phone named: `~` is the home folder, and it must exist.
fn folder(cwd: Option<&str>) -> Result<Option<String>, Error> {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let path = match cwd {
        "~" => home,
        _ => match cwd.strip_prefix("~/") {
            Some(rest) => format!("{home}/{rest}"),
            None => cwd.to_owned(),
        },
    };
    if !path.starts_with('/') || !std::path::Path::new(&path).is_dir() {
        return Err(Error::Invalid("that folder does not exist"));
    }
    Ok(Some(path))
}
