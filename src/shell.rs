//! Shell hooks and bounded command tracking. The private OSC 2 marker transport
//! uses the engine's existing parser, including its synchronized-update buffering.
use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{Term, TermMode},
    vte::ansi::{self, Handler},
};
use portable_pty::CommandBuilder;
use std::{collections::VecDeque, ffi::OsStr, path::Path};
use unicode_width::UnicodeWidthChar;

const PREFIX: &str = "__vibeterm_shell__;";
const MAX_COMMANDS: usize = 1024;
const MAX_OUTPUT: usize = 1024 * 1024;

/// Temporary startup files live as long as the session; no user files are edited.
pub fn prepare(
    cmd: &mut CommandBuilder,
    program: &OsStr,
) -> anyhow::Result<Option<tempfile::TempDir>> {
    let name = Path::new(program).file_name().and_then(OsStr::to_str);
    if !matches!(name, Some("bash" | "zsh")) {
        return Ok(None);
    }
    let dir = tempfile::Builder::new()
        .prefix("vibeterm-shell-")
        .tempdir()?;
    match name {
        Some("bash") => {
            let path = dir.path().join("bashrc");
            std::fs::write(
                &path,
                concat!(
                    "[[ -r ~/.bashrc ]] && source ~/.bashrc\n",
                    include_str!("../shell-integration/bash.bash")
                ),
            )?;
            cmd.arg("--rcfile");
            cmd.arg(path);
            cmd.arg("-i");
        }
        Some("zsh") => {
            std::fs::write(
                dir.path().join(".zshenv"),
                include_str!("../shell-integration/zshenv.zsh"),
            )?;
            std::fs::write(
                dir.path().join(".zshrc"),
                include_str!("../shell-integration/zshrc.zsh"),
            )?;
            let original_zdotdir = cmd.get_env("ZDOTDIR").map(OsStr::to_os_string);
            cmd.env(
                "VIBETERM_ZDOTDIR",
                original_zdotdir.as_deref().unwrap_or_default(),
            );
            cmd.env(
                "VIBETERM_ZDOTDIR_SET",
                if original_zdotdir.is_some() { "1" } else { "0" },
            );
            cmd.env("ZDOTDIR", dir.path());
            cmd.arg("-i");
        }
        _ => unreachable!(),
    }
    Ok(Some(dir))
}

#[derive(Default)]
pub struct History {
    // Absolute row numbers avoid touching every mark on every linefeed.
    origin: i64,
    commands: VecDeque<i64>,
    prompt: Option<i64>,
    output_start: Option<(i64, Column)>,
    running: bool,
    last_output: Option<String>,
    navigation: Option<i64>,
    restricted_region: bool,
}
impl History {
    /// Reflow/editing can make old coordinates ambiguous. Keep the completed copy,
    /// discard coordinates rather than ever navigating/copying an unrelated range.
    pub fn invalidate(&mut self) {
        self.commands.clear();
        self.prompt = None;
        self.output_start = None;
        self.navigation = None;
    }
    pub fn resized(&mut self) {
        self.invalidate();
        self.restricted_region = false;
    }
    pub fn last_output(&self) -> Option<&str> {
        self.last_output.as_deref()
    }
    fn prune<T>(&mut self, term: &Term<T>) {
        let oldest = self.origin - term.history_size() as i64;
        while self.commands.front().is_some_and(|line| *line < oldest) {
            self.commands.pop_front();
        }
    }
    fn marker<T: EventListener>(&mut self, term: &Term<T>, mark: &str) {
        if term.mode().contains(TermMode::ALT_SCREEN) || self.restricted_region {
            return;
        }
        self.prune(term);
        let point = term.grid().cursor.point;
        let line = self.origin + i64::from(point.line.0);
        match mark {
            "A" => self.prompt = Some(line),
            "C" => {
                if let Some(prompt) = self.prompt.take() {
                    if self.commands.back() != Some(&prompt) {
                        self.commands.push_back(prompt);
                    }
                    if self.commands.len() > MAX_COMMANDS {
                        self.commands.pop_front();
                    }
                }
                self.output_start = Some((line, point.column));
                self.running = true;
                self.navigation = None;
            }
            "D" if self.running => {
                self.running = false;
                self.last_output = self.output_start.take().and_then(|(line, column)| {
                    let row = line - self.origin;
                    if row < -(term.history_size() as i64) || row > i64::from(point.line.0) {
                        return None;
                    }
                    let start = Point::new(Line(row as i32), column);
                    // Bound the extraction before allocating. Combining characters can
                    // increase UTF-8 size further, so also check the resulting string.
                    let cells = (i64::from(point.line.0) - row + 1) as usize * term.columns();
                    if cells > MAX_OUTPUT / 4 || start > point {
                        return None;
                    }
                    if start == point && !term.grid().cursor.input_needs_wrap {
                        return Some(String::new());
                    }
                    let end = if term.grid().cursor.input_needs_wrap {
                        point
                    } else if point.column.0 == 0 {
                        Point::new(point.line - 1, Column(term.columns() - 1))
                    } else {
                        Point::new(point.line, point.column - 1)
                    };
                    // A cell can contain arbitrarily many combining characters.
                    // Bound that allocation too, before calling the engine extractor.
                    let mut bytes = 0usize;
                    for row in start.line.0..=end.line.0 {
                        let left = if row == start.line.0 {
                            start.column.0
                        } else {
                            0
                        };
                        let right = if row == end.line.0 {
                            end.column.0
                        } else {
                            term.columns() - 1
                        };
                        for col in left..=right {
                            let cell = &term.grid()[Line(row)][Column(col)];
                            bytes = bytes.saturating_add(
                                4usize
                                    .saturating_mul(1 + cell.zerowidth().map_or(0, <[char]>::len)),
                            );
                            if bytes > MAX_OUTPUT {
                                return None;
                            }
                        }
                        bytes = bytes.saturating_add(1);
                    }
                    if bytes > MAX_OUTPUT {
                        return None;
                    }
                    let mut text = term.bounds_to_string(start, end);
                    if point.column.0 == 0 {
                        text.push('\n');
                    }
                    (text.len() <= MAX_OUTPUT).then_some(text)
                });
            }
            _ => (),
        }
    }
    pub fn navigate<T: EventListener>(&mut self, term: &mut Term<T>, previous: bool) -> bool {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        self.prune(term);
        let top = self.origin - term.grid().display_offset() as i64;
        let from = self
            .navigation
            .filter(|line| {
                let expected = (self.origin - line).max(0) as usize;
                expected.min(term.history_size()) == term.grid().display_offset()
            })
            .unwrap_or(if term.grid().display_offset() == 0 {
                i64::MAX
            } else {
                top
            });
        let target = if previous {
            self.commands.iter().rev().find(|line| **line < from)
        } else {
            self.commands.iter().find(|line| **line > from)
        }
        .copied();
        if let Some(line) = target {
            let offset = (self.origin - line).max(0).min(term.history_size() as i64) as i32;
            let delta = offset - term.grid().display_offset() as i32;
            term.scroll_display(Scroll::Delta(delta));
            self.navigation = Some(line);
            true
        } else if !previous {
            term.scroll_display(Scroll::Bottom);
            self.navigation = None;
            true
        } else {
            false
        }
    }
}

/// Delegate all ANSI behavior to Alacritty, observing only operations that move
/// tracked rows. Both locks are acquired in terminal-then-history order.
pub struct Tracked<'a, T> {
    pub term: &'a mut Term<T>,
    pub history: &'a mut History,
}
impl<T: EventListener> Tracked<'_, T> {
    fn scroll(&mut self, rows: usize) {
        if !self.term.mode().contains(TermMode::ALT_SCREEN) && !self.history.restricted_region {
            self.history.origin += rows.min(self.term.screen_lines()) as i64;
        }
    }
    fn will_linefeed(&mut self) {
        if self.term.grid().cursor.point.line.0 == self.term.screen_lines() as i32 - 1 {
            self.scroll(1);
        }
    }
    fn invalidate_main(&mut self) {
        if !self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.history.invalidate();
        }
    }
}
macro_rules! forward {
    ($(fn $name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $name(&mut self, $($arg: $ty),*) { self.term.$name($($arg),*); }
    )*};
}
impl<T: EventListener> Handler for Tracked<'_, T> {
    fn set_title(&mut self, title: Option<String>) {
        if let Some(mark) = title.as_deref().and_then(|s| s.strip_prefix(PREFIX)) {
            self.history.marker(self.term, mark);
        } else {
            self.term.set_title(title);
        }
    }
    #[inline]
    fn input(&mut self, c: char) {
        let cursor = &self.term.grid().cursor;
        if (cursor.input_needs_wrap || cursor.point.column.0 + 1 >= self.term.columns())
            && cursor.point.line.0 == self.term.screen_lines() as i32 - 1
            && self.term.mode().contains(TermMode::LINE_WRAP)
            && c.width()
                .is_some_and(|width| width > 0 && (cursor.input_needs_wrap || width == 2))
        {
            self.will_linefeed();
        }
        self.term.input(c);
    }
    fn linefeed(&mut self) {
        self.will_linefeed();
        self.term.linefeed();
    }
    fn newline(&mut self) {
        self.will_linefeed();
        self.term.newline();
    }
    fn scroll_up(&mut self, rows: usize) {
        self.scroll(rows);
        self.term.scroll_up(rows);
    }
    fn scroll_down(&mut self, rows: usize) {
        self.invalidate_main();
        self.term.scroll_down(rows);
    }
    fn insert_blank_lines(&mut self, rows: usize) {
        self.invalidate_main();
        self.term.insert_blank_lines(rows);
    }
    fn delete_lines(&mut self, rows: usize) {
        self.invalidate_main();
        self.term.delete_lines(rows);
    }
    fn reverse_index(&mut self) {
        self.invalidate_main();
        self.term.reverse_index();
    }
    fn clear_screen(&mut self, mode: ansi::ClearMode) {
        self.invalidate_main();
        self.term.clear_screen(mode);
    }
    fn decaln(&mut self) {
        self.invalidate_main();
        self.term.decaln();
    }
    fn reset_state(&mut self) {
        self.history.resized();
        self.term.reset_state();
    }
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        if top < bottom.unwrap_or(self.term.screen_lines()) {
            self.history.invalidate();
            self.history.restricted_region =
                top > 1 || bottom.is_some_and(|b| b != self.term.screen_lines());
        }
        self.term.set_scrolling_region(top, bottom);
    }
    fn set_private_mode(&mut self, mode: ansi::PrivateMode) {
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        self.term.set_private_mode(mode);
        if !alt && self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.history.output_start = None;
        }
    }
    forward! {
        fn set_cursor_style(arg0: Option<ansi::CursorStyle>);
        fn set_cursor_shape(arg0: ansi::CursorShape);
        fn goto(arg0: i32, arg1: usize);
        fn goto_line(arg0: i32);
        fn goto_col(arg0: usize);
        fn insert_blank(arg0: usize);
        fn move_up(arg0: usize);
        fn move_down(arg0: usize);
        fn identify_terminal(arg0: Option<char>);
        fn device_status(arg0: usize);
        fn move_forward(arg0: usize);
        fn move_backward(arg0: usize);
        fn move_down_and_cr(arg0: usize);
        fn move_up_and_cr(arg0: usize);
        fn put_tab(arg0: u16);
        fn backspace();
        fn carriage_return();
        fn bell();
        fn substitute();
        fn set_horizontal_tabstop();
        fn erase_chars(arg0: usize);
        fn delete_chars(arg0: usize);
        fn move_backward_tabs(arg0: u16);
        fn move_forward_tabs(arg0: u16);
        fn save_cursor_position();
        fn restore_cursor_position();
        fn clear_line(arg0: ansi::LineClearMode);
        fn clear_tabs(arg0: ansi::TabulationClearMode);
        fn set_tabs(arg0: u16);
        fn terminal_attribute(arg0: ansi::Attr);
        fn set_mode(arg0: ansi::Mode);
        fn unset_mode(arg0: ansi::Mode);
        fn report_mode(arg0: ansi::Mode);
        fn unset_private_mode(arg0: ansi::PrivateMode);
        fn report_private_mode(arg0: ansi::PrivateMode);
        fn set_keypad_application_mode();
        fn unset_keypad_application_mode();
        fn set_active_charset(arg0: ansi::CharsetIndex);
        fn configure_charset(arg0: ansi::CharsetIndex, arg1: ansi::StandardCharset);
        fn set_color(arg0: usize, arg1: ansi::Rgb);
        fn dynamic_color_sequence(arg0: String, arg1: usize, arg2: &str);
        fn reset_color(arg0: usize);
        fn clipboard_store(arg0: u8, arg1: &[u8]);
        fn clipboard_load(arg0: u8, arg1: &str);
        fn push_title();
        fn pop_title();
        fn text_area_size_pixels();
        fn text_area_size_chars();
        fn set_hyperlink(arg0: Option<ansi::Hyperlink>);
        fn set_mouse_cursor_icon(arg0: winit::window::CursorIcon);
        fn report_keyboard_mode();
        fn push_keyboard_mode(arg0: ansi::KeyboardModes);
        fn pop_keyboard_modes(arg0: u16);
        fn set_keyboard_mode(arg0: ansi::KeyboardModes, arg1: ansi::KeyboardModesApplyBehavior);
        fn set_modify_other_keys(arg0: ansi::ModifyOtherKeys);
        fn report_modify_other_keys();
        fn set_scp(arg0: ansi::ScpCharPath, arg1: ansi::ScpUpdateMode);
    }
}
