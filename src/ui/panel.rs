//! The panel itself: provider / model / permission pickers, Chat and Terminal modes, history,
//! attachments, drop actions and pending-change cards. State lives here; `chat.rs` renders the
//! conversation and the composer.
use super::host::{AgentHost, DropAction, DropOutcome};
use super::style::PanelStyle;
use super::terminal::{Spawn, TermStyle, TerminalView};
use crate::args::{self, Session, Turn};
use crate::attach::{self, Attachment};
use crate::history::{self, ChangeState, Conversation, DockMode, Message, PendingChange, Prefs, ToolUse, ViewMode};
use crate::mcp::McpServer;
use crate::permission::PermissionMode;
use crate::provider::{self, Cli, Model, ProviderId};
use crate::remote::{Cancel, RemoteProvider, RemoteTurn};
use crate::stream::{AgentError, StreamEvent};
use crate::t;
use gpui::*;
use gpui_component::input::{InputEvent, TextareaState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const KEY_CONTEXT: &str = "AgentPanel";
const POLL: Duration = Duration::from_millis(90);

pub enum PanelEvent {
    /// Dock mode or size changed (the dock re-lays out).
    DockChanged,
}

impl EventEmitter<PanelEvent> for AgentPanel {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Provider,
    Model,
    Permission,
    Commands,
}

#[derive(Default)]
struct Shared {
    events: Vec<StreamEvent>,
    done: Option<Result<String, AgentError>>,
}

/// The answer being streamed.
pub(super) struct Live {
    shared: Arc<Mutex<Shared>>,
    cancel: Cancel,
    pub(super) text: String,
    pub(super) tools: Vec<ToolUse>,
    pub(super) changes: Vec<PendingChange>,
}

pub(super) struct DropOffer {
    pub(super) files: Vec<PathBuf>,
    pub(super) actions: Vec<DropAction>,
}

/// A picker entry.
#[derive(Clone)]
pub(super) struct ProviderEntry {
    pub(super) id: ProviderId,
    pub(super) label: String,
    pub(super) vendor: String,
    pub(super) program: Option<PathBuf>,
    pub(super) remote: Option<Arc<dyn RemoteProvider>>,
}

pub struct AgentPanel {
    pub(super) host: Rc<dyn AgentHost>,
    pub(super) dir: PathBuf,
    pub prefs: Prefs,
    pub(super) clis: Option<Vec<(Cli, PathBuf)>>,
    detecting: bool,
    pub(super) remotes: Vec<Arc<dyn RemoteProvider>>,
    pub(super) models: HashMap<String, Vec<Model>>,
    pub(super) conv: Conversation,
    pub(super) past: Vec<Conversation>,
    pub(super) show_history: bool,
    pub(super) menu: Option<Menu>,
    pub(super) input: Entity<TextareaState>,
    pub(super) draft: Vec<Attachment>,
    pub(super) offer: Option<DropOffer>,
    pub(super) live: Option<Live>,
    pub(super) notice: Option<(String, bool)>,
    pub(super) terminal: Option<Entity<TerminalView>>,
    /// The settings the running terminal was launched with.
    pub(super) terminal_key: Option<String>,
    pub(super) scroll: ScrollHandle,
    pending_ask: Option<String>,
    pub(super) focus: FocusHandle,
    /// Where the bar expands to.
    pub(super) expand_to: DockMode,
    _subs: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(host: Rc<dyn AgentHost>, window: &mut Window, cx: &mut Context<Self>) -> AgentPanel {
        let dir = history::dir(&host.app_id());
        let prefs = Prefs::load(&dir);
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder(t!("panel.input.placeholder")));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { secondary: _, shift: false } => this.send_input(window, cx),
            InputEvent::Change if input.read(cx).value().as_str() == "/" => {
                input.update(cx, |s, cx| s.set_value("", window, cx));
                this.menu = Some(Menu::Commands);
                cx.notify();
            }
            _ => {}
        });
        let expand_to = if matches!(prefs.last_visible, DockMode::Bar | DockMode::Hidden) { DockMode::Right } else { prefs.last_visible };
        let mut p = AgentPanel {
            remotes: host.remote_providers(),
            host,
            dir,
            prefs,
            clis: None,
            detecting: false,
            models: HashMap::new(),
            conv: Conversation::new(),
            past: Vec::new(),
            show_history: false,
            menu: None,
            input,
            draft: Vec::new(),
            offer: None,
            live: None,
            notice: None,
            terminal: None,
            terminal_key: None,
            scroll: ScrollHandle::new(),
            pending_ask: None,
            focus: cx.focus_handle(),
            expand_to,
            _subs: vec![sub],
        };
        for r in &p.remotes {
            p.models.insert(ProviderId::Remote(r.id()).key(), r.models());
        }
        p.detect(cx);
        p
    }

    // ── public API for hosts ────────────────────────────────────────────────────────────

    /// Open the panel (if hidden) with a new conversation and ask `question` — e.g. from a
    /// "Preguntar" button on an item. Deferred, so it can be called from inside the host's update.
    pub fn ask(&mut self, question: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        let q = question.into();
        cx.defer_in(window, move |this, window, cx| {
            if this.prefs.dock == DockMode::Hidden || this.prefs.dock == DockMode::Bar {
                this.set_dock(this.expand_to, cx);
            }
            if this.live.is_some() {
                this.input.update(cx, |s, cx| s.set_value(q, window, cx));
                return;
            }
            this.prefs.view = ViewMode::Chat;
            this.new_conversation(window, cx);
            this.send_text(q, window, cx);
        });
    }

    /// A change the app's tools proposed (mode "ask"): shown as an Aplicar / Rechazar card on
    /// the current answer. Resolved through [`AgentHost::resolve_change`].
    pub fn report_pending_change(&mut self, change: PendingChange, cx: &mut Context<Self>) {
        if let Some(live) = self.live.as_mut() {
            live.changes.retain(|c| c.id != change.id);
            live.changes.push(change);
        } else {
            if let Some(c) = self.conv.change_mut(&change.id) {
                *c = change;
            } else {
                match self.conv.messages.last_mut() {
                    Some(m) if !m.user => m.changes.push(change),
                    _ => {
                        let mut m = Message::assistant("");
                        m.changes.push(change);
                        self.conv.push(m);
                    }
                }
            }
            self.save_conversation();
        }
        cx.notify();
    }

    /// The app applied / rejected a change on its own (or it failed).
    pub fn set_change_state(&mut self, id: &str, state: ChangeState, error: Option<String>, cx: &mut Context<Self>) {
        let mut hit = false;
        if let Some(live) = self.live.as_mut()
            && let Some(c) = live.changes.iter_mut().find(|c| c.id == id)
        {
            c.state = state;
            c.error = error.clone();
            hit = true;
        }
        if let Some(c) = self.conv.change_mut(id) {
            c.state = state;
            c.error = error;
            hit = true;
            self.save_conversation();
        }
        if hit {
            cx.notify();
        }
    }

    /// Copy files into the conversation's attachments (shown in the composer until sent).
    pub fn attach_files(&mut self, files: &[PathBuf], cx: &mut Context<Self>) {
        let dir = history::attachments_dir(&self.session_dir());
        let mut failed = Vec::new();
        for f in files {
            match attach::import(f, &dir) {
                Ok(a) => {
                    if !self.draft.iter().any(|d| d.path == a.path) {
                        self.draft.push(a);
                    }
                }
                Err(e) => failed.push(e.to_string()),
            }
        }
        self.notice = (!failed.is_empty()).then(|| (t!("panel.attach.failed", detail = failed.join("; ")), true));
        cx.notify();
    }

    pub fn dock_mode(&self) -> DockMode {
        self.prefs.dock
    }

    pub fn set_dock(&mut self, mode: DockMode, cx: &mut Context<Self>) {
        if matches!(mode, DockMode::Bottom | DockMode::Right | DockMode::Focus) {
            self.expand_to = mode;
        }
        self.prefs.set_dock(mode);
        self.menu = None;
        self.save_prefs();
        cx.emit(PanelEvent::DockChanged);
        cx.notify();
    }

    /// ⌘J.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.toggle();
        self.save_prefs();
        if self.prefs.dock != DockMode::Hidden {
            self.focus_main_control(window, cx);
        }
        cx.emit(PanelEvent::DockChanged);
        cx.notify();
    }

    pub fn is_busy(&self) -> bool {
        self.live.is_some()
    }

    /// Resize from the dock's divider (saved when the drag ends, see [`Self::save_prefs`]).
    pub fn set_size(&mut self, width: Option<f32>, height: Option<f32>, cx: &mut Context<Self>) {
        if let Some(w) = width {
            self.prefs.width = w.max(history::MIN_WIDTH);
        }
        if let Some(h) = height {
            self.prefs.height = h.max(history::MIN_HEIGHT);
        }
        cx.emit(PanelEvent::DockChanged);
        cx.notify();
    }

    pub fn save_prefs(&self) {
        let _ = self.prefs.save(&self.dir);
    }

    // ── providers ───────────────────────────────────────────────────────────────────────

    pub fn detect(&mut self, cx: &mut Context<Self>) {
        if self.detecting {
            return;
        }
        self.detecting = true;
        let dir = self.dir.clone();
        let task = cx.background_executor().spawn(async move {
            let found = provider::detect_system();
            let home = otherwise_link::paths::home();
            let models: Vec<(String, Vec<Model>)> = found.iter().map(|(c, p)| (ProviderId::Cli(*c).key(), provider::models(*c, p, &home))).collect();
            (found, models, history::list(&dir))
        });
        cx.spawn(async move |this, cx| {
            let (found, models, past) = task.await;
            this.update(cx, |this, cx| {
                this.clis = Some(found);
                this.models.extend(models);
                this.past = past;
                this.detecting = false;
                if let Some(q) = this.pending_ask.take() {
                    this.start_turn(q, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn providers(&self) -> Vec<ProviderEntry> {
        let mut v: Vec<ProviderEntry> =
            self.clis.iter().flatten().map(|(c, p)| ProviderEntry { id: ProviderId::Cli(*c), label: c.label().into(), vendor: c.vendor().into(), program: Some(p.clone()), remote: None }).collect();
        v.extend(self.remotes.iter().map(|r| ProviderEntry { id: ProviderId::Remote(r.id()), label: r.label(), vendor: r.vendor(), program: None, remote: Some(r.clone()) }));
        v
    }

    /// The chosen provider: the saved one when available, else the first.
    pub(super) fn current(&self) -> Option<ProviderEntry> {
        let all = self.providers();
        let want = self.prefs.provider.clone();
        want.and_then(|w| all.iter().find(|e| e.id == w).cloned()).or_else(|| all.into_iter().next())
    }

    pub(super) fn models_of(&self, p: &ProviderId) -> Vec<Model> {
        self.models.get(&p.key()).cloned().unwrap_or_else(|| vec![Model::default_model()])
    }

    pub(super) fn current_model(&self, p: &ProviderId) -> Model {
        let models = self.models_of(p);
        let want = self.prefs.model_for(p);
        want.and_then(|w| models.iter().find(|m| m.id.as_deref() == Some(w.as_str())).cloned().or(Some(Model { id: Some(w.clone()), label: w })))
            .unwrap_or_else(|| models.first().cloned().unwrap_or_else(Model::default_model))
    }

    pub(super) fn set_provider(&mut self, p: ProviderId, cx: &mut Context<Self>) {
        if p.cli().is_none() && self.prefs.view == ViewMode::Terminal {
            self.prefs.view = ViewMode::Chat;
        }
        self.prefs.provider = Some(p);
        self.menu = None;
        self.save_prefs();
        cx.notify();
    }

    pub(super) fn set_model(&mut self, m: Model, cx: &mut Context<Self>) {
        if let Some(p) = self.current() {
            match m.id {
                Some(id) => self.prefs.models.insert(p.id.key(), id),
                None => self.prefs.models.remove(&p.id.key()),
            };
        }
        self.menu = None;
        self.save_prefs();
        cx.notify();
    }

    pub(super) fn set_permission(&mut self, m: PermissionMode, cx: &mut Context<Self>) {
        self.prefs.permission = m;
        self.menu = None;
        self.save_prefs();
        cx.notify();
    }

    pub(super) fn set_view(&mut self, v: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if v == ViewMode::Terminal && self.current().is_none_or(|p| p.id.cli().is_none()) {
            self.notice = Some((t!("panel.terminal.cli_only"), false));
            cx.notify();
            return;
        }
        self.prefs.view = v;
        self.menu = None;
        self.show_history = false;
        self.save_prefs();
        if v == ViewMode::Terminal && !self.terminal_running(cx) {
            self.launch_terminal(window, cx);
        }
        self.focus_main_control(window, cx);
        cx.notify();
    }

    fn focus_main_control(&self, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.terminal, self.prefs.view) {
            (Some(t), ViewMode::Terminal) => {
                let f = t.read(cx).focus_handle(cx);
                window.focus(&f, cx)
            }
            _ => self.input.update(cx, |s, cx| s.focus(window, cx)),
        }
    }

    // ── conversations ───────────────────────────────────────────────────────────────────

    pub(super) fn session_dir(&self) -> PathBuf {
        history::session_dir(&self.dir, &self.conv.id)
    }

    pub fn new_conversation(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.live.is_some() {
            return;
        }
        self.conv = Conversation::new();
        self.draft.clear();
        self.offer = None;
        self.show_history = false;
        self.notice = None;
        if self.terminal.is_some() {
            self.terminal = None;
            self.terminal_key = None;
        }
        cx.notify();
    }

    pub(super) fn open_conversation(&mut self, c: Conversation, cx: &mut Context<Self>) {
        if self.live.is_some() {
            return;
        }
        self.conv = c;
        self.draft.clear();
        self.show_history = false;
        self.prefs.view = ViewMode::Chat;
        self.terminal = None;
        self.terminal_key = None;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(super) fn delete_conversation(&mut self, id: String, cx: &mut Context<Self>) {
        if Conversation::delete(&self.dir, &id).is_ok() {
            self.past.retain(|c| c.id != id);
            if self.conv.id == id {
                self.conv = Conversation::new();
            }
        }
        cx.notify();
    }

    fn save_conversation(&mut self) {
        if self.conv.is_empty() {
            return;
        }
        if self.conv.save(&self.dir).is_ok() {
            let c = self.conv.clone();
            self.past.retain(|p| p.id != c.id);
            self.past.insert(0, c);
        }
    }

    // ── chat turns ──────────────────────────────────────────────────────────────────────

    pub(super) fn send_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if (text.is_empty() && self.draft.is_empty()) || self.live.is_some() {
            return;
        }
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.send_text(text, window, cx);
    }

    fn send_text(&mut self, text: String, _window: &mut Window, cx: &mut Context<Self>) {
        if self.clis.is_none() {
            self.pending_ask = Some(text);
            self.detect(cx);
            return;
        }
        self.start_turn(text, cx);
    }

    fn servers(&self, mode: PermissionMode, cx: &App) -> Vec<McpServer> {
        let mut v = self.host.mcp_servers(mode, cx);
        if self.host.include_hub()
            && !v.iter().any(|s| s.name == crate::mcp::HUB)
            && let Some(h) = McpServer::find_hub()
        {
            v.push(h);
        }
        v
    }

    fn system(&self) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        args::system_prompt(&self.host.app_name(), self.host.instructions().as_deref(), crate::i18n::locale(), &today)
    }

    fn start_turn(&mut self, text: String, cx: &mut Context<Self>) {
        if self.live.is_some() {
            return;
        }
        let history = self.conv.pairs();
        let attachments = std::mem::take(&mut self.draft);
        self.offer = None;
        self.notice = None;
        self.conv.push(Message::user(text.clone(), attachments.clone()));
        let Some(p) = self.current() else {
            let mut m = Message::assistant("");
            m.error = Some(t!("panel.none.body"));
            self.conv.push(m);
            cx.notify();
            return;
        };
        if let Some(Err(why)) = p.remote.as_ref().map(|r| r.available()) {
            let mut m = Message::assistant("");
            m.error = Some(why);
            self.conv.push(m);
            cx.notify();
            return;
        }
        let model = self.current_model(&p.id).id;
        let mode = self.prefs.permission;
        let context = self.host.context(cx);
        let system = self.system();
        let session = self.session_dir();
        self.conv.provider = Some(p.id.clone());
        self.conv.model = model.clone();
        self.save_conversation();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cancel = Cancel::new();
        self.live = Some(Live { shared: shared.clone(), cancel: cancel.clone(), text: String::new(), tools: Vec::new(), changes: Vec::new() });
        let servers = if p.remote.is_some() { Vec::new() } else { self.servers(mode, cx) };
        let conversation_id = self.conv.id.clone();
        let language = crate::i18n::locale().to_string();
        std::thread::spawn(move || {
            let sink = shared.clone();
            let mut emit = |e: StreamEvent| sink.lock().unwrap_or_else(|p| p.into_inner()).events.push(e);
            let res = match (&p.remote, p.id.cli(), &p.program) {
                (Some(r), _, _) => r.send(RemoteTurn { conversation_id, model, mode, system, context, history, text, attachments, language }, &mut emit, &cancel),
                (None, Some(cli), Some(program)) => {
                    let s = Session { cli, program, model: model.as_deref(), mode, servers: &servers, dir: &session };
                    let turn = Turn { system: &system, context: context.as_deref(), history: &history, text: &text, attachments: &attachments };
                    std::fs::create_dir_all(&session)
                        .and_then(|_| args::chat(&s, &turn))
                        .map_err(|e| AgentError::Failed(e.to_string()))
                        .and_then(|inv| crate::run::run(cli, &inv, servers.iter().map(|s| s.name.clone()).collect(), &cancel, &mut emit))
                }
                _ => Err(AgentError::Failed(String::new())),
            };
            shared.lock().unwrap_or_else(|p| p.into_inner()).done = Some(res);
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.drain(cx)).unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(l) = &self.live {
            l.cancel.cancel();
        }
        cx.notify();
    }

    /// Moves streamed events into the view; true when the run is over.
    fn drain(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(live) = self.live.as_mut() else { return true };
        let (events, done) = {
            let mut s = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut s.events), s.done.take())
        };
        let any = !events.is_empty();
        for e in events {
            match e {
                StreamEvent::Delta(d) => live.text.push_str(&d),
                StreamEvent::ToolCall(t) => live.tools.push(ToolUse { tool: t, failed: false }),
                StreamEvent::ToolResult { tool, is_error: true } => {
                    if let Some(u) = live.tools.iter_mut().rev().find(|u| u.tool == tool || tool.tool.is_empty()) {
                        u.failed = true;
                    }
                }
                _ => {}
            }
        }
        let Some(res) = done else {
            if any {
                self.scroll.scroll_to_bottom();
                cx.notify();
            }
            return false;
        };
        let Some(live) = self.live.take() else { return true };
        let mut msg = Message::assistant("");
        msg.tools = live.tools;
        msg.changes = live.changes;
        match res {
            Ok(text) => msg.text = if text.trim().is_empty() { live.text } else { text },
            Err(e) => {
                msg.text = live.text;
                msg.error = Some(error_text(&e));
            }
        }
        self.conv.push(msg);
        self.save_conversation();
        self.scroll.scroll_to_bottom();
        cx.notify();
        true
    }

    pub(super) fn resolve(&mut self, id: String, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let res = host.resolve_change(&id, approve, window, cx);
        let (state, err) = match res {
            Ok(()) => (if approve { ChangeState::Applied } else { ChangeState::Rejected }, None),
            Err(e) => (ChangeState::Failed, Some(e)),
        };
        self.set_change_state(&id, state, err, cx);
    }

    // ── drops, paste, picker ────────────────────────────────────────────────────────────

    pub(super) fn on_files(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let files: Vec<PathBuf> = files.into_iter().filter(|f| f.is_file()).collect();
        if files.is_empty() {
            return;
        }
        let actions = self.host.drop_actions(&files, cx);
        if actions.is_empty() {
            self.attach_files(&files, cx);
        } else {
            self.offer = Some(DropOffer { files, actions });
            cx.notify();
        }
    }

    pub(super) fn run_drop_action(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.offer.take() else { return };
        let outcome = match id {
            Some(id) => self.host.clone().run_drop_action(&id, &offer.files, window, cx),
            None => DropOutcome::Attach,
        };
        match outcome {
            DropOutcome::Done(msg) => self.notice = msg.map(|m| (m, false)),
            DropOutcome::Failed(e) => self.notice = Some((e, true)),
            DropOutcome::Attach => self.attach_files(&offer.files, cx),
            DropOutcome::Ask { prompt, send } => {
                self.attach_files(&offer.files, cx);
                if send && self.live.is_none() {
                    self.send_text(prompt, window, cx);
                } else {
                    self.input.update(cx, |s, cx| s.set_value(prompt, window, cx));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| {
                    if this.prefs.view == ViewMode::Terminal
                        && let Some(t) = &this.terminal
                    {
                        t.read(cx).insert_files(&paths);
                    } else {
                        this.on_files(paths, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// ⌘V in the composer: an image or copied files become attachments; text is left to the
    /// input. True when it took the paste.
    pub(super) fn paste_into_chat(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        for e in item.entries() {
            match e {
                ClipboardEntry::Image(img) => {
                    let dir = history::attachments_dir(&self.session_dir());
                    match attach::save_bytes(&img.bytes, super::terminal::view::image_ext(img.format), &dir) {
                        Ok(a) => self.draft.push(a),
                        Err(e) => self.notice = Some((t!("panel.attach.failed", detail = e), true)),
                    }
                    cx.notify();
                    return true;
                }
                ClipboardEntry::ExternalPaths(p) => {
                    self.on_files(p.paths().to_vec(), cx);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub(super) fn toggle_menu(&mut self, m: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(m) { None } else { Some(m) };
        cx.notify();
    }

    pub(super) fn use_suggestion(&mut self, s: String, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if self.live.is_none() && self.prefs.view == ViewMode::Chat {
            self.send_text(s, window, cx);
        }
    }

    // ── terminal ────────────────────────────────────────────────────────────────────────

    pub(super) fn terminal_running(&self, cx: &App) -> bool {
        self.terminal.as_ref().is_some_and(|t| t.read(cx).is_running())
    }

    /// What a terminal launched now would use; a running one with another key is stale.
    pub(super) fn terminal_settings_key(&self) -> Option<String> {
        let p = self.current()?;
        Some(format!("{}|{:?}|{:?}|{}", p.id.key(), self.current_model(&p.id).id, self.prefs.permission, self.conv.id))
    }

    pub(super) fn launch_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.current() else { return };
        let (Some(cli), Some(program)) = (p.id.cli(), p.program.clone()) else { return };
        let mode = self.prefs.permission;
        let servers = self.servers(mode, cx);
        let model = self.current_model(&p.id).id;
        let session = self.session_dir();
        let instructions = args::terminal_instructions(&self.system(), self.host.context(cx).as_deref());
        let s = Session { cli, program: &program, model: model.as_deref(), mode, servers: &servers, dir: &session };
        let inv = match args::terminal(&s, &instructions) {
            Ok(i) => i,
            Err(e) => {
                self.notice = Some((t!("panel.terminal.failed", detail = e), true));
                cx.notify();
                return;
            }
        };
        let st = PanelStyle::get(cx);
        let style = TermStyle { font_family: st.mono_font.clone(), font_size: 12.5, fg: st.terminal_fg, bg: st.terminal_bg, selection: Hsla { a: 0.3, ..st.accent } };
        let view = self.terminal.clone().unwrap_or_else(|| cx.new(|cx| TerminalView::new(style.clone(), cx)));
        let spec = Spawn { program: inv.program, args: inv.args, cwd: inv.cwd, env: inv.env };
        let attach_dir = history::attachments_dir(&session);
        view.update(cx, |v, cx| {
            v.style = style;
            v.attach_dir = Some(attach_dir);
            v.run(&spec, window, cx);
        });
        if self.terminal.is_none() {
            self._subs.push(cx.subscribe(&view, |_, _, _: &super::terminal::TermEvent, cx| cx.notify()));
        }
        self.terminal = Some(view);
        self.terminal_key = self.terminal_settings_key();
        cx.notify();
    }
}

pub(super) fn error_text(e: &AgentError) -> String {
    match e {
        AgentError::NotInstalled(c) => t!("panel.err.missing", agent = c.label()),
        AgentError::NotLoggedIn(c) => t!("panel.err.login", agent = c.label(), command = c.login_command()),
        AgentError::Cancelled => t!("panel.err.cancelled"),
        AgentError::Failed(d) => t!("panel.err.failed", detail = d),
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
