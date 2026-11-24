# otherwise-agent-panel

The AI panel shared by Otherwise Technologies' desktop apps, for [GPUI](https://www.gpui.rs) +
[gpui-component](https://github.com/longbridge/gpui-component). It docks your own **Claude
Code**, **Codex** or **opencode**, a local **Ollama** model, or the host app's own AI service next to the app, with the
app's data attached through MCP **for that session only**. Your agent configuration files are
never touched.

- **Dock modes**: Barra (a one-line bar), Abajo, Derecha (resizable, remembered) and Foco (full
  window); ⌘J shows / hides it from anywhere.
- **Pickers** like the Claude Code editor extension: agent (only the installed ones, found on
  the login shell's PATH and the usual install folders), model (Default, Opus 5.5, Sonnet 5.5,
  Haiku 5.5, Fable 5.2 for Claude; Codex's cached list; `opencode models`), permission mode
  (Solo lectura · Preguntar antes de cambiar · Automático), `+` attach, `/` commands, history,
  new conversation.
- **Chat** over each CLI's headless streaming JSON (Claude `-p --input-format stream-json
  --output-format stream-json` with image / PDF content blocks, `codex exec --json --image`,
  `opencode run --format json --file`): streamed markdown, tool chips ("expense ·
  receipts_list"), attachment thumbnails, **Aplicar / Rechazar** cards for changes the app
  reports as pending.
- **Terminal**: the interactive CLI in an embedded PTY (alacritty), launched with the
  per-session MCP config; drop a file to type its quoted path, ⌘V pastes images as files.
- **Attachments** are copied into the session folder; host apps can offer their own drop
  actions first ("Importar como recibo").
- **Ollama, built in**: when Ollama runs (`http://localhost:11434` or a configured URL) the
  picker offers "Ollama · <model>" with the models already pulled, their size and a warning for
  models large for the computer's memory. The panel runs the tool loop itself (`/api/chat` with
  `tools`): it starts the app's MCP server for each question, maps its tools to Ollama's schema
  and runs the calls, at most 8 rounds per question. Writes stay Aplicar / Rechazar cards, even
  in Automático. Models without tool support answer as plain chat. The panel never starts
  Ollama and never pulls a model; it sends `keep_alive: "5m"` and unloads the model when the
  conversation closes. Settings (in the agent menu): server URL, default model, hide Ollama.
- **Remote providers**: an app with its own authenticated AI service implements
  `RemoteProvider` and appears in the agent picker with the same chat UI.
- **History** under `<data root>/<app>/assistant/` (data root from
  [otherwise-link](https://github.com/otherwisetech/otherwise-link)), strings in es / en / pt / fr.

```
cargo run --example demo      # a tiny host app with a fake remote provider
cargo test
```

How an app adopts it: [docs/INTEGRATING.md](docs/INTEGRATING.md).

## Privacy

What you ask, what you attach and the data the agent reads through the app's tools go to the
provider you picked (Anthropic, OpenAI, opencode's provider, or the app's own service), with
your own account; the panel says so under the input. With Ollama on this computer everything
is processed locally ("Todo se procesa en este computador con Ollama"). Nothing else leaves the computer. The
agent's shell, file-editing and web tools are turned off; it reads files only inside the
session folder.

## License

MIT OR Apache-2.0. Made by [Otherwise Technologies LLC](https://otherwisetech.com).
