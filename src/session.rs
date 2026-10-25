//! One PTY owner, one byte stream, bounded input, coalesced UI notifications.
use crate::config::{Config, CursorShape};
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::Dimensions,
    sync::FairMutex,
    term::{self, Term},
    vte::ansi::{self, Processor},
};
use anyhow::{Context, Result, bail};
use portable_pty::{CommandBuilder, PtySize};
use std::{
    ffi::OsString,
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const MAX_INPUT: usize = 256 * 1024;
pub type Terminal = Term<Listener>;
pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: usize,
    pub rows: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.columns
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn total_lines(&self) -> usize {
        self.rows
    }
}
impl Size {
    fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows as u16,
            cols: self.columns as u16,
            pixel_width: (self.columns as u16).saturating_mul(self.cell_width),
            pixel_height: (self.rows as u16).saturating_mul(self.cell_height),
        }
    }
}

pub struct Shared {
    pub revision: Arc<AtomicU64>,
    pub dirty: AtomicBool,
    pub exited: AtomicBool,
    pub title: Mutex<String>,
    pub error: Mutex<Option<String>>,
    stop: AtomicBool,
    wake: Wake,
}
impl Shared {
    pub fn notify(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
    fn fail(&self, message: impl Into<String>) {
        *self.error.lock().unwrap() = Some(message.into());
        self.notify();
    }
}

enum Command {
    Input(Vec<u8>),
    Resize(Size),
}

#[derive(Clone)]
pub struct Listener {
    tx: SyncSender<Command>,
    signal: Arc<UnixStream>,
    shared: Arc<Shared>,
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if self.tx.try_send(Command::Input(text.into_bytes())).is_err() {
                    self.shared.fail(
                        "PTY reply queue full; session stopped to avoid protocol desynchronization",
                    );
                    self.shared.stop.store(true, Ordering::Release);
                }
                signal(&self.signal);
            }
            Event::Title(title) => {
                *self.shared.title.lock().unwrap() = title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect()
            }
            Event::ResetTitle => *self.shared.title.lock().unwrap() = "vibeterm".into(),
            _ => (),
        }
    }
}

pub struct Session {
    pub terminal: Arc<FairMutex<Terminal>>,
    pub shared: Arc<Shared>,
    tx: SyncSender<Command>,
    signal: Arc<UnixStream>,
    pid: i32,
    worker: Option<JoinHandle<()>>,
}

pub fn engine_config(config: &Config) -> term::Config {
    term::Config {
        scrolling_history: config.scrollback,
        default_cursor_style: ansi::CursorStyle {
            shape: match config.cursor_shape {
                CursorShape::Block => ansi::CursorShape::Block,
                CursorShape::Beam => ansi::CursorShape::Beam,
                CursorShape::Underline => ansi::CursorShape::Underline,
            },
            blinking: config.cursor_blink,
        },
        kitty_keyboard: false,
        osc52: term::Osc52::Disabled,
        ..Default::default()
    }
}

fn signal(stream: &UnixStream) {
    let _ = (&*stream).write(&[1]);
}

impl Session {
    pub fn spawn(
        config: &Config,
        size: Size,
        command: &[OsString],
        cwd: Option<&Path>,
        wake: Wake,
    ) -> Result<Self> {
        let pair = portable_pty::native_pty_system()
            .openpty(size.pty())
            .context("Create PTY")?;
        let mut cmd = if let Some(program) = command.first() {
            let mut cmd = CommandBuilder::new(program);
            cmd.args(&command[1..]);
            cmd
        } else {
            CommandBuilder::new(
                config
                    .shell
                    .clone()
                    .map(OsString::from)
                    .or_else(|| std::env::var_os("SHELL"))
                    .unwrap_or_else(|| "/bin/sh".into()),
            )
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "vibeterm");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.cwd(
            cwd.map(std::path::PathBuf::from)
                .unwrap_or(std::env::current_dir()?),
        );
        let (mut signal_tx, mut signal_rx) = UnixStream::pair()?;
        signal_tx.set_nonblocking(true)?;
        signal_rx.set_nonblocking(true)?;
        // Fail before spawning a child if the host sandbox prohibits wake-channel I/O.
        signal_tx
            .write_all(&[0])
            .context("PTY wake channel is blocked by the host sandbox")?;
        signal_rx
            .read_exact(&mut [0])
            .context("Read PTY wake channel")?;
        let signal_tx = Arc::new(signal_tx);
        let fd = pair
            .master
            .as_raw_fd()
            .context("Linux PTY has no file descriptor")?;
        // SAFETY: fd is owned by master for the lifetime of the reactor; flags are preserved.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Launch shell/command")?;
        drop(pair.slave);
        let pid = child.process_id().context("Missing child PID")? as i32;
        let (tx, rx) = mpsc::sync_channel(32);
        let shared = Arc::new(Shared {
            revision: Arc::new(AtomicU64::new(0)),
            dirty: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            title: Mutex::new("vibeterm".into()),
            error: Mutex::new(None),
            wake,
        });
        let listener = Listener {
            tx: tx.clone(),
            signal: signal_tx.clone(),
            shared: shared.clone(),
        };
        let terminal = Arc::new(FairMutex::new(Term::new(
            engine_config(config),
            &size,
            listener,
        )));
        let term = terminal.clone();
        let state = shared.clone();
        let child_done = Arc::new(AtomicBool::new(false));
        let done = child_done.clone();
        let child_signal = signal_tx.clone();
        let waiter = thread::spawn(move || {
            // Observe exit without reaping. The reactor retains the child PID until
            // cleanup, so a concurrent close can never signal a reused process ID.
            let result = loop {
                let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
                // SAFETY: waitid initializes siginfo on success; WNOWAIT retains ownership.
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as _,
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if result == 0 {
                    break Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    break Err(error);
                }
            };
            done.store(true, Ordering::Release);
            signal(&child_signal);
            result
        });
        let worker = thread::spawn(move || {
            let master = pair.master;
            let mut parser = Processor::<ansi::StdSyncHandler>::new();
            let mut pending = Vec::new();
            let mut offset = 0;
            let mut bytes = [0u8; 16384];
            loop {
                if state.stop.load(Ordering::Acquire) {
                    break;
                }
                if pending.is_empty() {
                    // Cap work per iteration so input cannot starve output.
                    for _ in 0..32 {
                        match rx.try_recv() {
                            Ok(Command::Input(data)) => {
                                pending = data;
                                offset = 0;
                                if !pending.is_empty() {
                                    break;
                                }
                            }
                            Ok(Command::Resize(size)) => {
                                let mut terminal = term.lock();
                                terminal.resize(size);
                                state.revision.fetch_add(1, Ordering::Release);
                                if let Err(error) = master.resize(size.pty()) {
                                    state.fail(format!("PTY resize: {error}"));
                                }
                                drop(terminal);
                                state.notify();
                            }
                            Err(_) => break,
                        }
                    }
                }
                let timeout = parser
                    .sync_timeout()
                    .sync_timeout()
                    .map(|deadline| {
                        deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(i32::MAX as u128) as i32
                    })
                    .unwrap_or(-1);
                let timeout = if child_done.load(Ordering::Acquire) {
                    0
                } else {
                    timeout
                };
                let mut fds = [
                    libc::pollfd {
                        fd,
                        events: libc::POLLIN | if pending.is_empty() { 0 } else { libc::POLLOUT },
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: signal_rx.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: poll receives two live descriptors and the array's exact length.
                let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    state.fail("PTY poll failed");
                    break;
                }
                if fds[1].revents & libc::POLLIN != 0 {
                    // SAFETY: valid writable buffer and live wake descriptor.
                    while unsafe {
                        libc::read(
                            signal_rx.as_raw_fd(),
                            bytes.as_mut_ptr().cast(),
                            bytes.len(),
                        )
                    } > 0
                    {}
                }
                if fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                    // SAFETY: the master outlives this read, and bytes is writable for its full length.
                    let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
                    let error = std::io::Error::last_os_error();
                    if count > 0 {
                        {
                            let mut terminal = term.lock();
                            parser.advance(&mut *terminal, &bytes[..count as usize]);
                            state.revision.fetch_add(1, Ordering::Release);
                        }
                        state.notify();
                    } else if count == 0 || error.raw_os_error() == Some(libc::EIO) {
                        break;
                    } else if !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) {
                        state.fail("PTY read failed");
                        break;
                    }
                } else if child_done.load(Ordering::Acquire) {
                    break;
                }
                if fds[0].revents & libc::POLLOUT != 0 && !pending.is_empty() {
                    let end = (offset + 16384).min(pending.len());
                    // SAFETY: this slice is readable and the master descriptor is live.
                    let count = unsafe {
                        libc::write(fd, pending[offset..end].as_ptr().cast(), end - offset)
                    };
                    if count > 0 {
                        offset += count as usize;
                        if offset == pending.len() {
                            pending.clear();
                        }
                    } else if count < 0
                        && !matches!(
                            std::io::Error::last_os_error().kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        )
                    {
                        state.fail("PTY write failed");
                        break;
                    }
                }
                if parser
                    .sync_timeout()
                    .sync_timeout()
                    .is_some_and(|deadline| deadline <= Instant::now())
                {
                    {
                        let mut terminal = term.lock();
                        parser.stop_sync(&mut *terminal);
                        state.revision.fetch_add(1, Ordering::Release);
                    }
                    state.notify();
                }
            }
            {
                let mut terminal = term.lock();
                parser.stop_sync(&mut *terminal);
                state.revision.fetch_add(1, Ordering::Release);
            }
            if !child_done.load(Ordering::Acquire) {
                // Closing a session hangs up the foreground process group and shell.
                // SAFETY: positive IDs originate from this PTY; never signal our own group.
                unsafe {
                    if let Some(group) = master
                        .process_group_leader()
                        .filter(|g| *g > 1 && *g != libc::getpgrp())
                    {
                        libc::kill(-group, libc::SIGHUP);
                    }
                    libc::kill(pid, libc::SIGHUP);
                }
                let deadline = Instant::now() + Duration::from_millis(200);
                while !child_done.load(Ordering::Acquire) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                if !child_done.load(Ordering::Acquire) {
                    // SAFETY: the owned child has not been reaped; SIGKILL bounds shutdown.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            drop(master);
            if let Ok(Err(error)) = waiter.join() {
                state.fail(format!("Observe child exit: {error}"));
            }
            if let Err(error) = child.wait() {
                state.fail(format!("Wait for child: {error}"));
            }
            state.exited.store(true, Ordering::Release);
            // Exit must wake the UI even when a hidden pane already has pending damage.
            (state.wake)();
        });
        Ok(Self {
            terminal,
            shared,
            tx,
            signal: signal_tx,
            pid,
            worker: Some(worker),
        })
    }

    pub fn input(&self, data: Vec<u8>) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if data.len() > MAX_INPUT {
            bail!("Input exceeds the 256 KiB paste limit");
        }
        self.tx.try_send(Command::Input(data)).map_err(|_| {
            anyhow::anyhow!("Input queue full or session closed; input was not sent")
        })?;
        signal(&self.signal);
        Ok(())
    }

    /// The child's current directory, read from procfs. The reactor keeps the child
    /// unreaped until `exited` is set, so the PID cannot belong to another process here.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        if self.shared.exited.load(Ordering::Acquire) {
            return None;
        }
        std::fs::read_link(format!("/proc/{}/cwd", self.pid))
            .ok()
            .filter(|dir| dir.is_dir())
    }

    pub fn configure(&self, config: &Config) {
        let mut terminal = self.terminal.lock();
        terminal.set_options(engine_config(config));
        self.shared.revision.fetch_add(1, Ordering::Release);
    }

    pub fn resize(&self, size: Size) -> Result<()> {
        self.tx
            .try_send(Command::Resize(size))
            .map_err(|_| anyhow::anyhow!("PTY resize queue full"))?;
        signal(&self.signal);
        Ok(())
    }

    pub fn close(&self) {
        self.shared.stop.store(true, Ordering::Release);
        signal(&self.signal);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
