# Adopting otherwise-agent-panel in an app

This is the checklist for an Otherwise desktop app (GPUI + gpui-component) that wants the
shared AI panel. Otherwise Apps is the reference adopter.

## 1. Depend on it

```toml
[dependencies]
otherwise-agent-panel = { git = "https://github.com/otherwisetech/otherwise-agent-panel", tag = "v0.2.0" }
```

The panel uses the same git URLs for `gpui` / `gpui-component` as every Otherwise app, without a
`rev`, so a single copy is built and **your** `Cargo.lock` decides the revision. Keep it pinned to
zed `eb5483528be6…` and gpui-component `972a3ebfd01a…`; never `cargo update` them. Adding the
crate only adds lock entries (alacritty_terminal, libc); check the diff of `Cargo.lock`.

## 2. Make the dock the window's root view

```rust
gpui_component::init(cx);
otherwise_agent_panel::ui::init(cx);                 // ⌘J / Ctrl-J, Escape in menus
otherwise_agent_panel::i18n::set_locale(my_i18n::locale());

cx.open_window(options, |window, cx| {
    let main = cx.new(|cx| MyApp::new(window, cx));
    let focus = main.read(cx).focus_handle.clone();          // gets focus back when the panel hides
    let host: Rc<dyn AgentHost> = Rc::new(MyHost { app: main.downgrade() });
    let dock = cx.new(|cx| AgentDock::new(main.clone(), Some(focus), host, window, cx));
    let panel = dock.read(cx).panel().downgrade();
    main.update(cx, |m, _| m.panel = Some(panel));           // for "Ask about this" buttons
    cx.new(|cx| Root::new(dock, window, cx))
})
```

Your view no longer needs an assistant page: the panel is available from every screen. Call
`panel.update(cx, |p, cx| p.ask(question, window, cx))` from an "Ask" button (it opens the
panel, starts a new conversation and sends; it is deferred, so calling it from inside your own
update is fine). Set `PanelStyle` as a global if your design tokens differ from the
gpui-component theme, and update it when the theme changes.

## 3. Implement `AgentHost`

| method | what to return |
|---|---|
| `app_id()` | the id under the data root (`expense`): history goes to `<data root>/<id>/assistant/` |
| `app_name()` | shown in the title ("Asistente de expense") and told to the agent |
| `mcp_servers(mode, cx)` | the app's MCP server(s) for this permission mode (see 4) |
| `include_hub()` | `true` to also attach the Otherwise hub (all installed apps) when found |
| `context(cx)` | "lo que estoy viendo": a short text / JSON of the current screen and selection |
| `instructions()` | extra English instructions for the agent |
| `suggestions()` | example questions (empty state and the `/` menu) |
| `remote_providers()` | the app's own AI service(s), see 6 |
| `drop_actions(files, cx)` / `run_drop_action(id, files, window, cx)` | app-specific actions for dropped files, offered before "attach" |
| `resolve_change(id, approve, window, cx)` | the user pressed Aplicar / Rechazar on a pending change |

The panel calls these outside your view's own updates, so reading your entity through a
`WeakEntity` is safe. Strings come from the crate's es / en / pt / fr catalogs;
`i18n::override_label(key, text)` replaces any of them.

## 4. Permission modes and your MCP server

The picker has three modes. The panel maps each to the CLI's own flags (see
`src/permission.rs`) and asks you for the matching server:

| mode | `mode.writes()` | your server should |
|---|---|---|
| Solo lectura | `McpWrites::Off` | offer read tools only |
| Preguntar antes de cambiar | `McpWrites::Propose` | offer write tools that **don't** write: they record a proposal, return "pending approval" to the agent, and the app calls `panel.report_pending_change(PendingChange::new(id, title, detail), cx)`; on `resolve_change(id, true)` the app applies it |
| Automático | `McpWrites::On` | offer write tools that write (and record what changed) |

The agent's own shell / edit / web tools stay off in every mode; it may only `Read` inside the
session folder (attachments). The panel never writes the user's Claude / Codex / opencode
configuration: Claude Code gets a temporary `--mcp-config` (in the session folder) with
`--strict-mcp-config`, Codex `-c mcp_servers.<name>.*` overrides, opencode
`OPENCODE_CONFIG_CONTENT`. In Terminal mode the interactive CLI asks before each tool itself in
"Preguntar"; in Chat mode (headless) "Preguntar" is your pending-change cards.

### The single-writer socket pattern (expense)

An app whose data file must have one writer (expense's SQLite book) should not let a second
process write it. Pattern:

1. While the GUI runs it listens on a local socket: `<data root>/<app>/run/app.sock` (Unix
   domain socket, `0600`) or a named pipe `\\.\pipe\otherwise-<app>-<user>` on Windows. It writes
   its path and pid to `<data root>/<app>/run/app.json`.
2. `mcp_servers(mode)` returns the app's CLI as a bridge:
   `McpServer::new("expense", cli, &["mcp", "--via-app", socket, "--writes", "off|propose|on"])`.
3. The bridge speaks MCP on stdio to the agent and forwards every `tools/call` as one JSON line
   over the socket; the GUI runs it on its own database connection and answers. Proposals are
   kept by the GUI, which calls `report_pending_change` itself (it owns the panel).
4. If the GUI isn't running (the agent CLI was started elsewhere), the bridge serves read-only
   from the file, or refuses writes with a clear message.
5. The socket accepts only the current user (file mode / pipe ACL) and only tool calls; no
   arbitrary SQL. Never put the socket in a synced cloud folder if the data root is one: use
   the OS temp dir then, and point to it from `app.json`.

## 5. Attachments and drops

Dropped, pasted (⌘V images) or picked files are copied to
`<data root>/<app>/assistant/sessions/<conversation>/attachments/` and passed to the agent:
Claude Code as image / PDF content blocks, Codex `--image`, opencode `--file`, other files by
path. With `drop_actions` your actions come first ("Importar como recibo", "Preguntar sobre
esto"); return `DropOutcome::Done(Some(msg))`, `Attach`, `Ask { prompt, send }` or `Failed(e)`.
In Terminal mode a drop types the copied file's quoted path into the agent.

## 6. Your own AI service (remote providers)

An app with its own authenticated AI service (Vcity's agent, for example) adds it next to the
CLIs:

```rust
struct VcityAi { client: Arc<ApiClient> }

impl RemoteProvider for VcityAi {
    fn id(&self) -> String { "vcity".into() }            // saved as "remote:vcity"
    fn label(&self) -> String { "Vcity AI".into() }
    fn vendor(&self) -> String { "Vcity".into() }         // named in the privacy note
    fn models(&self) -> Vec<Model> { vec![Model::default_model(), Model::new("deep", "Vcity Deep")] }
    fn available(&self) -> Result<(), String> { self.client.signed_in().then_some(()).ok_or("Inicia sesión en Vcity".into()) }
    fn send(&self, turn: RemoteTurn, emit: &mut dyn FnMut(StreamEvent), cancel: &Cancel) -> Result<String, AgentError> {
        // POST turn.text + uploads of turn.attachments to the conversation turn.conversation_id
        // (create it the first time), then for each server-sent event:
        //   emit(StreamEvent::Delta(text)) / ToolCall(ToolRef::new(Some("vcity"), name)) /
        //   ToolResult { tool, is_error } / Error(msg); return Err(AgentError::Cancelled) once
        //   cancel.is_cancelled().
        todo!()
    }
}
```

`send` runs on a background thread; the panel renders the events in the same chat (no Terminal
mode for remote providers), with the same attachments and permission modes — what a mode means
for your service is yours to decide (`turn.mode`). Report proposals with
`report_pending_change` just like an MCP server would. `remote::FakeRemote` is a scripted
example used by the tests and `examples/demo.rs`.

## 7. Ollama (built in, nothing to do)

Every app gets Ollama in the agent picker when it runs on the computer (or at the URL set in
the panel's Ollama settings) and has models. The panel itself is the agent: it starts the
servers from your `mcp_servers(mode)` for the conversation, lists their tools, offers them to
the model and runs the calls. Two things to know:

- **Automático is asked as Preguntar**: with Ollama, `mcp_servers` is called with
  `PermissionMode::Ask` even in Automático, so writes are always proposals your app reports
  with `report_pending_change`.
- Your server must answer `initialize` / `tools/list` / `tools/call` over stdio (any MCP server
  does). Tool results longer than 16 KB are cut before they reach the model; keep read tools
  concise and paged.

## 8. Before you ship

- `cargo test` in the panel repo passes on macOS, Linux and Windows (CI).
- Your app's i18n guard still passes (the panel's strings live in the panel).
- History written by the panel is plain JSON; if your app had its own assistant history in
  `<data root>/<app>/assistant/`, schema 1 conversations load as they are.
