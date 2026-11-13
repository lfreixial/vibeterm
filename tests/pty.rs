use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line},
};
use std::{
    sync::{Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};
use vibeterm::{
    config::Config,
    render::{Damage, Palette, Snapshot},
    session::{Session, Size},
};

fn size(columns: usize, rows: usize) -> Size {
    Size {
        columns,
        rows,
        cell_width: 9,
        cell_height: 20,
    }
}
fn spawn(script: &str) -> Session {
    Session::spawn(
        &Config::default(),
        size(80, 24),
        &["/bin/sh".into(), "-c".into(), script.into()],
        None,
        Arc::new(|| {}),
    )
    .unwrap()
}
fn visible(session: &Session) -> String {
    let term = session.terminal.lock();
    let mut text = String::new();
    for row in 0..term.screen_lines() {
        for col in 0..term.columns() {
            text.push(term.grid()[Line(row as i32)][Column(col)].c);
        }
        text.push('\n');
    }
    text
}
fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "PTY condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn real_shell_input_output_and_exit() {
    let session = spawn("printf 'READY\\n'; read value; printf 'REPLY:%s\\n' \"$value\"");
    wait_until(|| visible(&session).contains("READY"));
    session.input(b"hello world\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    assert!(visible(&session).contains("REPLY:hello world"));
    assert!(session.shared.error.lock().unwrap().is_none());
}

#[test]
fn resize_reaches_kernel_and_child() {
    let session = spawn("printf 'READY\\n'; read value; stty size");
    wait_until(|| visible(&session).contains("READY"));
    session.resize(size(100, 31)).unwrap();
    session.input(b"go\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    assert!(visible(&session).contains("31 100"));
    assert_eq!(session.terminal.lock().columns(), 100);
}

#[test]
fn closing_reaps_a_hangup_resistant_child() {
    let session = spawn("trap '' HUP; printf 'READY\\n'; while :; do :; done");
    wait_until(|| visible(&session).contains("READY"));
    let start = Instant::now();
    session.close();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    drop(session);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn ordered_output_and_sync_update_timeout() {
    let session = spawn("printf '\\033[?2026hAFTER_SYNC'; sleep 1; printf '\\r\\nDONE\\n'");
    wait_until(|| visible(&session).contains("AFTER_SYNC"));
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    assert!(visible(&session).contains("DONE"));
}

#[test]
fn large_input_is_rejected_atomically() {
    let session = spawn("read value");
    assert!(session.input(vec![b'x'; 256 * 1024 + 1]).is_err());
    session.input(b"done\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
}

#[test]
fn background_search_finds_literal_text_in_old_history_and_wraps() {
    use alacritty_terminal::index::Point;
    use vibeterm::search::{Request, SearchWorker};
    let session = spawn(
        "printf '[needle].\\r\\n'; i=0; while [ $i -lt 300 ]; do printf 'filler\\r\\n'; i=$((i+1)); done; printf '[needle].\\r\\n'",
    );
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = SearchWorker::new(move |response| {
        tx.send(response).unwrap();
    });
    for (backward, start_at_end, expected_history) in [
        (false, false, true),
        (true, true, false),
        (false, true, true),
    ] {
        let term = session.terminal.lock();
        let origin = if start_at_end {
            Point::new(term.bottommost_line(), term.last_column())
        } else {
            Point::new(term.topmost_line(), Column(0))
        };
        drop(term);
        let token = worker.cancel();
        worker
            .submit(Request {
                token,
                pane: 1,
                query: "[needle].".into(),
                origin,
                backward,
                terminal: session.terminal.clone(),
                revision: session.shared.revision.clone(),
            })
            .unwrap();
        let response = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let matched = response.result.unwrap().unwrap();
        assert_eq!(matched.start().line.0 < 0, expected_history);
        assert_eq!(
            session
                .terminal
                .lock()
                .bounds_to_string(*matched.start(), *matched.end()),
            "[needle]."
        );
    }
    let cancelled = worker.cancel();
    let latest = worker.cancel();
    for token in [cancelled, latest] {
        worker
            .submit(Request {
                token,
                pane: 1,
                query: "[needle].".into(),
                origin: Point::default(),
                backward: false,
                terminal: session.terminal.clone(),
                revision: session.shared.revision.clone(),
            })
            .unwrap();
    }
    let response = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        response.token, latest,
        "Cancelled requests must not publish a result"
    );
    assert!(response.result.unwrap().is_some());
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn independent_sessions_route_input_and_close_without_affecting_each_other() {
    let first = spawn("printf 'READY\\n'; read value; printf 'FIRST:%s\\n' \"$value\"");
    let second = spawn("printf 'READY\\n'; read value; printf 'SECOND:%s\\n' \"$value\"");
    wait_until(|| visible(&first).contains("READY") && visible(&second).contains("READY"));
    first.input(b"one\r".to_vec()).unwrap();
    wait_until(|| first.shared.exited.load(Ordering::Acquire));
    assert!(!second.shared.exited.load(Ordering::Acquire));
    second.input(b"two\r".to_vec()).unwrap();
    wait_until(|| second.shared.exited.load(Ordering::Acquire));
    assert!(visible(&first).contains("FIRST:one"));
    assert!(visible(&second).contains("SECOND:two"));
    assert!(!visible(&second).contains("FIRST"));
}

#[test]
fn working_directory_follows_the_shell() {
    let session = spawn("printf 'READY\\n'; read value; cd /usr; printf 'MOVED\\n'; read value");
    wait_until(|| visible(&session).contains("READY"));
    assert_eq!(session.cwd(), Some(std::env::current_dir().unwrap()));
    session.input(b"go\r".to_vec()).unwrap();
    wait_until(|| visible(&session).contains("MOVED"));
    assert_eq!(session.cwd().as_deref(), Some(std::path::Path::new("/usr")));
    session.input(b"done\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    assert_eq!(session.cwd(), None);
}

#[test]
fn snapshots_repaint_only_damaged_rows() {
    let session =
        spawn("printf 'one\\ntwo\\n'; read value; printf 'X'; read value; seq 1 40; read value");
    wait_until(|| visible(&session).contains("two"));
    let palette = Palette::new(&Config::default());
    let mut snapshot = Snapshot::default();
    let update = |snapshot: &mut Snapshot| {
        snapshot.update(&mut session.terminal.lock(), &palette, true, true, "")
    };
    assert_eq!(update(&mut snapshot), Damage::Full);
    let text: String = snapshot.rows[0]
        .iter()
        .filter_map(|cell| cell.glyph.as_ref().map(|glyph| glyph.base))
        .collect();
    assert_eq!(text, "one");
    // Idle: only the row holding the cursor, which the engine always reports.
    assert_eq!(update(&mut snapshot), Damage::Rows(vec![2]));
    // The echoed reply lands on row 2 and output moves on to row 3.
    session.input(b"go\r".to_vec()).unwrap();
    wait_until(|| visible(&session).contains('X'));
    assert_eq!(update(&mut snapshot), Damage::Rows(vec![2, 3]));
    // A cursor style change repaints the old and new cursor rows only.
    let damage = snapshot.update(&mut session.terminal.lock(), &palette, true, false, "");
    assert_eq!(damage, Damage::Rows(vec![3]));
    // Output that scrolls the screen changes every row.
    session.input(b"more\r".to_vec()).unwrap();
    wait_until(|| visible(&session).contains("40"));
    let damage = snapshot.update(&mut session.terminal.lock(), &palette, true, true, "");
    assert_eq!(damage, Damage::Full);
    // So does scrolling the view back into history.
    session
        .terminal
        .lock()
        .scroll_display(alacritty_terminal::grid::Scroll::Delta(1));
    let damage = snapshot.update(&mut session.terminal.lock(), &palette, true, true, "");
    assert_eq!(damage, Damage::Full);
    session.input(b"done\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
}

#[test]
fn programs_can_copy_but_not_read_the_clipboard() {
    // "aGVsbG8=" is base64 for "hello"; the second sequence asks to read the clipboard.
    let session =
        spawn("printf '\\033]52;c;aGVsbG8=\\a\\033]52;p;?\\a'; printf 'READY\\n'; read value");
    wait_until(|| visible(&session).contains("READY"));
    let copied = session.shared.clipboard.lock().unwrap().clone();
    assert_eq!(copied, [Some("hello".to_string()), None]);
    session.input(b"\r".to_vec()).unwrap();
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    assert!(session.shared.error.lock().unwrap().is_none());
}

#[test]
fn resize_burst_keeps_latest_geometry_without_filling_the_input_queue() {
    let session = spawn("printf READY; read value; stty size; printf 'ACK:%s' \"$value\"");
    wait_until(|| visible(&session).contains("READY"));
    // Model a busy renderer/parser while pointer events keep arriving.
    let terminal = session.terminal.lock();
    for i in 0..5000 {
        session.resize(size(80 + i % 40, 24)).unwrap();
    }
    session.resize(size(101, 37)).unwrap();
    session.input(b"hello\r".to_vec()).unwrap();
    drop(terminal);
    wait_until(|| session.shared.exited.load(Ordering::Acquire));
    let terminal = session.terminal.lock();
    assert_eq!((terminal.columns(), terminal.screen_lines()), (101, 37));
    drop(terminal);
    let output = visible(&session);
    assert!(output.contains("37 101"), "{output}");
    assert!(output.contains("ACK:hello"), "{output}");
    assert!(session.shared.error.lock().unwrap().is_none());
}

#[test]
fn closed_sessions_reject_resize_requests() {
    let session = spawn("sleep 10");
    session.close();
    assert!(session.resize(size(100, 30)).is_err());
}

// Build the same private startup files as normal pane creation, with an isolated
// HOME/ZDOTDIR. Passing env as argv avoids mutating the test process environment.
fn integrated_shell(shell: &str, rc: &str) -> (Session, tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = portable_pty::CommandBuilder::new(shell);
    cmd.env("HOME", home.path());
    cmd.env_remove("ZDOTDIR");
    if shell.ends_with("bash") {
        std::fs::write(
            home.path().join(".bashrc"),
            format!("PS1='READY> '\n{rc}\n"),
        )
        .unwrap();
    } else {
        std::fs::write(home.path().join(".zshenv"), "export VT_ENV_LOADED=yes\n").unwrap();
        std::fs::write(home.path().join(".zshrc"), format!("PS1='READY> '\n{rc}\n")).unwrap();
    }
    let files = vibeterm::shell::prepare(&mut cmd, std::ffi::OsStr::new(shell))
        .unwrap()
        .unwrap();
    let mut args: Vec<std::ffi::OsString> =
        vec!["/usr/bin/env".into(), "-u".into(), "ZDOTDIR".into()];
    for (key, value) in cmd.iter_extra_env_as_str() {
        args.push(format!("{key}={value}").into());
    }
    args.extend(cmd.get_argv().iter().cloned());
    let session = Session::spawn(
        &Config::default(),
        size(80, 10),
        &args,
        Some(home.path()),
        Arc::new(|| {}),
    )
    .unwrap();
    wait_until(|| visible(&session).contains("READY>"));
    (session, home, files)
}
fn last_output(session: &Session) -> Option<String> {
    session
        .shared
        .commands
        .lock()
        .unwrap()
        .last_output()
        .map(str::to_owned)
}
fn wait_for_output(session: &Session, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = last_output(session);
        if output.as_deref() == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Expected {expected:?}; captured {output:?}; terminal:\n{}",
            visible(session)
        );
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn bash_integration_preserves_prompt_commands_status_and_copies_real_output() {
    let (session, _home, _files) = integrated_shell(
        "/bin/bash",
        "PROMPT_COMMAND=('VT_STATUS=$?' 'VT_HOOK=kept')",
    );
    session
        .input(b"printf '\\033[31mhello world\\033[0m\\n'; false\r".to_vec())
        .unwrap();
    wait_for_output(&session, "hello world\n");
    session
        .input(b"printf '%s:%s\\n' \"$VT_STATUS\" \"$VT_HOOK\"\r".to_vec())
        .unwrap();
    wait_for_output(&session, "1:kept\n");
    session.input(b"printf next\r".to_vec()).unwrap();
    wait_for_output(&session, "next");
}
#[test]
fn bash_integration_does_not_replace_an_existing_debug_trap() {
    let (session, _home, _files) = integrated_shell("/bin/bash", "trap 'VT_TRAP=kept' DEBUG");
    session
        .input(b"printf 'TRAP:%s\\n' \"$VT_TRAP\"\r".to_vec())
        .unwrap();
    wait_until(|| visible(&session).contains("TRAP:kept"));
    assert!(last_output(&session).is_none());
}
fn zsh_program() -> String {
    std::env::var("VIBETERM_TEST_ZSH").unwrap_or_else(|_| "/bin/zsh".into())
}
#[test]
fn zsh_integration_preserves_startup_and_prompt_hooks() {
    let shell = zsh_program();
    if !std::path::Path::new(&shell).exists() {
        eprintln!("zsh unavailable; skipping real zsh integration test");
        return;
    }
    let (session, _home, _files) = integrated_shell(
        &shell,
        "precmd() { VT_STATUS=$?; }; vt_hook() { VT_HOOK=kept; }; precmd_functions=(vt_hook)",
    );
    session
        .input(b"printf 'hello zsh\\n'; false\r".to_vec())
        .unwrap();
    wait_for_output(&session, "hello zsh\n");
    session.input(b"printf '%s:%s:%s:%s\\n' \"$VT_STATUS\" \"$VT_HOOK\" \"$VT_ENV_LOADED\" \"${ZDOTDIR-unset}\"\r".to_vec()).unwrap();
    wait_for_output(&session, "1:kept:yes:unset\n");
    session.input(b"printf no-newline\r".to_vec()).unwrap();
    wait_for_output(&session, "no-newline");
}

#[test]
fn zsh_integration_restores_zdotdir_changed_by_zshenv() {
    let shell = zsh_program();
    if !std::path::Path::new(&shell).exists() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let original = home.path().join("original dots");
    let redirected = home.path().join("new dots");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&redirected).unwrap();
    std::fs::write(
        original.join(".zshenv"),
        "ZDOTDIR=$HOME/'new dots'\nexport VT_ORIGINAL_READ=yes\n",
    )
    .unwrap();
    std::fs::write(
        redirected.join(".zshrc"),
        "PS1='READY> '\nexport VT_RC_READ=yes\n",
    )
    .unwrap();
    let mut cmd = portable_pty::CommandBuilder::new(&shell);
    cmd.env("HOME", home.path());
    cmd.env("ZDOTDIR", &original);
    let _files = vibeterm::shell::prepare(&mut cmd, std::ffi::OsStr::new(&shell)).unwrap();
    let mut args: Vec<std::ffi::OsString> = vec!["/usr/bin/env".into()];
    for (key, value) in cmd.iter_extra_env_as_str() {
        args.push(format!("{key}={value}").into());
    }
    args.extend(cmd.get_argv().iter().cloned());
    let session = Session::spawn(
        &Config::default(),
        size(80, 10),
        &args,
        Some(home.path()),
        Arc::new(|| {}),
    )
    .unwrap();
    wait_until(|| visible(&session).contains("READY>"));
    session
        .input(
            b"printf '%s:%s:%s\\n' \"$VT_ORIGINAL_READ\" \"$VT_RC_READ\" \"$ZDOTDIR\"\r".to_vec(),
        )
        .unwrap();
    let expected = format!("yes:yes:{}\n", redirected.display());
    wait_for_output(&session, expected.as_str());
}
