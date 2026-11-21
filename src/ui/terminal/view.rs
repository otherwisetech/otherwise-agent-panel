//! The terminal surface: keys, paste (text, files, images), drops, selection, scrolling, and the
//! grid painted with GPUI text runs.
use super::model::{Dims, Frame, Spawn, Terminal};
use crate::attach;
use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::*;
use std::path::PathBuf;
use std::time::Duration;

pub const KEY_CONTEXT: &str = "AgentTerminal";

/// Font and colours the terminal paints with.
#[derive(Debug, Clone)]
pub struct TermStyle {
    pub font_family: SharedString,
    pub font_size: f32,
    pub fg: Hsla,
    pub bg: Hsla,
    pub selection: Hsla,
}

/// The title changed or the child exited.
pub enum TermEvent {
    Changed,
}

impl EventEmitter<TermEvent> for TerminalView {}

pub struct TerminalView {
    pub terminal: Option<Terminal>,
    pub error: Option<String>,
    /// Where dropped files and pasted images are copied before their path is typed in.
    pub attach_dir: Option<PathBuf>,
    pub style: TermStyle,
    focus: FocusHandle,
    cell: (Pixels, Pixels),
    origin: Point<Pixels>,
    scroll_rem: f32,
    dragging: bool,
    hitbox: Option<HitboxId>,
    focused: bool,
    seen: (String, Option<i32>),
}

impl TerminalView {
    pub fn new(style: TermStyle, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        TerminalView {
            terminal: None,
            error: None,
            attach_dir: None,
            style,
            focus: cx.focus_handle(),
            cell: (px(8.), px(16.)),
            origin: point(px(0.), px(0.)),
            scroll_rem: 0.,
            dragging: false,
            hitbox: None,
            focused: false,
            seen: (String::new(), None),
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let focused = self.focused;
        let Some(t) = self.terminal.as_mut() else { return };
        if t.take_dirty() {
            t.pump();
            cx.notify();
        }
        let now = (t.title.clone(), t.exited);
        if now != self.seen {
            self.seen = now;
            cx.emit(TermEvent::Changed);
        }
        if let Some(text) = t.take_clipboard()
            && focused
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn run(&mut self, spec: &Spawn, window: &mut Window, cx: &mut Context<Self>) {
        let dims = self.terminal.as_ref().map(|t| t.dims).unwrap_or(Dims { cols: 100, rows: 30 });
        self.terminal = None;
        let cell = (f32::from(self.cell.0) as u16, f32::from(self.cell.1) as u16);
        match Terminal::spawn(spec, dims, cell) {
            Ok(mut t) => {
                t.set_colors(self.style.fg, self.style.bg);
                self.terminal = Some(t);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.terminal = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn is_running(&self) -> bool {
        self.terminal.as_ref().is_some_and(|t| t.exited.is_none())
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.terminal.as_ref().and_then(|t| t.exited)
    }

    /// Type text as a paste (bracketed when the child asked for it).
    pub fn paste_text(&self, text: &str) {
        if let Some(t) = &self.terminal {
            t.paste(text);
        }
    }

    /// Copy files into the session and type their quoted paths.
    pub fn insert_files(&self, paths: &[PathBuf]) {
        let copied: Vec<PathBuf> = match &self.attach_dir {
            Some(dir) => paths.iter().filter_map(|p| attach::import(p, dir).ok().map(|a| a.path)).collect(),
            None => paths.to_vec(),
        };
        let text = attach::paths_to_text(&copied);
        if !text.is_empty() {
            self.paste_text(&text);
        }
    }

    fn paste_clipboard(&self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        for e in item.entries() {
            match e {
                ClipboardEntry::ExternalPaths(p) => return self.insert_files(p.paths()),
                ClipboardEntry::Image(img) => {
                    if let Some(dir) = &self.attach_dir
                        && let Ok(a) = attach::save_bytes(&img.bytes, image_ext(img.format), dir)
                    {
                        self.paste_text(&attach::paths_to_text(&[a.path]));
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Some(text) = item.text() {
            self.paste_text(&text);
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = &self.terminal else { return };
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let copy_paste = if cfg!(target_os = "macos") { m.platform && !m.control && !m.alt } else { m.control && m.shift && !m.alt };
        if copy_paste {
            match ks.key.to_lowercase().as_str() {
                "v" => {
                    self.paste_clipboard(cx);
                    cx.stop_propagation();
                    return;
                }
                "c" => {
                    if let Some(text) = t.selection_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        if m.platform {
            if let Some(b) = super::keys::platform_bytes(ks) {
                t.input(b);
                cx.stop_propagation();
            }
            return;
        }
        if m.shift && (ks.key == "pageup" || ks.key == "pagedown") {
            let page = t.dims.rows.max(2) as i32 - 1;
            t.scroll(if ks.key == "pageup" { page } else { -page });
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if let Some(bytes) = super::keys::to_bytes(ks, t.mode()) {
            t.input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = &self.terminal else { return };
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(self.cell.1).max(1.),
        };
        let n = super::keys::scroll_lines(&mut self.scroll_rem, lines);
        if n == 0 {
            return;
        }
        let (p, _) = self.grid_point(ev.position, t.dims, 0);
        match super::keys::wheel_bytes(t.mode(), n, p.column.0, p.line.0.max(0) as usize, ev.modifiers.shift) {
            Some(b) => t.write(b),
            None => t.scroll(n),
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn grid_point(&self, pos: Point<Pixels>, dims: Dims, offset: usize) -> (TermPoint, Side) {
        grid_point(pos, self.origin, self.cell, dims, offset)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(t) = &self.terminal else { return };
        let (p, side) = self.grid_point(ev.position, t.dims, t.display_offset());
        let ty = match ev.click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        if ev.click_count <= 1 {
            t.clear_selection();
        }
        t.start_selection(ty, p, side);
        self.dragging = true;
        if let Some(id) = self.hitbox {
            window.capture_pointer(id);
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(t) = &self.terminal {
            let (p, side) = self.grid_point(ev.position, t.dims, t.display_offset());
            t.update_selection(p, side);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            if let Some(t) = &self.terminal
                && t.selection_text().is_none()
            {
                t.clear_selection();
            }
            cx.notify();
        }
    }
}

pub fn image_ext(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

/// Pixel → grid point and side, clamped to the viewport; the display offset maps rows back into
/// the scrollback.
pub fn grid_point(pos: Point<Pixels>, origin: Point<Pixels>, cell: (Pixels, Pixels), dims: Dims, offset: usize) -> (TermPoint, Side) {
    let x = f32::from(pos.x - origin.x).max(0.);
    let y = f32::from(pos.y - origin.y).max(0.);
    let (cw, lh) = (f32::from(cell.0).max(1.), f32::from(cell.1).max(1.));
    let col = ((x / cw).floor() as usize).min(dims.cols.saturating_sub(1));
    let row = ((y / lh).floor() as usize).min(dims.rows.saturating_sub(1));
    let side = if (x / cw).fract() > 0.5 { Side::Right } else { Side::Left };
    (TermPoint::new(Line(row as i32 - offset as i32), Column(col)), side)
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("agent-terminal")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .size_full()
            .bg(self.style.bg)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.insert_files(paths.paths());
                cx.stop_propagation();
                cx.notify();
            }))
            .child(Grid { view: cx.entity() })
    }
}

struct Grid {
    view: Entity<TerminalView>,
}

struct Prepared {
    frame: Option<Frame>,
    message: Option<String>,
    origin: Point<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    font: Font,
    font_size: Pixels,
    style: TermStyle,
    focused: bool,
    hitbox: Hitbox,
}

impl IntoElement for Grid {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Grid {
    type RequestLayoutState = ();
    type PrepaintState = Prepared;

    fn id(&self) -> Option<ElementId> {
        Some("agent-terminal-grid".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepared {
        let st = self.view.read(cx).style.clone();
        let font = font(st.font_family.clone());
        let font_size = px(st.font_size);
        let font_id = cx.text_system().resolve_font(&font);
        let cell_w = cx.text_system().advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(px(8.));
        let line_h = (font_size * 1.3).round();
        let pad = px(6.);
        let cols = ((f32::from(bounds.size.width - pad * 2.) / f32::from(cell_w).max(1.)).floor() as usize).max(2);
        let rows = ((f32::from(bounds.size.height - pad * 2.) / f32::from(line_h).max(1.)).floor() as usize).max(1);
        let focused = self.view.read(cx).focus.is_focused(window);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        if self.view.read(cx).dragging {
            window.capture_pointer(hitbox.id);
        }
        let origin = bounds.origin + point(pad, pad);
        let id = hitbox.id;
        let (frame, message) = self.view.update(cx, |v, _| {
            v.cell = (cell_w, line_h);
            v.origin = origin;
            v.hitbox = Some(id);
            v.focused = focused;
            let cell = (f32::from(cell_w) as u16, f32::from(line_h) as u16);
            match v.terminal.as_mut() {
                Some(t) => {
                    t.resize(Dims { cols, rows }, cell);
                    t.set_colors(st.fg, st.bg);
                    (Some(t.frame(st.fg, st.bg)), None)
                }
                None => (None, v.error.clone()),
            }
        });
        Prepared { frame, message, origin, cell_w, line_h, font, font_size, style: st, focused, hitbox }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), p: &mut Prepared, window: &mut Window, cx: &mut App) {
        let (view, hb) = (self.view.clone(), p.hitbox.clone());
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                view.update(cx, |v, cx| v.on_mouse_move(ev, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |v, cx| v.on_mouse_up(ev, cx));
            }
        });
        let (fg0, bg0) = (p.style.fg, p.style.bg);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, bg0));
            let run = |text: &str, font: Font, color: Hsla, ul: bool| TextRun {
                len: text.len(),
                font,
                color,
                background_color: None,
                underline: ul.then(|| UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false }),
                strikethrough: None,
            };
            let Some(frame) = &p.frame else {
                if let Some(msg) = p.message.as_ref().filter(|m| !m.is_empty()) {
                    let line = window.text_system().shape_line(msg.clone().into(), p.font_size, &[run(msg, p.font.clone(), fg0, false)], None);
                    let _ = line.paint(p.origin, p.line_h, TextAlign::Left, None, window, cx);
                }
                return;
            };
            let at = |col: usize, row: usize| point(p.origin.x + p.cell_w * col as f32, p.origin.y + p.line_h * row as f32);
            for (r, row) in frame.cells.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    let Some(cell) = cell else { continue };
                    let b = Bounds::new(at(c, r), size(p.cell_w, p.line_h));
                    if let Some(bg) = cell.bg {
                        window.paint_quad(fill(b, bg));
                    }
                    if cell.selected {
                        window.paint_quad(fill(b, p.style.selection));
                    }
                }
            }
            for (r, row) in frame.cells.iter().enumerate() {
                let mut c = 0;
                while c < row.len() {
                    let Some(first) = &row[c] else {
                        c += 1;
                        continue;
                    };
                    if first.c == ' ' && !first.underline {
                        c += 1;
                        continue;
                    }
                    let start = c;
                    let (fg, bold, italic, ul) = (first.fg, first.bold, first.italic, first.underline);
                    let mut text = String::new();
                    while c < row.len() {
                        match &row[c] {
                            Some(n) if n.fg == fg && n.bold == bold && n.italic == italic && n.underline == ul => text.push(if n.c == '\0' { ' ' } else { n.c }),
                            None => text.push(' '),
                            _ => break,
                        }
                        c += 1;
                    }
                    if !ul {
                        text.truncate(text.trim_end().len());
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let f = Font { weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL }, style: if italic { FontStyle::Italic } else { FontStyle::Normal }, ..p.font.clone() };
                    let line = window.text_system().shape_line(text.clone().into(), p.font_size, &[run(&text, f, fg, ul)], Some(p.cell_w));
                    let _ = line.paint(at(start, r), p.line_h, TextAlign::Left, None, window, cx);
                }
            }
            if let Some((col, row, shape)) = frame.cursor {
                let o = at(col, row);
                let color = Hsla { a: if p.focused { 0.85 } else { 0.4 }, ..fg0 };
                let b = match shape {
                    CursorShape::Beam => Bounds::new(o, size(px(2.), p.line_h)),
                    CursorShape::Underline => Bounds::new(o + point(px(0.), p.line_h - px(2.)), size(p.cell_w, px(2.))),
                    _ => Bounds::new(o, size(p.cell_w, p.line_h)),
                };
                if p.focused && matches!(shape, CursorShape::Block) {
                    window.paint_quad(fill(b, color));
                    if let Some(Some(cell)) = frame.cells.get(row).and_then(|r| r.get(col))
                        && cell.c != ' '
                    {
                        let s = cell.c.to_string();
                        let line = window.text_system().shape_line(s.clone().into(), p.font_size, &[run(&s, p.font.clone(), bg0, false)], Some(p.cell_w));
                        let _ = line.paint(o, p.line_h, TextAlign::Left, None, window, cx);
                    }
                } else if matches!(shape, CursorShape::Block | CursorShape::HollowBlock) {
                    let mut q = fill(b, Hsla::transparent_black());
                    q.border_widths = Edges::all(px(1.));
                    q.border_color = color;
                    window.paint_quad(q);
                } else {
                    window.paint_quad(fill(b, color));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::grid_point;
    use crate::ui::terminal::model::Dims;
    use alacritty_terminal::index::Side;
    use gpui::{Pixels, point, px};

    #[test]
    fn pixels_to_cells() {
        let cell: (Pixels, Pixels) = (px(8.), px(16.));
        let dims = Dims { cols: 80, rows: 24 };
        let (p, side) = grid_point(point(px(35.), px(52.)), point(px(10.), px(20.)), cell, dims, 0);
        assert_eq!((p.line.0, p.column.0, side), (2, 3, Side::Left));
        let (p, _) = grid_point(point(px(35.), px(52.)), point(px(10.), px(20.)), cell, dims, 10);
        assert_eq!(p.line.0, -8);
        let (p, _) = grid_point(point(px(9999.), px(9999.)), point(px(0.), px(0.)), cell, dims, 0);
        assert_eq!((p.line.0, p.column.0), (23, 79));
    }
}
