use alacritty_terminal::{
    event::VoidListener,
    grid::Scroll,
    index::{Column, Line, Point},
    term::Term,
    vte::ansi::{Processor, StdSyncHandler},
};
use vibeterm::{
    config::Config,
    links::{self, Target},
    session::{Size, engine_config},
};

fn terminal(text: &str, columns: usize, rows: usize) -> Term<VoidListener> {
    let mut term = Term::new(
        engine_config(&Config::default()),
        &Size {
            columns,
            rows,
            cell_width: 9,
            cell_height: 20,
        },
        VoidListener,
    );
    Processor::<StdSyncHandler>::new().advance(&mut term, text.as_bytes());
    term
}
fn at(term: &Term<VoidListener>, row: i32, col: usize) -> Option<links::Link> {
    links::at(term, Point::new(Line(row), Column(col)))
}
#[test]
fn urls_handle_wrapping_unicode_and_punctuation() {
    let text = "https://example.com/a_long_path?q=1";
    let term = terminal(text, 12, 5);
    assert_eq!(at(&term, 1, 3).unwrap().target, Target::Url(text.into()));
    let term = terminal("界a\u{301} (https://example.com/wiki/Foo_(bar)).", 80, 4);
    let link = at(&term, 0, 8).unwrap();
    assert_eq!(
        link.target,
        Target::Url("https://example.com/wiki/Foo_(bar)".into())
    );
    assert_eq!(link.start.column.0, 5);
    assert!(at(&term, 0, 3).is_none());
}
#[test]
fn file_locations_and_quoted_spaces() {
    let term = terminal(
        "src/main.rs:42:7: error\r\n'folder/my file.rs'\r\nfile.rs:12",
        80,
        4,
    );
    assert_eq!(
        at(&term, 0, 3).unwrap().target,
        Target::File {
            path: "src/main.rs".into(),
            line: Some(42),
            column: Some(7)
        }
    );
    assert_eq!(
        at(&term, 1, 10).unwrap().target,
        Target::File {
            path: "folder/my file.rs".into(),
            line: None,
            column: None
        }
    );
    assert_eq!(
        at(&term, 2, 3).unwrap().target,
        Target::File {
            path: "file.rs".into(),
            line: Some(12),
            column: None
        }
    );
    for text in [
        "ordinary",
        "javascript:alert(1)",
        "file.rs:0",
        "https://",
        "ssh://example.com",
    ] {
        assert!(at(&terminal(text, 80, 4), 0, 1).is_none(), "{text}");
    }
}
#[test]
fn explicit_links_use_the_destination_and_reject_unsafe_schemes() {
    let term = terminal(
        "\x1b]8;;https://example.com/private\x1b\\click here\x1b]8;;\x1b\\ plain",
        80,
        4,
    );
    let link = at(&term, 0, 6).unwrap();
    assert_eq!(
        link.target,
        Target::Url("https://example.com/private".into())
    );
    assert_eq!(link.start.column.0, 0);
    assert_eq!(link.end.column.0, 9);
    assert!(at(&term, 0, 12).is_none());
    let term = terminal(
        "\x1b]8;;javascript:alert(1)\x1b\\https://example.com\x1b]8;;\x1b\\",
        80,
        4,
    );
    assert!(at(&term, 0, 3).is_none());
}
#[test]
fn links_are_detected_in_scrollback_and_not_across_hard_newlines() {
    let mut term = terminal("https://example.com\r\none\r\ntwo\r\nthree", 80, 2);
    term.scroll_display(Scroll::Top);
    assert_eq!(
        at(&term, -2, 3).unwrap().target,
        Target::Url("https://example.com".into())
    );
    let term = terminal("https://\r\nexample.com", 80, 3);
    assert!(at(&term, 0, 2).is_none());
}
#[test]
fn hidden_and_overlong_output_are_not_opened() {
    let term = terminal("\x1b[8mhttps://example.com", 80, 4);
    assert!(at(&term, 0, 2).is_none());
    let text = format!("https://example.com/{}", "x".repeat(9000));
    let term = terminal(&text, 80, 120);
    assert!(at(&term, 0, 2).is_none());
}
#[test]
fn editor_argv_preserves_literal_filenames_and_locations() {
    let dir = tempfile::tempdir().unwrap();
    let name = "space $(touch nope){line}.rs";
    std::fs::write(dir.path().join(name), "fixture").unwrap();
    let target = Target::File {
        path: name.into(),
        line: Some(42),
        column: Some(7),
    };
    let editor = ["code", "--goto", "{path}:{line}:{column}"].map(String::from);
    let command = links::command(&target, dir.path(), &editor).unwrap();
    assert_eq!(command.get_program(), "code");
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "--goto".to_string(),
            format!("{}:42:7", dir.path().join(name).display())
        ]
    );
    let command = links::command(&target, dir.path(), &[]).unwrap();
    assert_eq!(command.get_program(), "xdg-open");
    assert_eq!(command.get_args().next().unwrap(), dir.path().join(name));
    assert!(links::command(&Target::Url("javascript:evil()".into()), dir.path(), &[]).is_err());
    assert!(
        links::command(
            &Target::File {
                path: "missing.rs".into(),
                line: None,
                column: None
            },
            dir.path(),
            &editor
        )
        .is_err()
    );
}
#[test]
fn editor_configuration_requires_a_path_argument() {
    assert!(Config::parse("link_editor = ['code', '--goto', '{path}:{line}:{column}']").is_ok());
    assert!(Config::parse("link_editor = ['code']").is_err());
    assert!(Config::parse("link_editor = ['', '{path}']").is_err());
}

#[test]
fn bare_domains_never_fall_back_to_websites() {
    let dir = tempfile::tempdir().unwrap();
    for text in [
        "google.com",
        "www.example.org",
        "example.co.uk/path",
        "EXAMPLE.COM",
    ] {
        let link = at(&terminal(text, 80, 4), 0, 3).unwrap();
        assert!(matches!(link.target, Target::File { .. }));
        assert!(links::command(&link.target, dir.path(), &[]).is_err());
    }
    std::fs::write(dir.path().join("google.com"), "local fixture").unwrap();
    let target = at(&terminal("google.com", 80, 4), 0, 3).unwrap().target;
    let command = links::command(&target, dir.path(), &[]).unwrap();
    assert_eq!(
        command.get_args().next().unwrap(),
        dir.path().join("google.com")
    );
    let explicit = at(&terminal("https://google.com", 80, 4), 0, 3)
        .unwrap()
        .target;
    let command = links::command(&explicit, dir.path(), &[]).unwrap();
    assert_eq!(command.get_args().next().unwrap(), "https://google.com");
}

#[test]
fn single_click_opening_is_explicitly_opt_in() {
    assert!(!Config::default().link_single_click);
    assert!(!Config::parse("").unwrap().link_single_click);
    assert!(
        Config::parse("link_single_click = true")
            .unwrap()
            .link_single_click
    );
    assert!(Config::parse("link_single_click = 'yes'").is_err());
}
