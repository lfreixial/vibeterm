use alacritty_terminal::{
    event::VoidListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Term, TermMode, cell::Flags},
    vte::ansi::{Color, Processor, StdSyncHandler},
};
use vibeterm::{
    config::Config,
    session::{Size, engine_config},
};

fn terminal(columns: usize, rows: usize) -> Term<VoidListener> {
    Term::new(
        engine_config(&Config {
            scrollback: 20,
            ..Default::default()
        }),
        &Size {
            columns,
            rows,
            cell_width: 9,
            cell_height: 20,
        },
        VoidListener,
    )
}

#[test]
fn split_utf8_combining_wide_and_truecolor() {
    let mut term = terminal(20, 5);
    let mut parser = Processor::<StdSyncHandler>::new();
    let text = "a\u{301}界🦀\x1b[38;2;12;34;56mX";
    for byte in text.as_bytes() {
        parser.advance(&mut term, &[*byte]);
    }
    let grid = term.grid();
    assert_eq!(grid[Line(0)][Column(0)].c, 'a');
    assert_eq!(grid[Line(0)][Column(0)].zerowidth(), Some(&['\u{301}'][..]));
    assert!(grid[Line(0)][Column(1)].flags.contains(Flags::WIDE_CHAR));
    assert!(
        grid[Line(0)][Column(2)]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_eq!(grid[Line(0)][Column(3)].c, '🦀');
    assert_eq!(grid[Line(0)][Column(5)].c, 'X');
    assert!(
        matches!(grid[Line(0)][Column(5)].fg, Color::Spec(rgb) if rgb.r == 12 && rgb.g == 34 && rgb.b == 56)
    );
}

#[test]
fn alternate_screen_restores_primary_and_modes() {
    let mut term = terminal(20, 5);
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(
        &mut term,
        b"primary\x1b[?1049h\x1b[2J\x1b[Halternate\x1b[?2004h\x1b[?1006h",
    );
    assert!(
        term.mode()
            .contains(TermMode::ALT_SCREEN | TermMode::BRACKETED_PASTE | TermMode::SGR_MOUSE)
    );
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    parser.advance(&mut term, b"\x1b[?1049l");
    assert!(!term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'p');
}

#[test]
fn selection_joins_wrapped_lines_and_preserves_wide_text() {
    let mut term = terminal(6, 5);
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut term, "abcd界ef".as_bytes());
    let mut selection = Selection::new(
        SelectionType::Simple,
        Point::new(Line(0), Column(0)),
        Side::Left,
    );
    selection.update(Point::new(Line(1), Column(1)), Side::Right);
    term.selection = Some(selection);
    assert_eq!(term.selection_to_string().as_deref(), Some("abcd界ef"));
}

#[test]
fn history_is_bounded_and_resize_keeps_valid_cursor() {
    let mut term = terminal(12, 4);
    let mut parser = Processor::<StdSyncHandler>::new();
    for _ in 0..200 {
        parser.advance(&mut term, b"line\r\n");
    }
    assert_eq!(term.history_size(), 20);
    term.scroll_display(Scroll::Top);
    assert_eq!(term.grid().display_offset(), 20);
    term.resize(Size {
        columns: 4,
        rows: 2,
        cell_width: 9,
        cell_height: 20,
    });
    assert!(term.grid().cursor.point.column.0 < 4);
    assert!(term.grid().cursor.point.line.0 < 2);
}

#[test]
fn erasure_scroll_regions_and_malformed_streams_are_contained() {
    let mut term = terminal(10, 4);
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(
        &mut term,
        b"header\r\nrow1\r\nrow2\r\nfooter\x1b[2;3r\x1b[3;1H\n",
    );
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'h');
    assert_eq!(term.grid()[Line(3)][Column(0)].c, 'f');
    parser.advance(&mut term, b"\x1b[r\x1b[2J\x1b[H");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    let mut seed = 17u32;
    for _ in 0..10000 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        parser.advance(&mut term, &[(seed >> 24) as u8]);
    }
    parser.stop_sync(&mut term);
    assert_eq!(term.columns(), 10);
    assert!(term.history_size() <= 20);
}
