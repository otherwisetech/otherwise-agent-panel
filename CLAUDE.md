# otherwise-agent-panel — for agents and contributors

The shared AI panel of Otherwise Technologies' desktop apps (Otherwise Apps, expense, Vigía,
Nodo, …). Open source (MIT OR Apache-2.0). Apps depend on it by git tag. Never mention or
reuse code from unrelated companies' products here.

## Layout

```
src/provider.rs   Cli (claude / codex / opencode), ProviderId (+ remote:<id>), detection on the
                  login shell's PATH + usual dirs, model lists (maintained / Codex cache /
                  `opencode models`)
src/permission.rs PermissionMode → McpWrites (Off / Propose / On) + each CLI's flags
src/mcp.rs        McpServer, per-session configs: Claude --mcp-config JSON, Codex -c overrides,
                  opencode OPENCODE_CONFIG_CONTENT; find_hub()
src/args.rs       Session / Turn → Invocation for chat (headless) and terminal; system prompt,
                  prompt text, Claude stream-json user message with image / PDF blocks
src/stream.rs     Parser for Claude stream-json, Codex --json, opencode --format json;
                  StreamEvent, ToolRef, AgentError, login-error detection
src/run.rs        run a chat turn (process group, cancel, temp files removed)
src/ollama.rs     built-in Ollama: /api/tags + /api/show, status (never starts / pulls), the
                  /api/chat tool loop (MAX_STEPS, Auto → proposals), MCP → Ollama schema,
                  keep_alive / unload, memory warning
src/mcp_client.rs the panel's own stdio MCP client (initialize, tools/list, tools/call)
src/http.rs       tiny blocking HTTP/1.1 client (http:// only, chunked) for Ollama
src/remote.rs     RemoteProvider trait (host-supplied AI services), Cancel, FakeRemote
src/attach.rs     copy into the session, pasted images, quoting, base64
src/history.rs    conversations, session dirs, pending changes, Prefs (.panel.json), DockMode
src/i18n.rs       embedded locales/<lang>/panel.ftl, t!, set_locale, override_label
src/ui/           (feature "ui") dock.rs AgentDock, panel.rs AgentPanel state, chat.rs render,
                  host.rs AgentHost trait, style.rs PanelStyle, widgets.rs, terminal/ (PTY model,
                  view, keys, colours on Zed's alacritty fork)
examples/demo.rs  a tiny host with a fake remote provider
tests/            core.rs (detection, args, permissions, attachments, history), streams.rs
                  (fixtures, fake CLI run, fake remote), ollama.rs (mock Ollama server, fake
                  MCP server), i18n_guard.rs
```

## Rules

- **Never write the user's agent configuration** (~/.claude*, ~/.codex, ~/.config/opencode).
  Everything goes on the command line / environment or into the session folder under the data
  root. Tests use fake CLIs and temp dirs; they never run a real agent or touch the real data
  root. **Tests never call the real Ollama** (tests/ollama.rs has a mock server); the panel
  never starts Ollama or pulls a model.
- The agent's own shell / edit / web tools stay off; only `Read` (session folder) for Claude.
- **i18n is mandatory**: no user-facing literal in `src/ui`; add every key to all four
  `locales/<lang>/panel.ftl`. `tests/i18n_guard.rs` checks keys, placeholders, usage and
  hard-coded `.child("…")`. Product names and ids end the line with `// i18n-ok` if needed.
  Prompts for the model are English.
- Public API (`AgentHost`, `RemoteProvider`, `AgentPanel::{ask, report_pending_change,
  set_change_state, attach_files}`, history formats) changes only compatibly within 0.x minor
  bumps; history files keep loading (schema number, readers skip newer).
- GPUI pins: Cargo.lock pins zed `eb5483528be607b497afdaad0a237bebeb191249` and gpui-component
  `972a3ebfd01afca7da6d8b6f31c9a51288ea5565`. The manifest has no `rev` so hosts share one copy.
  Never `cargo update` them.
- In test modules inside `src/ui` don't `use super::*` (gpui's glob exports its own `test`).
- Comments: sparse, only non-obvious why.

## Validate

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --no-default-features --lib -- -D warnings
cargo fmt --check
```

Release: bump `version` in Cargo.toml, tag `vX.Y.Z` on main once CI is green.
