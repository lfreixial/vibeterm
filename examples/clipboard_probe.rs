//! Clipboard helper for the desktop integration script; not part of the application UI.
use anyhow::Result;
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind};

fn main() -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("copy") => {
            clipboard.set_text(args.get(1).cloned().unwrap_or_default())?;
            println!("ready");
            // Retain clipboard ownership until the smoke test has pasted.
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
        }
        Some("primary") => println!(
            "{}",
            clipboard
                .get()
                .clipboard(LinuxClipboardKind::Primary)
                .text()?
        ),
        _ => println!("{}", clipboard.get_text()?),
    }
    Ok(())
}
