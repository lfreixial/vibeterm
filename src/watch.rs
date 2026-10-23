//! Linux inotify configuration watcher: no periodic UI polling or UI-thread file reads.
use crate::config::Config;
use anyhow::{Context, Result};
use std::{
    ffi::CString,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub struct ConfigWatcher {
    stop: UnixStream,
    worker: Option<JoinHandle<()>>,
}
fn parent(path: &Path) -> PathBuf {
    let mut dir = path.parent().unwrap_or(Path::new("/"));
    while !dir.is_dir() {
        dir = dir.parent().unwrap_or(Path::new("/"));
    }
    dir.to_path_buf()
}
fn contents(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut data = Vec::new();
            file.take(65537)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())?;
            if data.len() > 65536 {
                return Err("Configuration exceeds 64 KiB".into());
            }
            Ok(data)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}
impl ConfigWatcher {
    pub fn start(
        path: PathBuf,
        callback: impl Fn(Result<Config, String>) + Send + 'static,
    ) -> Result<Self> {
        // SAFETY: inotify_init1 returns a new owned fd or -1; flags require no pointers.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        anyhow::ensure!(
            fd >= 0,
            "Cannot initialize config watcher: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: ownership of the fresh descriptor transfers exactly once.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let (mut stop, mut receive) = UnixStream::pair()?;
        stop.set_nonblocking(true)?;
        receive.set_nonblocking(true)?;
        stop.write_all(&[0])
            .context("Config watcher wake channel blocked")?;
        receive.read_exact(&mut [0])?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let mut ready = Some(ready_tx);
            let mut watched = PathBuf::new();
            let mut watch = -1;
            let mut previous = contents(&path);
            let mut deadline = None;
            loop {
                let dir = parent(&path);
                if dir != watched {
                    let Ok(name) = CString::new(dir.as_os_str().as_bytes()) else {
                        break;
                    };
                    // SAFETY: valid inotify fd and NUL-terminated path, owned throughout this call.
                    let next = unsafe {
                        libc::inotify_add_watch(
                            fd.as_raw_fd(),
                            name.as_ptr(),
                            libc::IN_CLOSE_WRITE
                                | libc::IN_MOVED_TO
                                | libc::IN_MOVED_FROM
                                | libc::IN_CREATE
                                | libc::IN_DELETE
                                | libc::IN_DELETE_SELF
                                | libc::IN_MOVE_SELF,
                        )
                    };
                    if next < 0 {
                        callback(Err(format!(
                            "Config watcher: {}",
                            std::io::Error::last_os_error()
                        )));
                        break;
                    }
                    if watch >= 0 && watch != next {
                        unsafe {
                            libc::inotify_rm_watch(fd.as_raw_fd(), watch);
                        }
                    }
                    watch = next;
                    watched = dir;
                }
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                let timeout = deadline
                    .map(|d: Instant| {
                        d.saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(150) as i32
                    })
                    .unwrap_or(-1);
                let mut fds = [
                    libc::pollfd {
                        fd: fd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: receive.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: descriptors and array are live for this blocking poll.
                if unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) } < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if fds[1].revents != 0 {
                    break;
                }
                if fds[0].revents != 0 {
                    let mut bytes = [0u8; 8192];
                    // Ignore unrelated files, so activity in the config directory cannot
                    // indefinitely postpone this file's debounce.
                    let target = path
                        .strip_prefix(&watched)
                        .ok()
                        .and_then(|p| p.components().next())
                        .map(|p| p.as_os_str().as_bytes().to_vec());
                    loop {
                        // SAFETY: bytes is writable for the provided length and fd is live.
                        let count = unsafe {
                            libc::read(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len())
                        };
                        if count <= 0 {
                            break;
                        }
                        let mut offset = 0;
                        let header = std::mem::size_of::<libc::inotify_event>();
                        while offset + header <= count as usize {
                            // SAFETY: bounds were checked; the byte buffer need not be aligned.
                            let event = unsafe {
                                std::ptr::read_unaligned(
                                    bytes.as_ptr().add(offset).cast::<libc::inotify_event>(),
                                )
                            };
                            let end = offset + header + event.len as usize;
                            if end > count as usize {
                                break;
                            }
                            let name = &bytes[offset + header..end];
                            let name = name.split(|b| *b == 0).next().unwrap_or_default();
                            if event.wd != watch && event.mask & libc::IN_Q_OVERFLOW == 0 {
                                offset = end;
                                continue;
                            }
                            if event.mask
                                & (libc::IN_DELETE_SELF
                                    | libc::IN_MOVE_SELF
                                    | libc::IN_IGNORED
                                    | libc::IN_Q_OVERFLOW)
                                != 0
                            {
                                watched.clear();
                                if event.mask & libc::IN_IGNORED != 0 {
                                    watch = -1;
                                }
                                deadline = Some(Instant::now() + Duration::from_millis(150));
                            } else if target.as_deref() == Some(name) {
                                deadline = Some(Instant::now() + Duration::from_millis(150));
                            }
                            offset = end;
                        }
                    }
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    deadline = None;
                    let current = contents(&path);
                    if current != previous {
                        let parsed = current
                            .as_ref()
                            .map_err(Clone::clone)
                            .and_then(|data| std::str::from_utf8(data).map_err(|e| e.to_string()))
                            .and_then(|source| Config::parse(source).map_err(|e| e.to_string()));
                        previous = current;
                        callback(parsed);
                    }
                }
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .context("Configuration watcher did not become ready")?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        let _ = self.stop.write(&[1]);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
