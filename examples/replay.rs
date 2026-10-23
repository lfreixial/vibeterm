//! Synthetic parser throughput only: this deliberately excludes rendering and PTY I/O.
use alacritty_terminal::{
    event::VoidListener,
    term::Term,
    vte::ansi::{Processor, StdSyncHandler},
};
use std::time::Instant;
use vibeterm::{
    config::Config,
    session::{Size, engine_config},
};

fn main() {
    let lines: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(200_000);
    let mut terminal = Term::new(
        engine_config(&Config::default()),
        &Size {
            columns: 120,
            rows: 40,
            cell_width: 9,
            cell_height: 20,
        },
        VoidListener,
    );
    let mut parser = Processor::<StdSyncHandler>::new();
    let fixture =
        "\x1b[38;2;128;203;196m0123456789 abcdefghijklmnopqrstuvwxyz\x1b[0m café 界 a\u{301}\r\n"
            .repeat(100);
    let start = Instant::now();
    let batches = lines.div_ceil(100);
    for _ in 0..batches {
        parser.advance(&mut terminal, fixture.as_bytes());
    }
    let elapsed = start.elapsed().as_secs_f64();
    let bytes = batches * fixture.len();
    println!(
        "parser-only: {bytes} bytes, {} lines, {:.6} s, {:.2} MiB/s; 120x40, scrollback=10000",
        batches * 100,
        elapsed,
        bytes as f64 / 1048576.0 / elapsed
    );
}
