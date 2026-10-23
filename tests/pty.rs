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
