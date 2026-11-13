//! Headless PTY responsiveness probe. Run in release mode; this is not display latency.
use alacritty_terminal::grid::Dimensions;
use std::{
    sync::{Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};
use vibeterm::{
    config::Config,
    session::{Session, Size},
};
fn size(columns: usize) -> Size {
    Size {
        columns,
        rows: 32,
        cell_width: 9,
        cell_height: 20,
    }
}
fn main() -> anyhow::Result<()> {
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let session = Session::spawn(
        &Config::default(),
        size(100),
        &[
            "/bin/sh".into(),
            "-c".into(),
            "printf READY; read value; printf '\\033]0;ACK\\007'; sleep 1".into(),
        ],
        None,
        wake.clone(),
    )?;
    thread::sleep(Duration::from_millis(100));
    let guard = session.terminal.lock();
    let start = Instant::now();
    let rejected = (0..5000)
        .filter(|i| session.resize(size(80 + i % 40)).is_err())
        .count();
    let enqueue = start.elapsed();
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.terminal.lock().columns() != 119 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    println!(
        "resize burst: rejected={rejected}/5000 enqueue_ms={:.3} final_columns={} settle_ms={:.3}",
        enqueue.as_secs_f64() * 1000.0,
        session.terminal.lock().columns(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    session.close();
    drop(session);
    for floods in [0, 3] {
        let busy: Vec<_> = (0..floods).map(|_| Session::spawn(&Config::default(), size(100), &["/bin/sh".into(), "-c".into(), "while :; do printf 'background output 012345678901234567890123456789\\n'; done".into()], None, wake.clone()).unwrap()).collect();
        let active = Session::spawn(&Config::default(), size(100), &["/bin/sh".into(), "-c".into(), "stty -echo; printf '\\033]0;READY\\007'; while read value; do printf '\\033]0;%s\\007' \"$value\"; done".into()], None, wake.clone())?;
        let ready = Instant::now();
        while active.shared.title.lock().unwrap().as_str() != "READY" {
            anyhow::ensure!(
                ready.elapsed() < Duration::from_secs(5),
                "shell readiness timeout"
            );
            thread::sleep(Duration::from_millis(1));
        }
        let mut samples = Vec::new();
        for i in 0..100 {
            let value = format!("ack-{i}");
            let start = Instant::now();
            active.input(format!("{value}\r").into_bytes())?;
            while active.shared.title.lock().unwrap().as_str() != value {
                anyhow::ensure!(start.elapsed() < Duration::from_secs(5), "echo timeout");
                thread::sleep(Duration::from_micros(100));
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "PTY ack with {floods} background floods: p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
            samples[50], samples[95], samples[99]
        );
        active.close();
        for pane in &busy {
            pane.close();
        }
        drop(busy);
        anyhow::ensure!(
            !active.shared.exited.load(Ordering::Acquire)
                || active.shared.error.lock().unwrap().is_none(),
            "PTY failed"
        );
    }
    Ok(())
}
