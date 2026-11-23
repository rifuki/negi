//! tmux control mode lines, as `man tmux` (CONTROL MODE) defines them.

/// What a control client says outside a reply block, reduced to what the hub acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A pane wrote to its screen, or entered or left a mode.
    Output(String),
    /// Sessions, windows or panes were created, closed, renamed or rearranged.
    Tree,
    /// The client is now attached to this session.
    Moved(String),
    /// A paste buffer was made or changed, by a program's OSC 52 or a copy in copy mode.
    Buffer(String),
    /// The client is exiting.
    Exit,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Line<'a> {
    /// Opens a reply block. `ours` is false for blocks tmux emits on its own, such as the
    /// one for the attach itself.
    Begin {
        tag: &'a [u8],
        ours: bool,
    },
    End {
        tag: &'a [u8],
    },
    Error {
        tag: &'a [u8],
    },
    Notice(Notice),
    /// A notification the hub has no use for.
    Ignored,
    /// Anything that does not start with `%`.
    Text,
}

/// Reads one line, without its newline, as it appears outside a reply block. Inside a block
/// only [`closes`] applies: a pane is free to print `%end` or `%output` itself.
pub fn parse(line: &[u8]) -> Line<'_> {
    let Some(rest) = line.strip_prefix(b"%") else {
        return Line::Text;
    };
    let (word, args) = split(rest);
    match word {
        b"begin" => Line::Begin {
            tag: args,
            ours: args.rsplit(|b| *b == b' ').next() == Some(b"1"),
        },
        b"end" => Line::End { tag: args },
        b"error" => Line::Error { tag: args },
        b"output" | b"pane-mode-changed" => pane(split(args).0),
        // `%extended-output %1 <age> ... : value`
        b"extended-output" => pane(split(args).0),
        b"session-changed" => match split(args).0 {
            id if id.starts_with(b"$") => Line::Notice(Notice::Moved(text(id))),
            _ => Line::Ignored,
        },
        b"exit" => Line::Notice(Notice::Exit),
        b"paste-buffer-changed" if !args.is_empty() => Line::Notice(Notice::Buffer(text(args))),
        b"sessions-changed"
        | b"session-renamed"
        | b"session-window-changed"
        | b"window-add"
        | b"window-close"
        | b"window-renamed"
        | b"window-pane-changed"
        | b"layout-change"
        | b"unlinked-window-add"
        | b"unlinked-window-close"
        | b"unlinked-window-renamed" => Line::Notice(Notice::Tree),
        _ => Line::Ignored,
    }
}

/// True when `line` ends the block that `%begin <tag>` opened. The tag carries the time and
/// tmux's own command number, which pane content cannot know in advance.
pub fn closes(line: &[u8], tag: &[u8]) -> Option<bool> {
    match parse(line) {
        Line::End { tag: t } if t == tag => Some(true),
        Line::Error { tag: t } if t == tag => Some(false),
        _ => None,
    }
}

fn split(s: &[u8]) -> (&[u8], &[u8]) {
    match s.iter().position(|b| *b == b' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &[]),
    }
}

fn pane(id: &[u8]) -> Line<'static> {
    if id.starts_with(b"%") && id.len() > 1 {
        Line::Notice(Notice::Output(text(id)))
    } else {
        Line::Ignored
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_blocks_carry_their_tag() {
        assert_eq!(
            parse(b"%begin 1794682707 321 1"),
            Line::Begin {
                tag: b"1794682707 321 1",
                ours: true
            }
        );
        assert_eq!(
            parse(b"%begin 1794682706 319 0"),
            Line::Begin {
                tag: b"1794682706 319 0",
                ours: false
            }
        );
        assert_eq!(
            closes(b"%end 1794682707 321 1", b"1794682707 321 1"),
            Some(true)
        );
        assert_eq!(
            closes(b"%error 1794682707 321 1", b"1794682707 321 1"),
            Some(false)
        );
    }

    #[test]
    fn a_pane_printing_end_does_not_close_the_block() {
        assert_eq!(closes(b"%end 1 2 3", b"1794682780 321 1"), None);
        assert_eq!(closes(b"%end", b"1794682780 321 1"), None);
        assert_eq!(closes(b"plain text", b"1794682780 321 1"), None);
    }

    #[test]
    fn output_names_its_pane_and_ignores_the_payload() {
        assert_eq!(
            parse(b"%output %12 hi\\015\\012 %end 1 2 3"),
            Line::Notice(Notice::Output("%12".into()))
        );
        assert_eq!(
            parse(b"%extended-output %3 120 : text"),
            Line::Notice(Notice::Output("%3".into()))
        );
        assert_eq!(
            parse(b"%pane-mode-changed %4"),
            Line::Notice(Notice::Output("%4".into()))
        );
        assert_eq!(parse(b"%output"), Line::Ignored);
    }

    #[test]
    fn structure_changes_collapse_to_one_notice() {
        for line in [
            &b"%sessions-changed"[..],
            b"%window-add @3",
            b"%window-close @3",
            b"%window-renamed @0 -n new'name",
            b"%unlinked-window-renamed @1 zsh",
            b"%session-window-changed $0 @1",
            b"%layout-change @0 b25d,80x24,0,0,0 b25d,80x24,0,0,0 *",
        ] {
            assert_eq!(parse(line), Line::Notice(Notice::Tree), "{:?}", text(line));
        }
    }

    #[test]
    fn the_rest() {
        assert_eq!(
            parse(b"%session-changed $1 beta"),
            Line::Notice(Notice::Moved("$1".into()))
        );
        assert_eq!(parse(b"%exit"), Line::Notice(Notice::Exit));
        assert_eq!(parse(b"%exit server exited"), Line::Notice(Notice::Exit));
        assert_eq!(parse(b"%client-detached /dev/ttys001"), Line::Ignored);
        assert_eq!(
            parse(b"%paste-buffer-changed buffer0"),
            Line::Notice(Notice::Buffer("buffer0".into()))
        );
        assert_eq!(parse(b"hello"), Line::Text);
        assert_eq!(parse(b""), Line::Text);
    }
}
