//! Cancellable, bounded-history search. Engine locks cover small ranges, never a full history scan.
use crate::session::Terminal;
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point},
    sync::FairMutex,
    term::search::RegexSearch,
};
use std::{
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
};
pub type Match = RangeInclusive<Point>;
pub struct Request {
    pub token: u64,
    pub pane: u64,
    pub query: String,
    pub origin: Point,
    pub backward: bool,
    pub terminal: Arc<FairMutex<Terminal>>,
    pub revision: Arc<AtomicU64>,
}
pub struct Response {
    pub token: u64,
    pub pane: u64,
    pub revision: u64,
    pub result: Result<Option<Match>, String>,
}
pub struct SearchWorker {
    tx: SyncSender<Request>,
    pub generation: Arc<AtomicU64>,
}
impl SearchWorker {
    pub fn new(callback: impl Fn(Response) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Request>(8);
        let generation = Arc::new(AtomicU64::new(0));
        let cancel = generation.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    request = newer;
                }
                if cancel.load(Ordering::Acquire) != request.token {
                    continue;
                }
                let revision = request.revision.load(Ordering::Acquire);
                let result = find(&request, &cancel, revision);
                if cancel.load(Ordering::Acquire) == request.token {
                    callback(Response {
                        token: request.token,
                        pane: request.pane,
                        revision,
                        result,
                    });
                }
            }
        });
        Self { tx, generation }
    }
    pub fn submit(&self, request: Request) -> Result<(), String> {
        self.tx
            .try_send(request)
            .map_err(|_| "Search busy; press Enter to retry".into())
    }
    pub fn cancel(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
}
fn literal_pattern(query: &str) -> String {
    let mut pattern = String::new();
    for c in query.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}
fn find(request: &Request, cancel: &AtomicU64, revision: u64) -> Result<Option<Match>, String> {
    let mut regex =
        RegexSearch::new(&literal_pattern(&request.query)).map_err(|e| e.to_string())?;
    let (top, bottom, cols) = {
        let term = request.terminal.lock();
        (
            term.topmost_line().0,
            term.bottommost_line().0,
            term.columns(),
        )
    };
    let origin = Point::new(
        Line(request.origin.line.0.clamp(top, bottom)),
        Column(request.origin.column.0.min(cols - 1)),
    );
    let first = Point::new(Line(top), Column(0));
    let last = Point::new(Line(bottom), Column(cols - 1));
    let segments = if request.backward {
        [(origin, first), (last, origin)]
    } else {
        [(origin, last), (first, origin)]
    };
    let overlap = (request.query.chars().count() * 2).div_ceil(cols) as i32 + 2;
    for (mut start, limit) in segments {
        loop {
            if cancel.load(Ordering::Acquire) != request.token {
                return Ok(None);
            }
            let term = request.terminal.lock();
            if request.revision.load(Ordering::Acquire) != revision {
                return Err("Output changed; press Enter to search again".into());
            }
            let end = if request.backward {
                let line = (start.line.0 - 64 - overlap).max(limit.line.0);
                Point::new(
                    Line(line),
                    if line == limit.line.0 {
                        limit.column
                    } else {
                        Column(0)
                    },
                )
            } else {
                let line = (start.line.0 + 64 + overlap).min(limit.line.0);
                Point::new(
                    Line(line),
                    if line == limit.line.0 {
                        limit.column
                    } else {
                        Column(cols - 1)
                    },
                )
            };
            let found = if request.backward {
                term.regex_search_left(&mut regex, start, end)
            } else {
                term.regex_search_right(&mut regex, start, end)
            };
            drop(term);
            if found.is_some() {
                return Ok(found);
            }
            if end == limit {
                break;
            }
            start = if request.backward {
                Point::new(Line(start.line.0 - 64), Column(cols - 1))
            } else {
                Point::new(Line(start.line.0 + 64), Column(0))
            };
            std::thread::yield_now();
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_queries_escape_regex_operators() {
        assert_eq!(literal_pattern("[a].*\\"), "\\[a\\]\\.\\*\\\\");
    }
}
