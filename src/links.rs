//! Bounded link detection and argument-based external opening (never through a shell).
use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Term, cell::Flags},
};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const MAX_CELLS: usize = 8192;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    File {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: Target,
    pub start: Point,
    pub end: Point,
}
impl Target {
    pub fn label(&self) -> String {
        match self {
            Self::Url(url) => url.clone(),
            Self::File { path, line, column } => {
                let mut label = path.display().to_string();
                if let Some(line) = line {
                    label.push_str(&format!(":{line}"));
                }
                if let Some(column) = column {
                    label.push_str(&format!(":{column}"));
                }
                label
            }
        }
    }
}
fn url(text: &str) -> Option<Target> {
    if text.len() > MAX_CELLS {
        return None;
    }
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty() && !text.chars().any(|c| c.is_whitespace() || c.is_control()))
        .then(|| Target::Url(text.to_owned()))
}
fn target(text: &str) -> Option<Target> {
    if text.chars().any(char::is_control) || text.is_empty() {
        return None;
    }
    if text.contains("://") {
        return url(text);
    }
    let mut path = text;
    let mut numbers = Vec::new();
    for _ in 0..2 {
        let Some((head, tail)) = path.rsplit_once(':') else {
            break;
        };
        let Ok(number) = tail.parse::<u32>() else {
            break;
        };
        if number == 0 || !tail.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        numbers.push(number);
        path = head;
    }
    // Avoid treating ordinary words and arbitrary URI schemes as local files.
    if path.contains(':')
        || path.is_empty()
        || !(path.contains('/') || Path::new(path).extension().is_some())
    {
        return None;
    }
    let (line, column) = match numbers.as_slice() {
        [column, line] => (Some(*line), Some(*column)),
        [line] => (Some(*line), None),
        _ => (None, None),
    };
    Some(Target::File {
        path: path.into(),
        line,
        column,
    })
}
fn trimmed(text: &str, start: usize, end: usize) -> (usize, usize) {
    let value = &text[start..end];
    let value = value.trim_start_matches(['(', '[', '{']);
    let start = end - value.len();
    let mut value = value.trim_end_matches(['.', ',', ';', ':', '!']);
    while let Some(last) = value.chars().last() {
        let open = match last {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            _ => break,
        };
        if value.matches(last).count() <= value.matches(open).count() {
            break;
        }
        value = &value[..value.len() - last.len_utf8()];
    }
    (start, start + value.len())
}
fn candidate(text: &str, byte: usize) -> Option<(Target, usize, usize)> {
    // Quoted file paths can contain spaces. Match only complete quote pairs.
    for quote in ['\'', '"', '`'] {
        let mut positions = text.match_indices(quote).map(|(i, _)| i);
        while let (Some(start), Some(end)) = (positions.next(), positions.next()) {
            if byte > start && byte < end {
                let value = &text[start + 1..end];
                if let Some(target) = target(value) {
                    return Some((target, start + 1, end));
                }
            }
        }
    }
    let boundary = |c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '<' | '>');
    let start = text[..byte]
        .rfind(boundary)
        .map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
    let end = text[byte..].find(boundary).map_or(text.len(), |i| byte + i);
    let (start, end) = trimmed(text, start, end);
    if byte < start || byte >= end {
        return None;
    }
    Some((target(&text[start..end])?, start, end))
}

/// Inspect the logical line under a cell, joining soft wraps and preserving Unicode mapping.
/// Work is bounded even for maliciously long wrapped output. No filesystem I/O here.
pub fn at<T: EventListener>(term: &Term<T>, point: Point) -> Option<Link> {
    let first = -(term.history_size() as i32);
    let last = term.screen_lines() as i32 - 1;
    let columns = term.columns();
    if point.line.0 < first || point.line.0 > last || point.column.0 >= columns {
        return None;
    }
    let grid = term.grid();
    let clicked = &grid[point.line][point.column];
    if clicked.flags.contains(Flags::HIDDEN) {
        return None;
    }
    let explicit = clicked.hyperlink();
    if let Some(link) = &explicit {
        url(link.uri())?;
    }
    let max_rows = (MAX_CELLS / columns.max(1)).max(1);
    if columns > MAX_CELLS {
        return None;
    }
    let mut begin = point.line.0;
    while begin > first
        && point.line.0 - begin < max_rows as i32 - 1
        && grid[Line(begin - 1)][Column(columns - 1)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        begin -= 1;
    }
    // Do not open a truncated prefix of a longer logical line.
    if begin > first
        && grid[Line(begin - 1)][Column(columns - 1)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        return None;
    }
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut hit = None;
    for row in begin..=last.min(begin + max_rows as i32 - 1) {
        for col in 0..columns {
            let position = Point::new(Line(row), Column(col));
            let cell = &grid[Line(row)][Column(col)];
            if position == point {
                hit = Some(text.len());
            }
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                if position == point {
                    hit = cells.last().map(|&(byte, _)| byte);
                }
                continue;
            }
            cells.push((text.len(), position));
            let hidden = cell.flags.contains(Flags::HIDDEN);
            let matches_link = explicit
                .as_ref()
                .is_none_or(|link| cell.hyperlink().as_ref() == Some(link));
            text.push(if hidden || !matches_link { ' ' } else { cell.c });
            if !hidden
                && matches_link
                && let Some(marks) = cell.zerowidth()
            {
                text.extend(marks);
            }
            if text.len() > MAX_CELLS * 4 {
                return None;
            }
        }
        if !grid[Line(row)][Column(columns - 1)]
            .flags
            .contains(Flags::WRAPLINE)
        {
            break;
        }
        if row == last.min(begin + max_rows as i32 - 1) {
            return None;
        }
    }
    let hit = hit?;
    if let Some(explicit) = explicit {
        // OSC 8 labels may include spaces; restrict the highlight to contiguous cells.
        let index = cells.iter().position(|&(byte, _)| byte == hit)?;
        let matches = |p: Point| grid[p.line][p.column].hyperlink().as_ref() == Some(&explicit);
        let mut start = index;
        let mut end = index;
        while start > 0 && matches(cells[start - 1].1) {
            start -= 1;
        }
        while end + 1 < cells.len() && matches(cells[end + 1].1) {
            end += 1;
        }
        return Some(Link {
            target: url(explicit.uri())?,
            start: cells[start].1,
            end: cells[end].1,
        });
    }
    let (target, start, end) = candidate(&text, hit)?;
    Some(Link {
        target,
        start: cells.iter().find(|&&(byte, _)| byte >= start)?.1,
        end: cells.iter().rev().find(|&&(byte, _)| byte < end)?.1,
    })
}

/// Build argv separately so punctuation in output cannot become a shell command.
pub fn command(target: &Target, cwd: &Path, editor: &[String]) -> Result<Command> {
    let mut command = match target {
        Target::Url(uri) => {
            ensure!(url(uri).is_some(), "Unsupported link scheme");
            let mut command = Command::new("xdg-open");
            command.arg(uri);
            command
        }
        Target::File { path, line, column } => {
            let path = if let Ok(suffix) = path.strip_prefix("~/") {
                PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?).join(suffix)
            } else if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let path = path
                .canonicalize()
                .with_context(|| format!("Cannot open {}", path.display()))?;
            if editor.is_empty() {
                let mut command = Command::new("xdg-open");
                command.arg(&path);
                command
            } else {
                let mut command = Command::new(&editor[0]);
                let path = path.to_str().context("File path is not UTF-8")?;
                for argument in &editor[1..] {
                    // Replace template fields only in the original argument, never in filenames.
                    let mut expanded = String::new();
                    let mut rest = argument.as_str();
                    while !rest.is_empty() {
                        if let Some(tail) = rest.strip_prefix("{path}") {
                            expanded.push_str(path);
                            rest = tail;
                        } else if let Some(tail) = rest.strip_prefix("{line}") {
                            expanded.push_str(&line.unwrap_or(1).to_string());
                            rest = tail;
                        } else if let Some(tail) = rest.strip_prefix("{column}") {
                            expanded.push_str(&column.unwrap_or(1).to_string());
                            rest = tail;
                        } else {
                            let c = rest.chars().next().unwrap();
                            expanded.push(c);
                            rest = &rest[c.len_utf8()..];
                        }
                    }
                    command.arg(expanded);
                }
                command
            }
        }
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}
