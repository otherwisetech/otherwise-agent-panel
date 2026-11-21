//! A tiny host app: a list of items as the main view, the agent panel docked next to it, the
//! installed agent CLIs plus a fake remote provider, a drop action and a pending change.
//!
//! `cargo run --example demo` (⌘J shows / hides the panel).
use gpui::*;
use gpui_component::Root;
use otherwise_agent_panel::remote::FakeRemote;
use otherwise_agent_panel::ui::{AgentDock, AgentHost, AgentPanel, DropAction, DropOutcome};
use otherwise_agent_panel::{McpServer, PendingChange, PermissionMode, RemoteProvider};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

struct Items {
    items: Vec<String>,
    selected: usize,
    panel: Option<WeakEntity<AgentPanel>>,
    focus: FocusHandle,
    proposals: usize,
}

impl Render for Items {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap(px(4.)).p(px(16.));
        for (i, it) in self.items.iter().enumerate() {
            let on = i == self.selected;
            list = list.child(div().id(("item", i)).px(px(10.)).py(px(6.)).rounded(px(6.)).when_selected(on).child(it.clone()).on_click(cx.listener(move |this, _, _, cx| {
                this.selected = i;
                cx.notify();
            })));
        }
        div()
            .track_focus(&self.focus)
            .size_full()
            .bg(rgb(0xf6f5f2))
            .text_color(rgb(0x1c1b19))
            .flex()
            .flex_col()
            .child(div().p(px(16.)).text_size(px(18.)).child("Demo items")) // i18n-ok: example app
            .child(list)
            .child(
                div()
                    .id("propose")
                    .m(px(16.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(rgb(0x1f6a72))
                    .text_color(white())
                    .cursor_pointer()
                    .child("Simulate a proposed change") // i18n-ok: example app
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.proposals += 1;
                        let change = PendingChange::new(format!("chg-{}", this.proposals), format!("Rename “{}”", this.items[this.selected]), "Proposed by the agent through the app's MCP tools.");
                        if let Some(p) = this.panel.as_ref().and_then(|p| p.upgrade()) {
                            p.update(cx, |p, cx| p.report_pending_change(change, cx));
                        }
                    })),
            )
    }
}

trait WhenSelected {
    fn when_selected(self, on: bool) -> Self;
}

impl WhenSelected for Stateful<Div> {
    fn when_selected(self, on: bool) -> Self {
        if on { self.bg(rgb(0xe0eeee)) } else { self }
    }
}

struct DemoHost {
    items: WeakEntity<Items>,
}

impl AgentHost for DemoHost {
    fn app_id(&self) -> String {
        "agent-panel-demo".into()
    }
    fn app_name(&self) -> String {
        "Demo".into()
    }
    fn mcp_servers(&self, _mode: PermissionMode, _cx: &App) -> Vec<McpServer> {
        Vec::new()
    }
    fn include_hub(&self) -> bool {
        true
    }
    fn context(&self, cx: &App) -> Option<String> {
        let items = self.items.upgrade()?;
        let it = items.read(cx);
        Some(format!("{{\"screen\":\"items\",\"selected\":{:?}}}", it.items.get(it.selected)?))
    }
    fn suggestions(&self) -> Vec<String> {
        vec!["What is selected?".into(), "Summarise my items".into()]
    }
    fn remote_providers(&self) -> Vec<Arc<dyn RemoteProvider>> {
        vec![Arc::new(FakeRemote { delay_ms: 40 })]
    }
    fn drop_actions(&self, _files: &[PathBuf], _cx: &App) -> Vec<DropAction> {
        vec![DropAction::new("import", "Import as item"), DropAction::new("ask", "Ask about this")]
    }
    fn run_drop_action(&self, action: &str, files: &[PathBuf], _w: &mut Window, cx: &mut App) -> DropOutcome {
        match action {
            "import" => {
                let names: Vec<String> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
                if let Some(items) = self.items.upgrade() {
                    items.update(cx, |it, cx| {
                        it.items.extend(names.iter().cloned());
                        cx.notify();
                    });
                }
                DropOutcome::Done(Some(format!("Imported {}", names.join(", "))))
            }
            _ => DropOutcome::Ask { prompt: "What is in this file?".into(), send: false },
        }
    }
    fn resolve_change(&self, _id: &str, approve: bool, _w: &mut Window, _cx: &mut App) -> Result<(), String> {
        let _ = approve;
        Ok(())
    }
}

fn main() {
    gpui_platform::application().with_assets(gpui_component_assets::Assets).run(|cx| {
        gpui_component::init(cx);
        otherwise_agent_panel::ui::init(cx);
        otherwise_agent_panel::i18n::set_locale("en");
        let options = WindowOptions { window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)), ..Default::default() };
        cx.spawn(async move |cx| {
            cx.open_window(options, |window, cx| {
                let items =
                    cx.new(|cx| Items { items: vec!["Invoice 001".into(), "Receipt — café".into(), "Contract draft".into()], selected: 0, panel: None, focus: cx.focus_handle(), proposals: 0 });
                let focus = items.read(cx).focus.clone();
                let host: Rc<dyn AgentHost> = Rc::new(DemoHost { items: items.downgrade() });
                let dock = cx.new(|cx| AgentDock::new(items.clone(), Some(focus), host, window, cx));
                let panel = dock.read(cx).panel().downgrade();
                items.update(cx, |it, _| it.panel = Some(panel));
                cx.new(|cx| Root::new(dock, window, cx))
            })
            .expect("open window");
        })
        .detach();
    });
}
