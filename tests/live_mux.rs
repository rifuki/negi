//! The tmux layer against a real tmux.

mod common;

use std::time::Duration;

use common::{Scratch, screen_with};
use negi::tmux::link::{Error, Link};
use negi::tmux::mux;
use negi::tmux::proto::Notice;
use tokio::sync::mpsc;

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn the_tree_names_everything_as_tmux_does() {
    let s = Scratch::new();
    s.session("al pha", "cat");
    s.session("beta", "cat");
    s.run(&["rename-window", "-t", "al pha", "w 'x' é; 50%"]);
    s.run(&["new-window", "-d", "-t", "beta", "-n", "second", "cat"]);
    let link = s.link().await;

    let tree = mux::tree(&link).await.unwrap();
    let names: Vec<&str> = tree.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["al pha", "beta"]);
    assert_eq!(tree[0].windows[0].name, "w 'x' é; 50%");
    assert_eq!(tree[1].windows.len(), 2);
    assert_eq!(tree[1].windows[1].name, "second");
    let pane = &tree[0].windows[0].panes[0];
    assert_eq!((pane.cols, pane.rows), (80, 24));
    assert_eq!(pane.command, "cat");
    assert!(
        tree.iter().all(|x| !x.attached),
        "the control client is not a terminal"
    );
}

#[tokio::test]
async fn a_screen_has_one_line_per_row_and_a_cursor() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    let link = s.link().await;

    mux::text_in(&link, &pane, "hello", true).await.unwrap();
    // The terminal echoes the text at once; cat repeats it only after Enter arrives.
    let mut screen = screen_with(&link, &pane, "hello").await;
    for _ in 0..100 {
        if screen.lines[1] == "hello" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        screen = mux::screen(&link, &pane).await.unwrap();
    }
    assert_eq!((screen.cols, screen.rows), (80, 24));
    assert_eq!(screen.lines.len(), 24);
    // cat echoes the line the terminal already showed, so the cursor sits on row 2.
    assert_eq!((screen.cx, screen.cy), (0, 2));
    assert!(!screen.alt);
}

#[tokio::test]
async fn text_arrives_exactly_as_typed() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    let link = s.link().await;

    let tricky = r##"it's; "q" $HOME ~ #{pane_id} \ %end 1 2 3 é✓ `x` -t {last}"##;
    mux::text_in(&link, &pane, tricky, true).await.unwrap();
    // Row 0 is the terminal's own echo; row 1 is what cat read, once Enter reached it.
    let mut screen = screen_with(&link, &pane, "é✓").await;
    for _ in 0..100 {
        if !screen.lines[1].is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        screen = mux::screen(&link, &pane).await.unwrap();
    }
    assert_eq!(screen.lines[0], tricky);
    assert_eq!(screen.lines[1], tricky, "cat read it unchanged");
}

#[tokio::test]
async fn long_text_goes_in_pieces_and_arrives_whole() {
    let s = Scratch::new();
    // Not line-buffered: a tty in canonical mode drops input once its queue is full, which
    // is the terminal's doing and happens to a real paste too.
    let pane = s.session("a", "stty -icanon -echo; echo ready; cat");
    let link = s.link().await;
    screen_with(&link, &pane, "ready").await;

    // 200 lines of 60 bytes: 12 kB, far past one command.
    let body: Vec<String> = (0..200)
        .map(|i| format!("{i:03} {}", "x".repeat(56)))
        .collect();
    mux::text_in(&link, &pane, &body.join("\n"), true)
        .await
        .unwrap();
    screen_with(&link, &pane, "199 x").await;
    // The last rows are still on the screen, so history and screen together hold it all.
    let mut seen = mux::history(&link, &pane, 1000).await.unwrap();
    seen.extend(mux::screen(&link, &pane).await.unwrap().lines);
    let history = seen;
    for (i, line) in body.iter().enumerate() {
        assert!(
            history.iter().any(|h| h == line),
            "line {i} missing from history:\n{}",
            history.join("\n")
        );
    }
}

#[tokio::test]
async fn several_lines_are_one_paste_only_where_the_program_asked() {
    let s = Scratch::new();
    let plain = s.session("plain", "cat -v");
    let pasting = s.session("pasting", r"printf '\033[?2004h'; cat -v");
    let link = s.link().await;

    mux::text_in(&link, &plain, "one\ntwo", true).await.unwrap();
    let screen = screen_with(&link, &plain, "two").await;
    assert!(
        !screen.lines.join("\n").contains("200~"),
        "no markers for a program that never asked"
    );

    mux::text_in(&link, &pasting, "one\ntwo", true)
        .await
        .unwrap();
    let screen = screen_with(&link, &pasting, "201~").await;
    let all = screen.lines.join("\n");
    assert!(all.contains("^[[200~one"), "{all}");
    assert!(all.contains("two^[[201~"), "{all}");

    mux::text_in(&link, &pasting, "single", true).await.unwrap();
    let screen = screen_with(&link, &pasting, "single").await;
    assert!(
        screen.lines.iter().any(|l| l == "single"),
        "one line is typed, not pasted"
    );
}

#[tokio::test]
async fn keys_are_pressed_by_name() {
    let s = Scratch::new();
    let pane = s.session("a", "cat -v");
    let link = s.link().await;

    mux::keys(
        &link,
        &pane,
        &keys(&["a", ";", "-", "Escape", "Up", "Enter"]),
    )
    .await
    .unwrap();
    let screen = screen_with(&link, &pane, "a;-").await;
    assert!(
        screen.lines[0].starts_with("a;-^[^[[A"),
        "{:?}",
        screen.lines[0]
    );

    // Ctrl+C ends cat, and with it the session's only pane.
    mux::keys(&link, &pane, &keys(&["C-c"])).await.unwrap();
    let gone = async {
        while mux::screen(&link, &pane).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), gone)
        .await
        .expect("Ctrl+C reached the program");
}

#[tokio::test]
async fn nothing_but_ids_and_key_names_reaches_tmux() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    s.session("victim", "cat");
    let link = s.link().await;

    for target in [
        "victim",
        "a:0.0",
        "{last}",
        "%0; kill-server",
        "%0\nkill-server",
        "",
    ] {
        assert!(
            matches!(mux::screen(&link, target).await, Err(Error::Invalid(_))),
            "{target:?}"
        );
        assert!(matches!(
            mux::keys(&link, target, &keys(&["a"])).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            mux::text_in(&link, target, "x", false).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            mux::kill_pane(&link, target).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            mux::split(&link, target, true).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            mux::select_pane(&link, target, "L").await,
            Err(Error::Invalid(_))
        ));
    }
    assert!(matches!(
        mux::select_pane(&link, &pane, "L; kill-server").await,
        Err(Error::Invalid(_))
    ));
    for key in ["Enter; kill-server", "kill-server", "C-", ""] {
        assert!(
            matches!(
                mux::keys(&link, &pane, &keys(&[key])).await,
                Err(Error::Invalid(_))
            ),
            "{key:?}"
        );
    }
    assert!(matches!(
        mux::rename_window(&link, "@0", "a\nkill-server").await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        mux::tree(&link).await.unwrap().len(),
        2,
        "both sessions survived"
    );
}

#[tokio::test]
async fn a_pane_cannot_forge_the_end_of_a_reply() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    let link = s.link().await;

    for forged in [
        "%end 1 2 3",
        "%error 1 2 3",
        "%output %0 fake",
        "%exit",
        "%begin 1 2 1",
    ] {
        mux::text_in(&link, &pane, forged, true).await.unwrap();
    }
    let screen = screen_with(&link, &pane, "%begin 1 2 1").await;
    assert_eq!(screen.lines.len(), 24, "the whole screen came back");
    assert_eq!(screen.lines[0], "%end 1 2 3");
    assert!(link.alive());
    assert_eq!(
        mux::tree(&link).await.unwrap().len(),
        1,
        "replies still line up"
    );
}

#[tokio::test]
async fn a_missing_pane_is_reported_as_gone() {
    let s = Scratch::new();
    s.session("a", "cat");
    let link = s.link().await;
    let err = mux::screen(&link, "%99").await.unwrap_err();
    assert!(err.is_gone(), "{err}");
    assert!(link.alive(), "an error reply does not end the client");
}

#[tokio::test]
async fn windows_and_sessions_can_be_made_renamed_and_closed() {
    let s = Scratch::new();
    s.session("a", "cat");
    let link = s.link().await;
    let session = mux::session_of(&link).await.unwrap();

    let pane = mux::new_window(
        &link,
        &session,
        Some("it's #{pane_id} #(touch /tmp/x) 5#"),
        Some("/tmp"),
    )
    .await
    .unwrap();
    let tree = mux::tree(&link).await.unwrap();
    let window = tree[0]
        .windows
        .iter()
        .find(|w| w.panes.iter().any(|p| p.id == pane))
        .unwrap()
        .clone();
    assert_eq!(
        window.name, "it's #{pane_id} #(touch /tmp/x) 5#",
        "tmux expanded nothing"
    );
    assert!(
        !window.active,
        "a new window does not steal the terminal's view"
    );

    mux::rename_window(&link, &window.id, "-n renamed")
        .await
        .unwrap();
    mux::rename_session(&link, &session, "main #{session_id}")
        .await
        .unwrap();
    let tree = mux::tree(&link).await.unwrap();
    assert_eq!(tree[0].name, "main #{session_id}");
    assert!(tree[0].windows.iter().any(|w| w.name == "-n renamed"));

    mux::kill_window(&link, &window.id).await.unwrap();
    assert_eq!(mux::tree(&link).await.unwrap()[0].windows.len(), 1);

    let other = mux::new_session(&link, Some("second"), None).await.unwrap();
    let tree = mux::tree(&link).await.unwrap();
    let second = tree.iter().find(|x| x.name == "second").unwrap();
    assert_eq!(second.windows[0].panes[0].id, other);
    mux::kill_session(&link, &second.id).await.unwrap();
    assert_eq!(mux::tree(&link).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_link_reports_output_and_structure_and_never_resizes() {
    let s = Scratch::new();
    let pane = s.session("a", "cat");
    s.run(&["resize-window", "-t", "a", "-x", "132", "-y", "43"]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let link = Link::attach(&s.tmux, None, tx).unwrap();
    let session = mux::session_of(&link).await.unwrap();

    mux::text_in(&link, &pane, "ping", true).await.unwrap();
    wait_for(&mut rx, &Notice::Output(pane.clone())).await;

    mux::new_window(&link, &session, None, None).await.unwrap();
    wait_for(&mut rx, &Notice::Tree).await;

    let screen = mux::screen(&link, &pane).await.unwrap();
    assert_eq!(
        (screen.cols, screen.rows),
        (132, 43),
        "ignore-size left the window alone"
    );

    s.run(&["kill-server"]);
    wait_for(&mut rx, &Notice::Exit).await;
    assert!(!link.alive());
    assert!(matches!(mux::tree(&link).await, Err(Error::Closed(_))));
}

#[tokio::test]
async fn no_server_is_an_error_not_a_hang() {
    let s = Scratch::new();
    let err = mux::open(&s.tmux)
        .await
        .err()
        .expect("nothing to attach to");
    assert!(matches!(err, Error::Closed(_)), "{err:?}");
}

async fn wait_for(rx: &mut mpsc::UnboundedReceiver<(u64, Notice)>, want: &Notice) {
    let found = async {
        while let Some((_, notice)) = rx.recv().await {
            if &notice == want {
                return;
            }
        }
        panic!("the link closed before {want:?}");
    };
    tokio::time::timeout(Duration::from_secs(5), found)
        .await
        .unwrap_or_else(|_| panic!("no {want:?}"));
}

#[tokio::test]
async fn a_window_is_sized_for_a_phone_and_given_back() {
    let s = Scratch::new();
    let pane = s.session("work", "cat");
    let link = s.link().await;
    let size = || {
        s.run(&[
            "display-message",
            "-p",
            "-t",
            &pane,
            "#{window_width}x#{window_height}",
        ])
    };
    let policy = || s.run(&["show-options", "-w", "-t", &pane, "window-size"]);

    mux::fit(&link, &pane, 46, 28).await.unwrap();
    assert_eq!(size(), "46x28");
    assert_eq!(policy(), "window-size manual");
    let tree = mux::tree(&link).await.unwrap();
    assert_eq!(
        tree[0].windows[0].panes[0].cols, 46,
        "the tree reports the new width"
    );

    mux::unfit(&link, &pane).await.unwrap();
    assert_eq!(policy(), "", "tmux sizes the window again");

    // Sizes outside what a screen can be are brought inside it, not refused.
    mux::fit(&link, &pane, 5, 1000).await.unwrap();
    assert_eq!(size(), "20x200");
    mux::unfit(&link, &pane).await.unwrap();

    assert!(
        matches!(
            mux::fit(&link, "work", 40, 20).await,
            Err(Error::Invalid(_))
        ),
        "a pane id, not a name"
    );
}

#[tokio::test]
async fn a_click_reaches_only_a_program_that_asked_for_the_mouse() {
    let s = Scratch::new();
    let deaf = s.session("deaf", "cat");
    // Turns SGR mouse reporting on, then shows every byte it is sent.
    let keen = s.session("keen", "printf '\\033[?1000h\\033[?1006h'; exec cat -v");
    let link = s.link().await;

    assert!(
        !mux::click(&link, &deaf, 3, 2).await.unwrap(),
        "no mouse asked for, nothing sent"
    );
    // The program needs a moment to ask for the mouse; until then nothing is sent.
    let asked = std::time::Instant::now();
    while !mux::click(&link, &keen, 3, 2).await.unwrap() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(5),
            "the mouse was never asked for"
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    screen_with(&link, &keen, "^[[<0;4;3M^[[<0;4;3m").await;
}

#[tokio::test]
async fn the_wheel_turns_for_a_program_that_asked_for_the_mouse() {
    let s = Scratch::new();
    let keen = s.session("keen", "printf '\\033[?1000h\\033[?1006h'; exec cat -v");
    let link = s.link().await;

    // The program needs a moment to ask for the mouse; until then nothing is sent.
    let asked = std::time::Instant::now();
    while !mux::wheel(&link, &keen, 0, 0, -2).await.unwrap() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(5),
            "the mouse was never asked for"
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    screen_with(&link, &keen, "^[[<64;1;1M^[[<64;1;1M").await;
    assert!(mux::wheel(&link, &keen, 0, 0, 1).await.unwrap());
    screen_with(&link, &keen, "^[[<65;1;1M").await;
}

#[tokio::test]
async fn a_full_screen_program_is_told_apart_from_a_shell() {
    let s = Scratch::new();
    let plain = s.session("plain", "cat");
    // Switches to the alternate screen, then waits.
    let full = s.session("full", "printf '\\033[?1049h'; exec cat");
    let link = s.link().await;

    assert!(!mux::alternate(&link, &plain).await.unwrap());
    let asked = std::time::Instant::now();
    while !mux::alternate(&link, &full).await.unwrap() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(5),
            "never on the alternate screen"
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

#[tokio::test]
async fn split_then_move_between_the_two_panes() {
    let s = Scratch::new();
    let left = s.session("work", "cat");
    let link = s.link().await;

    let right = mux::split(&link, &left, true).await.unwrap();
    assert_ne!(right, left);
    let layout = s.run(&["list-panes", "-t", "work", "-F", "#{pane_id} #{pane_left}"]);
    assert!(
        layout.lines().any(|l| l.starts_with(&format!("{right} ")) && !l.ends_with(" 0")),
        "the new pane sits to the right: {layout}"
    );
    assert_eq!(mux::select_pane(&link, &right, "L").await.unwrap(), left);
    assert_eq!(mux::select_pane(&link, &left, "R").await.unwrap(), right);

    let below = mux::split(&link, &right, false).await.unwrap();
    assert_eq!(mux::select_pane(&link, &below, "U").await.unwrap(), right);
}
