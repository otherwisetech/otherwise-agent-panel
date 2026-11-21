//! One PTY session: the child (the agent CLI), alacritty's grid and parser, its event loop.
use alacritty_terminal::event::{Event, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::Hsla;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const SCROLLBACK: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dims {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// On top of ours, with PATH from the login shell.
    pub env: Vec<(String, String)>,
}

#[derive(Default)]
struct Shared {
    dirty: AtomicBool,
    events: Mutex<Vec<Event>>,
}

#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if !matches!(event, Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange) {
            self.0.events.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        }
        self.0.dirty.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub c: char,
    pub fg: Hsla,
    pub bg: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub cells: Vec<Vec<Option<Cell>>>,
    pub cursor: Option<(usize, usize, CursorShape)>,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    shared: Arc<Shared>,
    pub dims: Dims,
    pub title: String,
    pub exited: Option<i32>,
    #[cfg_attr(not(unix), allow(dead_code))]
    child_pid: u32,
    teardown: Option<EventLoopSender>,
    clipboard: Option<String>,
    colors: (Hsla, Hsla),
}

/// The child's environment: ours, PATH from the login shell, terminal variables, the caller's.
pub fn child_env(extra: &[(String, String)]) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("PATH".into(), crate::provider::child_path());
    for (k, v) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")] {
        env.insert(k.into(), v.into());
    }
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    env
}

impl Terminal {
    pub fn spawn(spec: &Spawn, dims: Dims, cell: (u16, u16)) -> std::io::Result<Self> {
        std::fs::create_dir_all(&spec.cwd)?;
        let options = tty::Options {
            shell: Some(tty::Shell::new(spec.program.display().to_string(), spec.args.clone())),
            working_directory: Some(spec.cwd.clone()),
            drain_on_exit: true,
            env: child_env(&spec.env),
            #[cfg(not(windows))]
            child_signal_mask: tty::SignalMask::current().ok(),
            #[cfg(windows)]
            escape_args: true,
        };
        let ws = WindowSize { num_lines: dims.rows as u16, num_cols: dims.cols as u16, cell_width: cell.0, cell_height: cell.1 };
        let pty = tty::new(&options, ws, 0)?;
        let shared = Arc::new(Shared::default());
        let listener = Listener(shared.clone());
        let term = Arc::new(FairMutex::new(Term::new(Config { scrolling_history: SCROLLBACK, ..Config::default() }, &dims, listener.clone())));
        #[cfg(unix)]
        let child_pid = pty.child().id();
        #[cfg(windows)]
        let child_pid = pty.child_watcher().pid().map_or(0, |p| p.get());
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let notifier = Notifier(event_loop.channel());
        let teardown = event_loop.channel();
        event_loop.spawn();
        Ok(Terminal { term, notifier, shared, dims, title: String::new(), exited: None, child_pid, teardown: Some(teardown), clipboard: None, colors: (Hsla::white(), Hsla::black()) })
    }

    pub fn set_colors(&mut self, fg: Hsla, bg: Hsla) {
        self.colors = (fg, bg);
    }

    /// OSC 52 text the child wants copied (write-only: reads would leak the clipboard).
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    pub fn take_dirty(&self) -> bool {
        self.shared.dirty.swap(false, Ordering::AcqRel)
    }

    /// Answer the child's queries, track title and exit.
    pub fn pump(&mut self) {
        let events: Vec<Event> = std::mem::take(&mut *self.shared.events.lock().unwrap_or_else(|e| e.into_inner()));
        for e in events {
            match e {
                Event::PtyWrite(s) => self.notifier.notify(s.into_bytes()),
                Event::Title(t) => self.title = t,
                Event::ResetTitle => self.title.clear(),
                Event::ColorRequest(idx, f) => {
                    let (fg, bg) = self.colors;
                    let rgb = self.term.lock().colors()[idx].unwrap_or_else(|| super::colors::query_reply(idx, fg, bg));
                    self.notifier.notify(f(rgb).into_bytes());
                }
                Event::TextAreaSizeRequest(f) => {
                    let ws = WindowSize { num_lines: self.dims.rows as u16, num_cols: self.dims.cols as u16, cell_width: 0, cell_height: 0 };
                    self.notifier.notify(f(ws).into_bytes());
                }
                Event::ClipboardStore(ClipboardType::Clipboard, text) => self.clipboard = Some(text),
                Event::ChildExit(status) => self.exited = Some(status.code().unwrap_or(-1)),
                Event::Exit if self.exited.is_none() => self.exited = Some(0),
                _ => {}
            }
        }
    }

    /// Typed or pasted bytes: drop the selection, jump to the bottom.
    pub fn input(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        {
            let mut t = self.term.lock();
            t.selection = None;
            t.scroll_display(Scroll::Bottom);
        }
        self.notifier.notify(bytes);
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let clean = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            // a pasted end marker would let the rest out of the bracket
            self.input(format!("\x1b[200~{}\x1b[201~", clean.replace("\x1b[201~", "")).into_bytes());
        } else {
            self.input(clean.into_bytes());
        }
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn start_selection(&self, ty: SelectionType, p: TermPoint, side: Side) {
        self.term.lock().selection = Some(Selection::new(ty, p, side));
    }

    pub fn update_selection(&self, p: TermPoint, side: Side) {
        if let Some(s) = self.term.lock().selection.as_mut() {
            s.update(p, side);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.trim().is_empty())
    }

    pub fn resize(&mut self, dims: Dims, cell: (u16, u16)) {
        if dims == self.dims || dims.cols == 0 || dims.rows == 0 {
            return;
        }
        self.dims = dims;
        self.term.lock().resize(dims);
        let _ = self.notifier.0.send(Msg::Resize(WindowSize { num_lines: dims.rows as u16, num_cols: dims.cols as u16, cell_width: cell.0, cell_height: cell.1 }));
    }

    pub fn frame(&self, fg0: Hsla, bg0: Hsla) -> Frame {
        let term = self.term.lock();
        let content = term.renderable_content();
        let (rows, cols) = (self.dims.rows, self.dims.cols);
        let mut cells: Vec<Vec<Option<Cell>>> = vec![vec![None; cols]; rows];
        let off = content.display_offset as i32;
        let palette = content.colors;
        let sel = content.selection;
        for ic in content.display_iter {
            let row = ic.point.line.0 + off;
            let col = ic.point.column.0;
            if row < 0 || row as usize >= rows || col >= cols {
                continue;
            }
            let cell = ic.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::HIDDEN) {
                continue;
            }
            let (mut fg, mut bg) = (super::colors::resolve(&cell.fg, palette, fg0, bg0), super::colors::resolve(&cell.bg, palette, fg0, bg0));
            let inverse = cell.flags.contains(Flags::INVERSE);
            if inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg.a *= 0.6;
            }
            let default_bg = matches!(cell.bg, Color::Named(NamedColor::Background)) && !inverse;
            cells[row as usize][col] = Some(Cell {
                c: cell.c,
                fg,
                bg: (!default_bg).then_some(bg),
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                selected: sel.is_some_and(|r| r.contains(ic.point)),
            });
        }
        let cur = content.cursor;
        let crow = cur.point.line.0 + off;
        let cursor = (crow >= 0 && (crow as usize) < rows && cur.point.column.0 < cols && cur.shape != CursorShape::Hidden).then_some((cur.point.column.0, crow as usize, cur.shape));
        Frame { cells, cursor }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let Some(tx) = self.teardown.take() else { return };
        #[cfg(unix)]
        {
            // the PTY gave the child its own process group: hang it all up, then make sure
            let group = -(self.child_pid as i32);
            // SAFETY: signalling a process group by id; a gone group yields ESRCH, ignored
            unsafe { libc::kill(group, libc::SIGHUP) };
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
                // SAFETY: signal 0 only probes for existence
                while std::time::Instant::now() < deadline && unsafe { libc::kill(group, 0) } == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                // SAFETY: as above
                unsafe { libc::kill(group, libc::SIGKILL) };
                let _ = tx.send(Msg::Shutdown);
            });
        }
        #[cfg(not(unix))]
        {
            let _ = tx.send(Msg::Shutdown);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Dims, Frame, Spawn, Terminal};
    use gpui::Hsla;
    use std::time::{Duration, Instant};

    fn text(f: &Frame) -> String {
        f.cells.iter().map(|r| r.iter().map(|c| c.as_ref().map(|c| c.c).unwrap_or(' ')).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    fn wait(t: &mut Terminal, needle: &str) -> Frame {
        let start = Instant::now();
        loop {
            t.take_dirty();
            t.pump();
            let f = t.frame(Hsla::white(), Hsla::black());
            if text(&f).contains(needle) {
                return f;
            }
            assert!(start.elapsed() < Duration::from_secs(8), "timed out waiting for {needle:?}:\n{}", text(&f));
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    #[test]
    fn runs_a_child_with_env_input_and_exit() {
        let spec = Spawn { program: "/bin/sh".into(), args: vec!["-c".into(), "echo \"v=$PANEL_TEST\"; cat".into()], cwd: std::env::temp_dir(), env: vec![("PANEL_TEST".into(), "it's here".into())] };
        let mut t = Terminal::spawn(&spec, Dims { cols: 40, rows: 6 }, (8, 16)).unwrap();
        wait(&mut t, "v=it's here");
        t.paste("'/tmp/a b.png' ");
        t.input(b"\r".to_vec());
        wait(&mut t, "'/tmp/a b.png'");
        t.input(b"\x04".to_vec());
        let start = Instant::now();
        while t.exited.is_none() {
            t.pump();
            assert!(start.elapsed() < Duration::from_secs(8));
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}
