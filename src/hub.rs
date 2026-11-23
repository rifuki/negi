//! The one task that owns the control clients and decides what each phone is told: one
//! client per session while a phone is connected, tmux read only when a notification says
//! something changed, and nothing at all with no phone.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::awake::Awake;
use crate::tmux::Tmux;
use crate::tmux::link::{Error, Link};
use crate::tmux::mux::{self, Screen, Session};
use crate::tmux::proto::Notice;
use crate::tmux::screen::{self, Delta};

const TICK: Duration = Duration::from_millis(40);
/// Shortest time between two captures of a pane someone has open.
const OPEN_GAP: Duration = Duration::from_millis(50);
/// A program redraws its screen in several writes; an open pane is read once its output has
/// been quiet this long, so a half-drawn frame is not what the phone gets.
const OPEN_QUIET: Duration = Duration::from_millis(16);
/// A pane whose output never pauses is read anyway after this long.
const OPEN_WAIT: Duration = Duration::from_millis(150);
/// The same for a pane shown only as a preview.
const PREVIEW_GAP: Duration = Duration::from_secs(1);
/// An open pane is read again this often even without a notification, in case one was missed.
const OPEN_BACKSTOP: Duration = Duration::from_secs(5);
/// Tree notifications arrive in bursts; wait for the burst to end.
const TREE_SETTLE: Duration = Duration::from_millis(120);
/// A pane's program and folder change without a notification.
const TREE_BACKSTOP: Duration = Duration::from_secs(10);
const BOOT_RETRY: Duration = Duration::from_secs(3);
/// A pane counts as busy until it has printed nothing for this long.
const QUIET: Duration = Duration::from_millis(2500);
const MAX_LINKS: usize = 24;
/// Every control client hears of the same new buffer; within this long it is one copy.
const COPY_ECHO: Duration = Duration::from_millis(500);
const CLIENT_QUEUE: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How long the control clients outlive the last phone, so a reconnect is instant.
    pub grace: Duration,
    /// After a control client attaches, idle agents redraw for a moment; output in this
    /// window is not counted as work.
    pub attach_redraw: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(10),
            attach_redraw: Duration::from_millis(1500),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub clients: usize,
    pub links: usize,
    pub panes: usize,
}

pub type Outbox = mpsc::Receiver<Arc<str>>;

enum Cmd {
    Join(oneshot::Sender<(u64, Outbox)>),
    Leave(u64),
    Watch {
        client: u64,
        pane: String,
        open: bool,
    },
    Unwatch {
        client: u64,
        pane: String,
    },
    Link(oneshot::Sender<Option<Arc<Link>>>),
    Refresh,
    Stats(oneshot::Sender<Stats>),
}

#[derive(Clone)]
pub struct Hub {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Hub {
    pub fn start(tmux: Tmux, timing: Timing) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Actor::new(tmux, timing).run(rx));
        Self { tx }
    }

    /// Registers a phone. Everything the hub has to say arrives on the outbox, as JSON
    /// text; the outbox closing means the phone fell too far behind and should reconnect.
    pub async fn join(&self) -> Option<(u64, Outbox)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Join(tx)).ok()?;
        rx.await.ok()
    }

    pub fn leave(&self, client: u64) {
        let _ = self.tx.send(Cmd::Leave(client));
    }

    /// Starts sending a pane's screen to a phone. `open` is the pane the phone is looking
    /// at; a preview is refreshed far less often.
    pub fn watch(&self, client: u64, pane: String, open: bool) {
        let _ = self.tx.send(Cmd::Watch { client, pane, open });
    }

    pub fn unwatch(&self, client: u64, pane: String) {
        let _ = self.tx.send(Cmd::Unwatch { client, pane });
    }

    /// A live control client to send a command through, if tmux is running.
    pub async fn link(&self) -> Option<Arc<Link>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Link(tx)).ok()?;
        rx.await.ok().flatten()
    }

    /// Asks for the tree to be read again, after a change the caller made.
    pub fn refresh(&self) {
        let _ = self.tx.send(Cmd::Refresh);
    }

    pub async fn stats(&self) -> Option<Stats> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Stats(tx)).ok()?;
        rx.await.ok()
    }
}

struct Client {
    tx: mpsc::Sender<Arc<str>>,
    /// Pane id, and whether it is the open pane.
    watching: HashMap<String, bool>,
}

struct Watched {
    last: Option<Screen>,
    dirty: bool,
    captured: Instant,
    /// When output last arrived, and when the pane first changed since it was last read.
    heard: Instant,
    since: Instant,
}

struct Actor {
    tmux: Tmux,
    timing: Timing,
    notices_tx: mpsc::UnboundedSender<(u64, Notice)>,
    notices_rx: mpsc::UnboundedReceiver<(u64, Notice)>,
    /// By session id.
    links: HashMap<String, Arc<Link>>,
    clients: HashMap<u64, Client>,
    next_client: u64,
    panes: HashMap<String, Watched>,
    tree: Vec<Session>,
    /// What phones were last told, to send again only on change.
    told: Option<Arc<str>>,
    tree_dirty: Option<Instant>,
    tree_read: Instant,
    busy: HashMap<String, Instant>,
    empty_since: Option<Instant>,
    /// Held while any phone is connected.
    awake: Option<Awake>,
    /// When each link attached, by its id, for [`Timing::attach_redraw`].
    attached: HashMap<u64, Instant>,
    booted: Option<Instant>,
    /// The buffer last passed on, and when.
    copied: Option<(String, Instant)>,
}

impl Actor {
    fn new(tmux: Tmux, timing: Timing) -> Self {
        let (notices_tx, notices_rx) = mpsc::unbounded_channel();
        Self {
            tmux,
            timing,
            notices_tx,
            notices_rx,
            links: HashMap::new(),
            clients: HashMap::new(),
            next_client: 1,
            panes: HashMap::new(),
            tree: Vec::new(),
            told: None,
            tree_dirty: None,
            tree_read: Instant::now(),
            busy: HashMap::new(),
            empty_since: Some(Instant::now()),
            awake: None,
            attached: HashMap::new(),
            booted: None,
            copied: None,
        }
    }

    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            // With no phone and no control client there is nothing to time.
            let ticking = !self.clients.is_empty() || !self.links.is_empty();
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => self.command(cmd).await,
                    None => return,
                },
                Some((link, notice)) = self.notices_rx.recv() => self.notice(link, notice).await,
                _ = tick.tick(), if ticking => self.tick().await,
            }
        }
    }

    async fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Join(reply) => {
                let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
                let id = self.next_client;
                self.next_client += 1;
                // Built now, not taken from what the others were last told: which panes
                // are busy has moved on since, and a pane that stays busy sends no news.
                if self.told.is_some() {
                    let _ = tx.try_send(self.tree_message(!self.links.is_empty()));
                }
                self.clients.insert(
                    id,
                    Client {
                        tx,
                        watching: HashMap::new(),
                    },
                );
                self.empty_since = None;
                if self.awake.is_none() {
                    self.awake = Awake::hold();
                }
                if reply.send((id, rx)).is_err() {
                    self.drop_client(id);
                    return;
                }
                if self.links.is_empty() {
                    self.boot().await;
                }
            }
            Cmd::Leave(id) => self.drop_client(id),
            Cmd::Watch { client, pane, open } => {
                let Some(c) = self.clients.get_mut(&client) else {
                    return;
                };
                c.watching.insert(pane.clone(), open);
                let now = Instant::now();
                let watched = self.panes.entry(pane.clone()).or_insert_with(|| Watched {
                    last: None,
                    dirty: true,
                    // Due at once, whichever gap applies.
                    captured: now.checked_sub(PREVIEW_GAP).unwrap_or(now),
                    heard: now.checked_sub(OPEN_WAIT).unwrap_or(now),
                    since: now.checked_sub(OPEN_WAIT).unwrap_or(now),
                });
                // A late joiner starts from the screen the others already have, so the
                // row updates that follow apply to all of them alike.
                let known = watched
                    .last
                    .as_ref()
                    .map(|screen| text(&screen::full_message(&pane, screen)));
                if let Some(msg) = known {
                    self.send(&[client], msg);
                }
            }
            Cmd::Unwatch { client, pane } => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.watching.remove(&pane);
                }
                self.forget_unwatched();
            }
            Cmd::Link(reply) => {
                if self.live_link().is_none() {
                    self.boot().await;
                }
                let _ = reply.send(self.live_link());
            }
            Cmd::Refresh => {
                let now = Instant::now();
                self.tree_dirty = Some(now.checked_sub(TREE_SETTLE).unwrap_or(now));
                // The caller may have just started tmux; do not wait for the next retry.
                if self.live_link().is_none() && !self.clients.is_empty() {
                    self.boot().await;
                }
            }
            Cmd::Stats(reply) => {
                let _ = reply.send(Stats {
                    clients: self.clients.len(),
                    links: self.links.len(),
                    panes: self.panes.len(),
                });
            }
        }
    }

    async fn notice(&mut self, link: u64, notice: Notice) {
        match notice {
            Notice::Output(pane) => {
                let redraw = self.attached.get(&link).is_some_and(|at| at.elapsed() < self.timing.attach_redraw);
                if !redraw && self.busy.insert(pane.clone(), Instant::now()).is_none() {
                    self.broadcast(text(&json!({ "t": "busy", "pane": pane, "on": true })));
                }
                if let Some(watched) = self.panes.get_mut(&pane) {
                    let now = Instant::now();
                    if !watched.dirty {
                        watched.since = now;
                    }
                    watched.heard = now;
                    watched.dirty = true;
                }
            }
            Notice::Tree => {
                self.tree_dirty.get_or_insert_with(Instant::now);
            }
            Notice::Moved(session) => {
                // tmux moved the client because its session closed. Two clients on one
                // session would report every output twice, so this one goes.
                let stray = self
                    .links
                    .iter()
                    .find(|(s, l)| l.id() == link && **s != session)
                    .map(|(s, _)| s.clone());
                if let Some(stray) = stray {
                    self.links.remove(&stray);
                    self.tree_dirty.get_or_insert_with(Instant::now);
                }
            }
            Notice::Buffer(name) => {
                if self.copied.as_ref().is_some_and(|(n, at)| *n == name && at.elapsed() < COPY_ECHO) {
                    return;
                }
                self.copied = Some((name.clone(), Instant::now()));
                let Some(link) = self.live_link().filter(|_| !self.clients.is_empty()) else {
                    return;
                };
                if let Ok(Some(copied)) = mux::buffer(&link, &name).await {
                    self.broadcast(text(&json!({ "t": "copied", "text": copied })));
                }
            }
            Notice::Exit => {
                self.links.retain(|_, l| l.id() != link);
                self.tree_dirty.get_or_insert_with(Instant::now);
            }
        }
    }

    async fn tick(&mut self) {
        if self.clients.is_empty() {
            if self
                .empty_since
                .is_none_or(|t| t.elapsed() >= self.timing.grace)
            {
                self.links.clear();
                self.panes.clear();
                self.busy.clear();
                self.told = None;
                self.tree.clear();
                self.booted = None;
            }
            return;
        }
        if self.live_link().is_none() {
            if self.booted.is_none_or(|t| t.elapsed() >= BOOT_RETRY) {
                self.boot().await;
            }
            return;
        }
        let settled = self.tree_dirty.is_some_and(|t| t.elapsed() >= TREE_SETTLE);
        if settled || self.tree_read.elapsed() >= TREE_BACKSTOP {
            self.read_tree().await;
        }

        let quiet: Vec<String> = self
            .busy
            .iter()
            .filter(|(_, at)| at.elapsed() >= QUIET)
            .map(|(p, _)| p.clone())
            .collect();
        // Quiet is not done while an agent's status line still shows (mux::agent_at_work).
        let still = match (quiet.is_empty(), self.live_link()) {
            (false, Some(link)) => mux::working(&link, &quiet).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        for pane in quiet {
            if still.contains(&pane) {
                self.busy.insert(pane, Instant::now());
                continue;
            }
            self.busy.remove(&pane);
            self.broadcast(text(&json!({ "t": "busy", "pane": pane, "on": false })));
        }

        let open: HashSet<&String> = self
            .clients
            .values()
            .flat_map(|c| c.watching.iter().filter(|(_, open)| **open).map(|(p, _)| p))
            .collect();
        let due: Vec<String> = self
            .panes
            .iter()
            .filter(|(pane, w)| {
                let age = w.captured.elapsed();
                if open.contains(pane) {
                    let settled = w.heard.elapsed() >= OPEN_QUIET || w.since.elapsed() >= OPEN_WAIT;
                    (w.dirty && age >= OPEN_GAP && settled) || age >= OPEN_BACKSTOP
                } else {
                    w.dirty && age >= PREVIEW_GAP
                }
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        for pane in due {
            self.capture(&pane).await;
        }
    }

    /// Attaches the first control client, to whichever session tmux picks.
    async fn boot(&mut self) {
        self.booted = Some(Instant::now());
        let attached = async {
            let link = Link::attach(&self.tmux, None, self.notices_tx.clone())?;
            let session = mux::session_of(&link).await?;
            Ok::<_, Error>((session, link))
        };
        match attached.await {
            Ok((session, link)) => {
                self.attached.insert(link.id(), Instant::now());
                self.links.insert(session, Arc::new(link));
                self.read_tree().await;
            }
            Err(_) => {
                self.links.clear();
                self.tree.clear();
                self.tell_tree(false);
            }
        }
    }

    async fn read_tree(&mut self) {
        let Some(link) = self.live_link() else { return };
        self.tree_read = Instant::now();
        let tree = match mux::tree(&link).await {
            Ok(tree) => tree,
            Err(_) => {
                self.links.retain(|_, l| l.alive());
                return;
            }
        };
        self.tree_dirty = None;

        let sessions: HashSet<&str> = tree.iter().map(|s| s.id.as_str()).collect();
        self.links
            .retain(|session, link| sessions.contains(session.as_str()) && link.alive());
        for session in &tree {
            if self.links.len() >= MAX_LINKS {
                break;
            }
            if !self.links.contains_key(&session.id)
                && let Ok(link) =
                    Link::attach(&self.tmux, Some(&session.id), self.notices_tx.clone())
            {
                self.attached.insert(link.id(), Instant::now());
                self.links.insert(session.id.clone(), Arc::new(link));
            }
        }
        let window = self.timing.attach_redraw;
        self.attached.retain(|_, at| at.elapsed() < window);

        let alive: HashSet<&str> = tree
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .map(|p| p.id.as_str())
            .collect();
        self.busy.retain(|pane, _| alive.contains(pane.as_str()));
        // An agent at work but silent since this server began to listen shows on its screen.
        let silent: Vec<String> = alive.iter().filter(|p| !self.busy.contains_key(**p)).map(|p| p.to_string()).collect();
        if let Ok(found) = mux::working(&link, &silent).await {
            for pane in found {
                self.busy.insert(pane, Instant::now());
            }
        }
        let closed: Vec<String> = self
            .panes
            .keys()
            .filter(|p| !alive.contains(p.as_str()))
            .cloned()
            .collect();
        self.tree = tree;
        for pane in closed {
            self.gone(&pane);
        }
        self.tell_tree(true);
    }

    /// Tells every phone the tree, unless it is what they already have.
    fn tell_tree(&mut self, server: bool) {
        let msg = self.tree_message(server);
        if self.told.as_ref() != Some(&msg) {
            self.told = Some(msg.clone());
            self.broadcast(msg);
        }
    }

    fn tree_message(&self, server: bool) -> Arc<str> {
        let mut busy: Vec<&String> = self.busy.keys().collect();
        busy.sort();
        text(&json!({ "t": "tree", "server": server, "sessions": self.tree, "busy": busy }))
    }

    async fn capture(&mut self, pane: &str) {
        let Some(link) = self.live_link() else { return };
        let result = mux::screen(&link, pane).await;
        let Some(watched) = self.panes.get_mut(pane) else {
            return;
        };
        watched.captured = Instant::now();
        let new = match result {
            Ok(screen) => screen,
            Err(e) if e.is_gone() => return self.gone(pane),
            // tmux restarting, a timeout: the backstop tries again.
            Err(_) => return,
        };
        watched.dirty = false;
        let msg = match &watched.last {
            None => Some(screen::full_message(pane, &new)),
            Some(old) => match screen::delta(old, &new) {
                None => None,
                Some(Delta::Full) => Some(screen::full_message(pane, &new)),
                Some(Delta::Rows { scroll, set }) => {
                    Some(screen::rows_message(pane, &new, scroll, &set))
                }
            },
        };
        watched.last = Some(new);
        if let Some(msg) = msg {
            let watchers = self.watchers(pane);
            self.send(&watchers, text(&msg));
        }
    }

    fn gone(&mut self, pane: &str) {
        let watchers = self.watchers(pane);
        self.send(&watchers, text(&json!({ "t": "gone", "pane": pane })));
        for client in self.clients.values_mut() {
            client.watching.remove(pane);
        }
        self.panes.remove(pane);
    }

    fn watchers(&self, pane: &str) -> Vec<u64> {
        self.clients
            .iter()
            .filter(|(_, c)| c.watching.contains_key(pane))
            .map(|(id, _)| *id)
            .collect()
    }

    fn live_link(&self) -> Option<Arc<Link>> {
        self.links.values().find(|l| l.alive()).cloned()
    }

    fn broadcast(&mut self, msg: Arc<str>) {
        let all: Vec<u64> = self.clients.keys().copied().collect();
        self.send(&all, msg);
    }

    /// A phone whose queue is full has stopped reading. It is dropped; its socket sees
    /// the outbox close and reconnects to a fresh, whole state.
    fn send(&mut self, to: &[u64], msg: Arc<str>) {
        let stuck: Vec<u64> = to
            .iter()
            .filter(|id| {
                self.clients
                    .get(id)
                    .is_some_and(|c| c.tx.try_send(msg.clone()).is_err())
            })
            .copied()
            .collect();
        for id in stuck {
            self.drop_client(id);
        }
    }

    fn drop_client(&mut self, id: u64) {
        self.clients.remove(&id);
        self.forget_unwatched();
        if self.clients.is_empty() {
            self.empty_since.get_or_insert_with(Instant::now);
            self.awake = None;
        }
    }

    fn forget_unwatched(&mut self) {
        let watched: HashSet<&String> = self
            .clients
            .values()
            .flat_map(|c| c.watching.keys())
            .collect();
        self.panes.retain(|pane, _| watched.contains(pane));
    }
}

fn text(value: &serde_json::Value) -> Arc<str> {
    Arc::from(value.to_string())
}
