use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line, Point},
    term::Term,
    vte::ansi::{Processor, StdSyncHandler},
};
use vibeterm::{
    config::Config,
    session::{Size, engine_config},
    shell::{History, Tracked},
};

fn size(columns: usize, rows: usize) -> Size {
    Size {
        columns,
        rows,
        cell_width: 9,
        cell_height: 20,
    }
}
struct Harness {
    term: Term<VoidListener>,
    parser: Processor<StdSyncHandler>,
    history: History,
}
impl Harness {
    fn new(columns: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            term: Term::new(
                engine_config(&Config {
                    scrollback,
                    ..Config::default()
                }),
                &size(columns, rows),
                VoidListener,
            ),
            parser: Processor::new(),
            history: History::default(),
        }
    }
    fn feed(&mut self, text: &str) {
        // One-byte chunks exercise split UTF-8 and escape sequences.
        for byte in text.bytes() {
            self.parser.advance(
                &mut Tracked {
                    term: &mut self.term,
                    history: &mut self.history,
                },
                &[byte],
            );
        }
    }
    fn mark(&mut self, mark: char) {
        self.feed(&format!("\x1b]2;__vibeterm_shell__;{mark}\x07"));
    }
    fn command(&mut self, output: &str) {
        self.mark('A');
        self.feed("$ cmd\r\n");
        self.mark('C');
        self.feed(output);
        self.mark('D');
    }
}
#[test]
fn copy_excludes_prompt_command_and_ansi_preserves_wraps_and_unicode() {
    let mut h = Harness::new(10, 4, 50);
    h.command("\x1b[31mhello界e\u{301}world\x1b[0m\r\n\r\n");
    assert_eq!(h.history.last_output(), Some("hello界e\u{301}world\n\n"));
    h.command("no newline");
    assert_eq!(h.history.last_output(), Some("no newline"));
}
#[test]
fn empty_output_and_sync_updates() {
    let mut h = Harness::new(10, 4, 50);
    h.command("");
    assert_eq!(h.history.last_output(), Some(""));
    h.feed("\x1b[?2026h");
    h.command("value\r\n");
    assert_eq!(h.history.last_output(), Some(""));
    h.feed("\x1b[?2026l");
    assert_eq!(h.history.last_output(), Some("value\n"));
}
#[test]
fn navigation_survives_full_scrollback_and_prunes_evicted_commands() {
    let mut h = Harness::new(12, 3, 8);
    for _ in 0..20 {
        h.command("hello\r\n");
    }
    assert_eq!(h.term.history_size(), 8);
    assert!(h.history.navigate(&mut h.term, true));
    assert_eq!(h.term.grid().display_offset(), 0);
    assert!(h.history.navigate(&mut h.term, true));
    assert_eq!(h.term.grid().display_offset(), 2);
    assert!(h.history.navigate(&mut h.term, false));
    assert_eq!(h.term.grid().display_offset(), 0);
    let mut count = 0;
    while h.history.navigate(&mut h.term, true) {
        count += 1;
    }
    assert_eq!(count, 4);
    assert_eq!(h.term.grid().display_offset(), 8);
}
#[test]
fn overflow_resize_and_fullscreen_do_not_copy_unrelated_text() {
    let mut h = Harness::new(12, 3, 2);
    h.command("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\n");
    assert_eq!(h.history.last_output(), None);
    h.command("kept\r\n");
    h.term.resize(size(8, 5));
    h.history.resized();
    assert!(!h.history.navigate(&mut h.term, true));
    assert_eq!(h.history.last_output(), Some("kept\n"));
    h.command("\x1b[?1049hfullscreen\x1b[?1049l");
    assert_eq!(h.history.last_output(), None);
}
#[test]
fn tracking_does_not_change_terminal_rendering() {
    let mut h = Harness::new(10, 4, 20);
    let mut plain = Harness::new(10, 4, 20);
    let text =
        "abc界e\u{301}\r\n\x1b[31mred\x1b[0m\r\n\x1b[2;3Hhere\x1b[?1049hother\x1b[?1049l\r\nlast";
    h.feed(text);
    plain.parser.advance(&mut plain.term, text.as_bytes());
    assert_eq!(h.term.grid().cursor.point, plain.term.grid().cursor.point);
    for row in -(h.term.history_size() as i32)..h.term.screen_lines() as i32 {
        for col in 0..h.term.columns() {
            assert_eq!(
                h.term.grid()[Point::new(Line(row), Column(col))],
                plain.term.grid()[Point::new(Line(row), Column(col))]
            );
        }
    }
}
