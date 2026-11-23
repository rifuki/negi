//! The hub against a real tmux: what a phone is told, and when.

mod common;

use std::time::Duration;

use common::Scratch;
use negi::hub::{Hub, Outbox, Stats, Timing};
use negi::tmux::mux;
use serde_json::Value;

const QUICK: Timing = Timing {
    grace: Duration::from_millis(150),
    attach_redraw: Duration::ZERO,
};

/// The next message of kind `t` for which `pick` holds; everything before it is skipped.
async fn next(outbox: &mut Outbox, t: &str, pick: impl Fn(&Value) -> bool) -> Value {
    let found = async {
        loop {
            let text = outbox.recv().await.expect("the hub keeps this phone");
            let msg: Value = serde_json::from_str(&text).unwrap();
            if msg["t"] == t && pick(&msg) {
                return msg;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(8), found)
        .await
        .unwrap_or_else(|_| panic!("no {t} message"))
}

fn names(tree: &Value) -> Vec<String> {
    tree["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect()
}

async fn stats_become(hub: &Hub, want: impl Fn(&Stats) -> bool) -> Stats {
    for _ in 0..100 {
        let stats = hub.stats().await.unwrap();
        if want(&stats) {
            return stats;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stats never matched: {:?}", hub.stats().await);
}

#[tokio::test]
async fn a_phone_gets_the_tree_then_rows_as_they_change() {
    let s = Scratch::new();
    let pane = s.session("work", "cat");
    s.session("other", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (client, mut outbox) = hub.join().await.unwrap();

    let tree = next(&mut outbox, "tree", |_| true).await;
    assert_eq!(tree["server"], true);
    assert_eq!(names(&tree), ["other", "work"]);
    assert_eq!(
        stats_become(&hub, |st| st.links == 2).await.links,
        2,
        "one control client per session"
    );

    hub.watch(client, pane.clone(), true);
    let screen = next(&mut outbox, "screen", |m| m["pane"] == pane.as_str()).await;
    assert_eq!(screen["rows"], 24);
    assert_eq!(screen["lines"].as_array().unwrap().len(), 24);

    let link = hub.link().await.unwrap();
    mux::text_in(&link, &pane, "hello phone", true)
        .await
        .unwrap();
    // The notification itself marks the pane busy, before anything is read from tmux.
    next(&mut outbox, "busy", |m| {
        m["pane"] == pane.as_str() && m["on"] == true
    })
    .await;
    let rows = next(&mut outbox, "rows", |m| {
        m["set"].to_string().contains("hello phone")
    })
    .await;
    assert_eq!(rows["pane"], pane.as_str());
    assert!(
        rows["set"].as_array().unwrap().len() <= 2,
        "only the rows that changed: {rows}"
    );
    next(&mut outbox, "busy", |m| {
        m["pane"] == pane.as_str() && m["on"] == false
    })
    .await;
}

#[tokio::test]
async fn panes_of_every_session_report_not_only_the_first() {
    let s = Scratch::new();
    s.session("a", "cat");
    let far = s.session("b", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (client, mut outbox) = hub.join().await.unwrap();
    next(&mut outbox, "tree", |_| true).await;
    stats_become(&hub, |st| st.links == 2).await;

    hub.watch(client, far.clone(), false);
    next(&mut outbox, "screen", |m| m["pane"] == far.as_str()).await;
    let link = hub.link().await.unwrap();
    mux::text_in(&link, &far, "from the other session", true)
        .await
        .unwrap();
    next(&mut outbox, "rows", |m| {
        m["set"].to_string().contains("from the other session")
    })
    .await;
}

#[tokio::test]
async fn a_late_phone_starts_from_the_same_screen() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (first, mut one) = hub.join().await.unwrap();
    hub.watch(first, pane.clone(), true);
    next(&mut one, "screen", |_| true).await;
    let link = hub.link().await.unwrap();
    mux::text_in(&link, &pane, "before", true).await.unwrap();
    next(&mut one, "rows", |m| {
        m["set"].to_string().contains("before")
    })
    .await;

    let (second, mut two) = hub.join().await.unwrap();
    hub.watch(second, pane.clone(), true);
    let screen = next(&mut two, "screen", |_| true).await;
    assert!(screen["lines"].to_string().contains("before"), "{screen}");

    mux::text_in(&link, &pane, "after", true).await.unwrap();
    let a = next(&mut one, "rows", |m| m["set"].to_string().contains("after")).await;
    let b = next(&mut two, "rows", |m| m["set"].to_string().contains("after")).await;
    assert_eq!(a, b);
}

#[tokio::test]
async fn the_tree_follows_tmux_and_closed_panes_are_announced() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (client, mut outbox) = hub.join().await.unwrap();
    next(&mut outbox, "tree", |_| true).await;
    hub.watch(client, pane.clone(), true);
    next(&mut outbox, "screen", |_| true).await;

    // Made outside negi, as the owner would at the keyboard.
    s.session("later", "cat");
    let tree = next(&mut outbox, "tree", |m| names(m).len() == 2).await;
    assert_eq!(names(&tree), ["a", "later"]);
    stats_become(&hub, |st| st.links == 2).await;

    s.run(&["kill-session", "-t", "a"]);
    let gone = next(&mut outbox, "gone", |_| true).await;
    assert_eq!(gone["pane"], pane.as_str());
    let tree = next(&mut outbox, "tree", |m| names(m).len() == 1).await;
    assert_eq!(names(&tree), ["later"]);
    let stats = stats_become(&hub, |st| st.links == 1 && st.panes == 0).await;
    assert_eq!(stats.clients, 1);
}

#[tokio::test]
async fn with_no_phone_the_hub_holds_nothing() {
    let s = Scratch::new();
    s.session("a", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    assert_eq!(
        hub.stats().await.unwrap(),
        Stats {
            clients: 0,
            links: 0,
            panes: 0
        }
    );

    let (client, mut outbox) = hub.join().await.unwrap();
    next(&mut outbox, "tree", |_| true).await;
    assert_eq!(hub.stats().await.unwrap().links, 1);
    let control = |s: &Scratch| {
        s.run(&["list-clients", "-F", "#{client_control_mode}"])
            .lines()
            .count()
    };
    assert_eq!(control(&s), 1);

    hub.leave(client);
    stats_become(&hub, |st| st.links == 0).await;
    for _ in 0..50 {
        if control(&s) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a control client outlived the last phone");
}

#[tokio::test]
async fn tmux_not_running_is_a_state_not_an_error() {
    let s = Scratch::new();
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (_client, mut outbox) = hub.join().await.unwrap();
    let tree = next(&mut outbox, "tree", |_| true).await;
    assert_eq!(tree["server"], false);
    assert!(names(&tree).is_empty());
    assert!(hub.link().await.is_none());

    s.session("started", "cat");
    let tree = next(&mut outbox, "tree", |m| m["server"] == true).await;
    assert_eq!(names(&tree), ["started"]);
}

#[tokio::test]
async fn a_phone_that_joins_late_learns_which_panes_are_busy() {
    let s = Scratch::new();
    let pane = s.session("a", "while true; do echo tick; sleep 0.2; done");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (_first, mut one) = hub.join().await.unwrap();
    next(&mut one, "busy", |m| {
        m["pane"] == pane.as_str() && m["on"] == true
    })
    .await;

    // The pane never goes quiet, so no further busy message will come.
    let (_second, mut two) = hub.join().await.unwrap();
    let tree = next(&mut two, "tree", |_| true).await;
    assert_eq!(tree["busy"], serde_json::json!([pane]), "{tree}");
}

#[tokio::test]
async fn a_window_made_after_joining_reports_its_output() {
    let s = Scratch::new();
    s.session("a", "cat");
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (client, mut outbox) = hub.join().await.unwrap();
    let tree = next(&mut outbox, "tree", |_| true).await;
    let session = tree["sessions"][0]["id"].as_str().unwrap().to_owned();
    let link = hub.link().await.unwrap();

    mux::rename_session(&link, &session, "renamed")
        .await
        .unwrap();
    let pane = mux::new_window(&link, &session, None, None).await.unwrap();
    hub.refresh();
    hub.watch(client, pane.clone(), true);
    next(&mut outbox, "screen", |m| m["pane"] == pane.as_str()).await;

    mux::text_in(&link, &pane, "echo in-the-new-window", true)
        .await
        .unwrap();
    let seen =
        |m: &Value| m["pane"] == pane.as_str() && m.to_string().contains("in-the-new-window");
    let found = async {
        loop {
            let text = outbox.recv().await.unwrap();
            let msg: Value = serde_json::from_str(&text).unwrap();
            if (msg["t"] == "rows" || msg["t"] == "screen") && seen(&msg) {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(8), found)
        .await
        .expect("the new window's output reached the phone");
}

#[tokio::test]
async fn a_copy_in_a_pane_reaches_the_phone_once() {
    let s = Scratch::new();
    let pane = s.session("work", "sh");
    s.session("other", "cat");
    s.run(&["set-option", "-s", "set-clipboard", "on"]);
    let hub = Hub::start(s.tmux.clone(), QUICK);
    let (_client, mut outbox) = hub.join().await.unwrap();
    next(&mut outbox, "tree", |_| true).await;
    stats_become(&hub, |st| st.links == 2).await;

    // What nvim sends on a yank over OSC 52: "yanked" in base64.
    let link = hub.link().await.unwrap();
    mux::text_in(&link, &pane, r"printf '\033]52;c;eWFua2Vk\a'", true)
        .await
        .unwrap();
    let copied = next(&mut outbox, "copied", |_| true).await;
    assert_eq!(copied["text"], "yanked");

    s.run(&["set-buffer", "second"]);
    let copied = next(&mut outbox, "copied", |_| true).await;
    assert_eq!(copied["text"], "second", "each control client hears of it, the phone once");
}
