//! The host's main view with the panel docked: Barra (a one-line bar along the bottom), Abajo
//! (below, resizable), Derecha (right, resizable), Foco (the panel fills the window) or hidden.
//! Sizes and the mode are remembered (`.panel.json`). ⌘J toggles.
use super::TogglePanel;
use super::host::AgentHost;
use super::panel::{AgentPanel, PanelEvent};
use super::style::PanelStyle;
use crate::history::{DockMode, MIN_HEIGHT, MIN_WIDTH};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;

const DIVIDER: f32 = 5.;
const BAR: f32 = 44.;
/// The main view keeps at least this much.
const MAIN_MIN: f32 = 260.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Width,
    Height,
}

pub struct AgentDock {
    main: AnyView,
    main_focus: Option<FocusHandle>,
    panel: Entity<AgentPanel>,
    focus: FocusHandle,
    drag: Option<Drag>,
    _subs: Vec<Subscription>,
}

impl AgentDock {
    /// `main` is the app's own view; `main_focus` gets the focus back when the panel hides.
    pub fn new(main: impl Into<AnyView>, main_focus: Option<FocusHandle>, host: Rc<dyn AgentHost>, window: &mut Window, cx: &mut Context<Self>) -> AgentDock {
        let panel = cx.new(|cx| AgentPanel::new(host, window, cx));
        let sub = cx.subscribe_in(&panel, window, |this, _, ev: &PanelEvent, window, cx| {
            let PanelEvent::DockChanged = ev;
            if this.panel.read(cx).dock_mode() == DockMode::Hidden
                && let Some(f) = &this.main_focus
            {
                window.focus(f, cx);
            }
            cx.notify();
        });
        AgentDock { main: main.into(), main_focus, panel, focus: cx.focus_handle(), drag: None, _subs: vec![sub] }
    }

    pub fn panel(&self) -> &Entity<AgentPanel> {
        &self.panel
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height));
        match drag {
            Drag::Width => {
                let width = (w - f32::from(ev.position.x)).clamp(MIN_WIDTH, (w - MAIN_MIN).max(MIN_WIDTH));
                self.panel.update(cx, |p, cx| p.set_size(Some(width), None, cx));
            }
            Drag::Height => {
                let height = (h - f32::from(ev.position.y)).clamp(MIN_HEIGHT, (h - MAIN_MIN * 0.6).max(MIN_HEIGHT));
                self.panel.update(cx, |p, cx| p.set_size(None, Some(height), cx));
            }
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.panel.read(cx).save_prefs();
            cx.notify();
        }
    }

    fn divider(&self, drag: Drag, cx: &mut Context<Self>) -> Stateful<Div> {
        let s = PanelStyle::get(cx);
        let (line, hot) = (s.border, s.accent);
        let active = self.drag == Some(drag);
        let d = div().id(if drag == Drag::Width { "dock-divider-v" } else { "dock-divider-h" }).flex_none().bg(if active { hot } else { line }).hover(move |st| st.bg(hot));
        let d = match drag {
            Drag::Width => d.w(px(DIVIDER)).h_full().cursor(CursorStyle::ResizeLeftRight),
            Drag::Height => d.h(px(DIVIDER)).w_full().cursor(CursorStyle::ResizeUpDown),
        };
        d.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.drag = Some(drag);
                cx.stop_propagation();
                cx.notify();
            }),
        )
    }
}

impl Render for AgentDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            match &self.main_focus {
                Some(f) => window.focus(f, cx),
                None => window.focus(&self.focus, cx),
            }
        }
        let prefs = self.panel.read(cx).prefs.clone();
        let mode = prefs.dock;
        let vp = window.viewport_size();
        let width = prefs.width.min((f32::from(vp.width) - MAIN_MIN).max(MIN_WIDTH));
        let height = prefs.height.min((f32::from(vp.height) - MAIN_MIN * 0.6).max(MIN_HEIGHT));
        let main = div().flex_1().min_w_0().min_h_0().overflow_hidden().child(self.main.clone());
        let panel = self.panel.clone();
        let root = div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .on_action(cx.listener(|this, _: &TogglePanel, window, cx| this.panel.update(cx, |p, cx| p.toggle(window, cx))))
            .when(self.drag.is_some(), |d| {
                d.cursor(if self.drag == Some(Drag::Width) { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown })
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            })
            .on_mouse_move(cx.listener(Self::on_move));
        match mode {
            DockMode::Hidden => root.child(main),
            DockMode::Focus => root.child(div().flex_1().min_w_0().child(panel)),
            DockMode::Right => root.flex_row().child(main).child(self.divider(Drag::Width, cx)).child(div().flex_none().w(px(width)).h_full().child(panel)),
            DockMode::Bottom => root.flex_col().child(main).child(self.divider(Drag::Height, cx)).child(div().flex_none().h(px(height)).w_full().child(panel)),
            DockMode::Bar => root.flex_col().child(main).child(div().flex_none().h(px(BAR)).w_full().child(panel)),
        }
    }
}
