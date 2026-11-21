//! Rendering: header, conversation, composer with the pickers, menus, the bar.
use super::panel::{AgentPanel, KEY_CONTEXT, Menu, ProviderEntry};
use super::style::PanelStyle;
use super::widgets::*;
use super::{ClosePanelMenu, NewConversation};
use crate::attach::{Attachment, Kind};
use crate::history::{ChangeState, DOCK_MODES, DockMode, Message, PendingChange, ToolUse, ViewMode};
use crate::permission::MODES;
use crate::t;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::IconName;
use gpui_component::input::Textarea;
use gpui_component::text::TextView;

fn dock_icon(m: DockMode) -> IconName {
    match m {
        DockMode::Bar => IconName::Minimize,
        DockMode::Bottom => IconName::PanelBottom,
        DockMode::Right => IconName::PanelRight,
        DockMode::Focus | DockMode::Hidden => IconName::Maximize,
    }
}

fn date_of(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

impl AgentPanel {
    fn render_header(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let view = self.prefs.view;
        let seg = |id: &'static str, label: String, on: bool| {
            let (accent, soft, muted) = (s.accent, s.accent_soft, s.muted);
            div()
                .id(id)
                .px(px(8.))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(on, move |d| d.bg(soft).text_color(accent))
                .when(!on, move |d| d.text_color(muted))
                .child(label)
        };
        let switch = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .border_1()
            .border_color(s.border)
            .child(seg("view-chat", t!("panel.view.chat"), view == ViewMode::Chat).on_click(cx.listener(|this, _, w, cx| this.set_view(ViewMode::Chat, w, cx))))
            .child(seg("view-terminal", t!("panel.view.terminal"), view == ViewMode::Terminal).on_click(cx.listener(|this, _, w, cx| this.set_view(ViewMode::Terminal, w, cx))));
        let mut modes = div().flex().items_center().gap(px(1.));
        for m in DOCK_MODES {
            let label = t!(&format!("panel.dock.{}", m.key()));
            modes = modes.child(
                icon_button(s, SharedString::from(format!("dock-{}", m.key())), dock_icon(m), self.prefs.dock == m)
                    .tooltip(move |w, cx| gpui_component::tooltip::Tooltip::new(label.clone()).build(w, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_dock(m, cx))),
            );
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(40.))
            .px(px(10.))
            .border_b_1()
            .border_color(s.border)
            .child(icon(IconName::Bot, 16., s.accent))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(t!("panel.title", app = self.host.app_name())))
            .child(switch)
            .child(icon_button(s, "history", IconName::GalleryVerticalEnd, self.show_history).tooltip(|w, cx| gpui_component::tooltip::Tooltip::new(t!("panel.history")).build(w, cx)).on_click(
                cx.listener(|this, _, _, cx| {
                    this.show_history = !this.show_history;
                    cx.notify();
                }),
            ))
            .child(
                icon_button(s, "new", IconName::Plus, false)
                    .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new(t!("panel.new")).build(w, cx))
                    .on_click(cx.listener(|this, _, w, cx| this.new_conversation(w, cx))),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(s.border))
            .child(modes)
            .child(
                icon_button(s, "close", IconName::Close, false)
                    .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new(t!("panel.hide")).build(w, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.set_dock(DockMode::Hidden, cx))),
            )
    }

    fn render_history(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().gap(px(2.));
        if self.past.is_empty() {
            list = list.child(div().p(px(8.)).text_size(px(12.)).text_color(s.muted).child(t!("panel.history.empty")));
        }
        let hover = s.raised;
        for (i, c) in self.past.iter().enumerate() {
            let conv = c.clone();
            let id = c.id.clone();
            let current = c.id == self.conv.id;
            list = list.child(
                div()
                    .id(("past", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(s.accent_soft))
                    .hover(move |st| st.bg(hover))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).child(if c.title.is_empty() { t!("panel.history.untitled") } else { c.title.clone() }))
                    .child(div().flex_none().text_size(px(11.)).text_color(s.muted).child(date_of(c.updated)))
                    .child(icon_button(s, ("past-del", i), IconName::Delete, false).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_conversation(id.clone(), cx);
                    })))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_conversation(conv.clone(), cx))),
            );
        }
        div().flex().flex_col().gap(px(6.)).p(px(8.)).rounded(px(8.)).border_1().border_color(s.border).bg(s.surface).child(menu_title(s, t!("panel.history"))).child(list)
    }

    fn render_attachment(&self, s: &PanelStyle, id: (&'static str, usize), a: &Attachment, removable: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = a.path.clone();
        let remove = removable.then(|| {
            icon_button(s, (id.0, id.1 + 10_000), IconName::Close, false).on_click(cx.listener(move |this, _, _, cx| {
                this.draft.retain(|d| d.path != path);
                cx.notify();
            }))
        });
        if a.kind == Kind::Image && a.path.is_file() {
            return div()
                .id(id)
                .relative()
                .flex_none()
                .size(px(56.))
                .rounded(px(6.))
                .overflow_hidden()
                .border_1()
                .border_color(s.border)
                .child(img(a.path.clone()).size_full().object_fit(ObjectFit::Cover))
                .when_some(remove, |d, r| d.child(div().absolute().top(px(-2.)).right(px(-2.)).child(r)))
                .into_any_element();
        }
        let ic = if a.kind == Kind::Pdf { IconName::BookOpen } else { IconName::File };
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(28.))
            .pl(px(7.))
            .pr(px(if removable { 0. } else { 8. }))
            .rounded(px(6.))
            .border_1()
            .border_color(s.border)
            .bg(s.raised)
            .text_size(px(11.5))
            .child(icon(ic, 13., s.muted))
            .child(div().max_w(px(160.)).truncate().child(a.name.clone()))
            .child(div().text_color(s.muted).child(crate::attach::human_size(a.bytes)))
            .when_some(remove, |d, r| d.child(r))
            .into_any_element()
    }

    fn tool_chips(s: &PanelStyle, tools: &[ToolUse]) -> Div {
        div().flex().flex_wrap().gap(px(4.)).children(tools.iter().map(|u| if u.failed { chip(u.tool.label(), s.danger, s.danger_soft) } else { chip(u.tool.label(), s.accent, s.accent_soft) }))
    }

    fn render_change(&self, s: &PanelStyle, key: usize, c: &PendingChange, cx: &mut Context<Self>) -> AnyElement {
        let (id_ok, id_no) = (c.id.clone(), c.id.clone());
        let status = match c.state {
            ChangeState::Pending => None,
            ChangeState::Applied => Some((t!("panel.change.applied"), s.ok)),
            ChangeState::Rejected => Some((t!("panel.change.rejected"), s.muted)),
            ChangeState::Failed => Some((t!("panel.change.failed", detail = c.error.clone().unwrap_or_default()), s.danger)),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if c.state == ChangeState::Pending { s.note } else { s.border })
            .bg(if c.state == ChangeState::Pending { s.note_soft } else { s.surface })
            .child(div().flex().items_center().gap(px(6.)).child(icon(IconName::Info, 13., s.note)).child(div().text_size(px(11.)).text_color(s.muted).child(t!("panel.change.pending"))))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(c.title.clone()))
            .when(!c.detail.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(s.muted).child(c.detail.clone())))
            .map(|d| match status {
                Some((text, color)) => d.child(div().text_size(px(12.)).text_color(color).child(text)),
                None => d.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(button(s, ("apply", key), t!("panel.change.apply"), true).on_click(cx.listener(move |this, _, w, cx| this.resolve(id_ok.clone(), true, w, cx))))
                        .child(button(s, ("reject", key), t!("panel.change.reject"), false).on_click(cx.listener(move |this, _, w, cx| this.resolve(id_no.clone(), false, w, cx)))),
                ),
            })
            .into_any_element()
    }

    fn render_message(&self, s: &PanelStyle, i: usize, m: &Message, cx: &mut Context<Self>) -> AnyElement {
        if m.user {
            let atts = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(4.))
                .children(m.attachments.iter().enumerate().map(|(j, a)| self.render_attachment(s, ("msg-att", i * 100 + j), a, false, cx)).collect::<Vec<_>>());
            return div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .when(!m.attachments.is_empty(), |d| d.child(atts))
                .when(!m.text.trim().is_empty(), |d| {
                    d.child(div().max_w(relative(0.88)).px(px(10.)).py(px(7.)).rounded(px(10.)).bg(s.accent_soft).text_size(px(13.)).line_height(px(19.)).child(m.text.clone()))
                })
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(!m.tools.is_empty(), |d| d.child(Self::tool_chips(s, &m.tools)))
            .when(!m.text.trim().is_empty(), |d| d.child(div().text_size(px(13.)).child(TextView::markdown(SharedString::from(format!("{}-{i}", self.conv.id)), m.text.clone()).selectable(true))))
            .children(m.changes.iter().enumerate().map(|(j, c)| self.render_change(s, i * 100 + j, c, cx)).collect::<Vec<_>>())
            .children(
                m.error
                    .clone()
                    .map(|e| div().flex().items_start().gap(px(6.)).text_size(px(12.)).text_color(s.danger).child(icon(IconName::TriangleAlert, 13., s.danger)).child(div().flex_1().child(e))),
            )
            .into_any_element()
    }

    fn render_empty(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let mut d = div().flex().flex_col().items_center().gap(px(10.)).py(px(28.)).px(px(12.));
        if self.clis.is_none() {
            return d.child(div().text_size(px(12.5)).text_color(s.muted).child(t!("panel.looking")));
        }
        if self.providers().is_empty() {
            return d
                .child(icon(IconName::Bot, 28., s.muted))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t!("panel.none.title")))
                .child(div().text_size(px(12.5)).text_color(s.muted).text_center().child(t!("panel.none.body")))
                .child(button(s, "detect", t!("panel.none.detect"), false).on_click(cx.listener(|this, _, _, cx| this.detect(cx))));
        }
        d = d
            .child(icon(IconName::Bot, 28., s.accent))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t!("panel.intro.title")))
            .child(div().text_size(px(12.5)).text_color(s.muted).text_center().child(t!("panel.intro.body", app = self.host.app_name())));
        let mut chips = div().flex().flex_wrap().justify_center().gap(px(6.));
        for (i, q) in self.host.suggestions().into_iter().take(6).enumerate() {
            let q2 = q.clone();
            chips = chips.child(button(s, ("suggest", i), q, false).on_click(cx.listener(move |this, _, w, cx| this.use_suggestion(q2.clone(), w, cx))));
        }
        d.child(chips)
    }

    fn render_chat_body(&self, s: &PanelStyle, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(14.)).p(px(12.));
        if self.show_history {
            body = body.child(self.render_history(s, cx));
        }
        if self.conv.is_empty() && self.live.is_none() {
            body = body.child(self.render_empty(s, cx));
        }
        for (i, m) in self.conv.messages.iter().enumerate() {
            body = body.child(self.render_message(s, i, m, cx));
        }
        if let Some(live) = &self.live {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(!live.tools.is_empty(), |d| d.child(Self::tool_chips(s, &live.tools)))
                    .child(if live.text.is_empty() {
                        div().text_size(px(13.)).text_color(s.muted).child(t!("panel.thinking")).into_any_element()
                    } else {
                        div().text_size(px(13.)).child(TextView::markdown(SharedString::from(format!("{}-live", self.conv.id)), live.text.clone())).into_any_element()
                    })
                    .children(live.changes.iter().enumerate().map(|(j, c)| self.render_change(s, 90_000 + j, c, cx)).collect::<Vec<_>>()),
            );
        }
        div().id("agent-chat-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).child(body).into_any_element()
    }

    fn render_terminal_body(&self, s: &PanelStyle, cx: &mut Context<Self>) -> AnyElement {
        if self.show_history {
            return div().id("agent-history-scroll").flex_1().min_h_0().overflow_y_scroll().p(px(12.)).child(self.render_history(s, cx)).into_any_element();
        }
        let running = self.terminal_running(cx);
        let stale = running && self.terminal_key != self.terminal_settings_key();
        let mut col = div().flex().flex_col().flex_1().min_h_0();
        if let Some(t) = &self.terminal {
            if let Some(code) = t.read(cx).exit_code() {
                col = col.child(self.terminal_banner(s, t!("panel.terminal.exited", code = code), cx));
            } else if stale {
                col = col.child(self.terminal_banner(s, t!("panel.terminal.stale"), cx));
            }
            col = col.child(div().flex_1().min_h_0().child(t.clone()));
        } else {
            let label = self.current().map(|p| t!("panel.terminal.start", agent = p.label)).unwrap_or_default();
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(icon(IconName::SquareTerminal, 28., s.muted))
                    .child(div().text_size(px(12.5)).text_color(s.muted).text_center().px(px(16.)).child(t!("panel.terminal.intro")))
                    .child(button(s, "term-start", label, true).on_click(cx.listener(|this, _, w, cx| this.launch_terminal(w, cx)))),
            );
        }
        col.into_any_element()
    }

    fn terminal_banner(&self, s: &PanelStyle, text: String, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .bg(s.note_soft)
            .text_size(px(12.))
            .child(div().flex_1().child(text))
            .child(button(s, "term-restart", t!("panel.terminal.restart"), false).on_click(cx.listener(|this, _, w, cx| this.launch_terminal(w, cx))))
    }

    fn render_menu(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        let mut frame = menu_frame(s);
        match menu {
            Menu::Provider => {
                frame = frame.child(menu_title(s, t!("panel.menu.agent")));
                let cur = self.current().map(|p| p.id);
                for (i, p) in self.providers().into_iter().enumerate() {
                    let id = p.id.clone();
                    let hint = if p.remote.is_some() { Some(t!("panel.menu.remote_hint", vendor = p.vendor)) } else { p.program.as_ref().map(|x| x.display().to_string()) };
                    frame = frame.child(menu_row(s, ("prov", i), p.label.clone(), hint, cur.as_ref() == Some(&p.id)).on_click(cx.listener(move |this, _, _, cx| this.set_provider(id.clone(), cx))));
                }
                frame = frame.child(menu_row(s, "prov-detect", t!("panel.none.detect"), None, false).on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    this.detect(cx);
                })));
            }
            Menu::Model => {
                frame = frame.child(menu_title(s, t!("panel.menu.model")));
                if let Some(p) = self.current() {
                    let cur = self.current_model(&p.id);
                    let list = self.models_of(&p.id);
                    let mut rows = div().id("model-list").flex().flex_col().max_h(px(300.)).overflow_y_scroll();
                    for (i, m) in list.into_iter().enumerate() {
                        let on = m.id == cur.id;
                        let hint = m.id.clone().filter(|id| *id != m.label);
                        let m2 = m.clone();
                        rows = rows.child(menu_row(s, ("model", i), m.label.clone(), hint, on).on_click(cx.listener(move |this, _, _, cx| this.set_model(m2.clone(), cx))));
                    }
                    frame = frame.child(rows);
                }
            }
            Menu::Permission => {
                frame = frame.child(menu_title(s, t!("panel.menu.permission")));
                for (i, m) in MODES.into_iter().enumerate() {
                    frame = frame.child(menu_row(s, ("perm", i), m.label(), Some(m.hint()), self.prefs.permission == m).on_click(cx.listener(move |this, _, _, cx| this.set_permission(m, cx))));
                }
            }
            Menu::Commands => {
                frame = frame.child(menu_title(s, t!("panel.menu.commands")));
                frame = frame
                    .child(menu_row(s, "cmd-new", t!("panel.new"), None, false).on_click(cx.listener(|this, _, w, cx| {
                        this.menu = None;
                        this.new_conversation(w, cx);
                    })))
                    .child(menu_row(s, "cmd-history", t!("panel.history"), None, false).on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        this.show_history = true;
                        cx.notify();
                    })))
                    .child(menu_row(s, "cmd-attach", t!("panel.attach"), None, false).on_click(cx.listener(|this, _, w, cx| this.pick_files(w, cx))));
                let other = if self.prefs.view == ViewMode::Chat { ViewMode::Terminal } else { ViewMode::Chat };
                let label = if other == ViewMode::Chat { t!("panel.view.chat") } else { t!("panel.view.terminal") };
                frame = frame.child(menu_row(s, "cmd-view", label, None, false).on_click(cx.listener(move |this, _, w, cx| this.set_view(other, w, cx))));
                let sugg = self.host.suggestions();
                if !sugg.is_empty() {
                    frame = frame.child(menu_title(s, t!("panel.menu.suggestions")));
                    for (i, q) in sugg.into_iter().enumerate() {
                        let q2 = q.clone();
                        frame = frame.child(menu_row(s, ("cmd-sugg", i), q, None, false).on_click(cx.listener(move |this, _, w, cx| this.use_suggestion(q2.clone(), w, cx))));
                    }
                }
            }
        }
        let frame = frame.id("agent-menu").occlude().on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        }));
        Some(deferred(div().absolute().bottom(px(40.)).left(px(8.)).child(frame)).with_priority(2).into_any_element())
    }

    /// The pickers and buttons under the input (and above the terminal in terminal mode).
    fn render_toolbar(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let cur: Option<ProviderEntry> = self.current();
        let mut row = div().flex().items_center().gap(px(2.));
        row = row
            .child(
                icon_button(s, "attach", IconName::Plus, false)
                    .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new(t!("panel.attach")).build(w, cx))
                    .on_click(cx.listener(|this, _, w, cx| this.pick_files(w, cx))),
            )
            .child(
                div()
                    .id("commands")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .rounded(px(6.))
                    .text_size(px(14.))
                    .text_color(s.muted)
                    .cursor_pointer()
                    .hover({
                        let h = s.raised;
                        move |st| st.bg(h)
                    })
                    .child("/")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Commands, cx))),
            );
        if let Some(p) = &cur {
            let model = self.current_model(&p.id);
            row = row
                .child(pill(s, "pick-agent", p.label.clone()).on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Provider, cx))))
                .child(pill(s, "pick-model", model.label).on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Model, cx))))
                .child(pill(s, "pick-perm", self.prefs.permission.label()).on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Permission, cx))));
        }
        row = row.child(div().flex_1());
        if self.prefs.view == ViewMode::Chat {
            if self.live.is_some() {
                row = row.child(button(s, "stop", t!("panel.stop"), false).on_click(cx.listener(|this, _, _, cx| this.stop(cx))));
            } else {
                let can = cur.is_some();
                row = row.child(button(s, "send", t!("panel.send"), can).when(can, |d| d.on_click(cx.listener(|this, _, w, cx| this.send_input(w, cx)))));
            }
        }
        row
    }

    fn render_offer(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Option<Div> {
        let offer = self.offer.as_ref()?;
        let names: Vec<String> = offer.files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        let mut buttons = div().flex().flex_wrap().gap(px(6.));
        for (i, a) in offer.actions.iter().enumerate() {
            let id = a.id.clone();
            buttons = buttons.child(button(s, ("drop-act", i), a.label.clone(), i == 0).on_click(cx.listener(move |this, _, w, cx| this.run_drop_action(Some(id.clone()), w, cx))));
        }
        buttons = buttons.child(button(s, "drop-attach", t!("panel.drop.attach"), false).on_click(cx.listener(|this, _, w, cx| this.run_drop_action(None, w, cx)))).child(
            button(s, "drop-cancel", t!("panel.drop.cancel"), false).on_click(cx.listener(|this, _, _, cx| {
                this.offer = None;
                cx.notify();
            })),
        );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(10.))
                .mx(px(10.))
                .mb(px(6.))
                .rounded(px(8.))
                .border_1()
                .border_color(s.accent)
                .bg(s.accent_soft)
                .child(div().text_size(px(12.5)).child(t!("panel.drop.title", files = names.join(", "))))
                .child(buttons),
        )
    }

    fn render_composer(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let mut c = div().relative().flex().flex_col().flex_none().gap(px(6.)).p(px(8.)).border_t_1().border_color(s.border);
        if let Some(o) = self.render_offer(s, cx) {
            c = c.child(o);
        }
        if let Some((text, err)) = &self.notice {
            c = c.child(div().text_size(px(11.5)).text_color(if *err { s.danger } else { s.muted }).child(text.clone()));
        }
        if self.prefs.view == ViewMode::Chat {
            if !self.draft.is_empty() {
                c = c.child(div().flex().flex_wrap().gap(px(6.)).children(self.draft.iter().enumerate().map(|(i, a)| self.render_attachment(s, ("draft", i), a, true, cx)).collect::<Vec<_>>()));
            }
            c = c.child(
                div()
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                        let ks = &ev.keystroke;
                        let paste = if cfg!(target_os = "macos") { ks.modifiers.platform } else { ks.modifiers.control };
                        if paste && ks.key == "v" && this.paste_into_chat(cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(Textarea::new(&self.input)),
            );
        }
        c = c.child(self.render_toolbar(s, cx));
        let privacy = match self.current() {
            Some(p) if p.remote.is_some() => t!("panel.privacy.remote", agent = p.label, vendor = p.vendor),
            Some(p) => t!("panel.privacy.cli", agent = p.label, vendor = p.vendor),
            None => t!("panel.privacy.none"),
        };
        c = c.child(div().text_size(px(10.5)).line_height(px(14.)).text_color(s.muted).child(privacy));
        if let Some(m) = self.render_menu(s, cx) {
            c = c.child(m);
        }
        c
    }

    fn render_bar(&self, s: &PanelStyle, cx: &mut Context<Self>) -> Div {
        let status = match &self.live {
            Some(l) if l.text.is_empty() => t!("panel.thinking"),
            Some(l) => l.text.lines().rev().find(|x| !x.trim().is_empty()).unwrap_or("").to_string(),
            None => self
                .conv
                .messages
                .iter()
                .rev()
                .find(|m| !m.user && !m.text.trim().is_empty())
                .map(|m| m.text.lines().next().unwrap_or("").to_string())
                .unwrap_or_else(|| t!("panel.bar.prompt", app = self.host.app_name())),
        };
        let to = self.expand_to;
        let hover = s.raised;
        let mut modes = div().flex().items_center().gap(px(1.));
        for m in DOCK_MODES {
            modes =
                modes.child(icon_button(s, SharedString::from(format!("bar-dock-{}", m.key())), dock_icon(m), self.prefs.dock == m).on_click(cx.listener(move |this, _, _, cx| this.set_dock(m, cx))));
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .size_full()
            .px(px(10.))
            .border_t_1()
            .border_color(s.border)
            .bg(s.surface)
            .child(icon(IconName::Bot, 15., s.accent))
            .child(
                div()
                    .id("bar-open")
                    .flex_1()
                    .min_w_0()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(s.border)
                    .cursor_pointer()
                    .hover(move |st| st.bg(hover))
                    .text_size(px(12.5))
                    .text_color(s.muted)
                    .child(div().truncate().child(status))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.set_dock(to, cx);
                        this.input.update(cx, |st, cx| st.focus(w, cx));
                    })),
            )
            .child(modes)
            .child(icon_button(s, "bar-close", IconName::Close, false).on_click(cx.listener(|this, _, _, cx| this.set_dock(DockMode::Hidden, cx))))
    }
}

impl Render for AgentPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = PanelStyle::get(cx);
        if self.prefs.dock == DockMode::Bar {
            return div().track_focus(&self.focus).size_full().child(self.render_bar(&s, cx)).into_any_element();
        }
        let drop_bg = s.accent_soft;
        let body = if self.prefs.view == ViewMode::Terminal { self.render_terminal_body(&s, cx) } else { self.render_chat_body(&s, cx) };
        div()
            .id("agent-panel")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(s.background)
            .text_color(s.ink)
            .on_action(cx.listener(|this, _: &ClosePanelMenu, _, cx| {
                if this.menu.take().is_some() || this.offer.take().is_some() {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &NewConversation, w, cx| this.new_conversation(w, cx)))
            .drag_over::<ExternalPaths>(move |st, _, _, _| st.bg(drop_bg))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if this.prefs.view == ViewMode::Terminal
                    && let Some(t) = &this.terminal
                {
                    t.read(cx).insert_files(paths.paths());
                } else {
                    this.on_files(paths.paths().to_vec(), cx);
                }
            }))
            .child(self.render_header(&s, cx))
            .child(body)
            .child(self.render_composer(&s, cx))
            .into_any_element()
    }
}
