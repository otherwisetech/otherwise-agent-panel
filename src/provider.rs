//! Who answers: one of the agent command lines the user already pays for (Claude Code, Codex,
//! opencode), found on the login shell's PATH or where they usually install, or a remote
//! provider the host app supplies (its own AI service, see [`crate::remote`]).
//!
//! Each provider has a model list: Claude Code's aliases (a maintained list, the CLI has no
//! listing command), Codex's (its `models_cache.json` when present, else a maintained list),
//! opencode's (`opencode models`, else a short maintained list). "Default" (no `--model`) is
//! always first and lets the CLI pick.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The agent command lines the panel can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cli {
    #[serde(alias = "claude")]
    ClaudeCode,
    Codex,
    Opencode,
}

pub const CLIS: [Cli; 3] = [Cli::ClaudeCode, Cli::Codex, Cli::Opencode];

impl Cli {
    pub fn binary(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "claude",
            Cli::Codex => "codex",
            Cli::Opencode => "opencode",
        }
    }

    pub fn from_binary(s: &str) -> Option<Cli> {
        CLIS.into_iter().find(|b| b.binary() == s)
    }

    /// Product name (not translated).
    pub fn label(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "Claude Code",
            Cli::Codex => "Codex",
            Cli::Opencode => "opencode",
        }
    }

    /// What the user runs to sign in.
    pub fn login_command(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "claude login",
            Cli::Codex => "codex login",
            Cli::Opencode => "opencode auth login",
        }
    }

    /// Who receives what the agent reads (the privacy note names it).
    pub fn vendor(self) -> &'static str {
        match self {
            Cli::ClaudeCode => "Anthropic",
            Cli::Codex => "OpenAI",
            Cli::Opencode => "opencode",
        }
    }
}

/// A provider as the picker and the saved preferences know it: `claude`, `codex`, `opencode`
/// or `remote:<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Cli(Cli),
    Remote(String),
}

impl ProviderId {
    pub fn key(&self) -> String {
        match self {
            ProviderId::Cli(c) => c.binary().to_string(),
            ProviderId::Remote(id) => format!("remote:{id}"),
        }
    }

    pub fn parse(s: &str) -> Option<ProviderId> {
        if let Some(id) = s.strip_prefix("remote:") {
            return (!id.is_empty()).then(|| ProviderId::Remote(id.to_string()));
        }
        Cli::from_binary(s).map(ProviderId::Cli)
    }

    pub fn cli(&self) -> Option<Cli> {
        match self {
            ProviderId::Cli(c) => Some(*c),
            ProviderId::Remote(_) => None,
        }
    }
}

impl serde::Serialize for ProviderId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.key())
    }
}

impl<'de> serde::Deserialize<'de> for ProviderId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        // conversations saved by Otherwise Apps 0.3 say "claude_code"
        let s = if s == "claude_code" { "claude".to_string() } else { s };
        ProviderId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown provider {s}")))
    }
}

/// One entry of a model picker. `id: None` is "Default": no `--model`, the CLI decides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: Option<String>,
    pub label: String,
}

impl Model {
    pub fn new(id: &str, label: &str) -> Model {
        Model { id: Some(id.to_string()), label: label.to_string() }
    }

    pub fn default_model() -> Model {
        Model { id: None, label: crate::t!("panel.model.default") }
    }
}

/// Claude Code takes aliases that always mean the latest of a family, so this list stays right
/// as models are released; full ids still work when typed by the host.
pub fn claude_models() -> Vec<Model> {
    vec![Model::default_model(), Model::new("opus", "Opus 5.5"), Model::new("sonnet", "Sonnet 5.5"), Model::new("haiku", "Haiku 5.5"), Model::new("fable", "Fable 5.2")]
}

/// Codex's maintained list, used when it hasn't cached its own.
pub fn codex_models_fallback() -> Vec<Model> {
    vec![Model::default_model(), Model::new("gpt-5.5", "GPT-5.5"), Model::new("gpt-5.4", "GPT-5.4"), Model::new("gpt-5.3-codex", "GPT-5.3 Codex"), Model::new("gpt-5.4-mini", "GPT-5.4 mini")]
}

/// opencode's maintained list, used when `opencode models` can't be run.
pub fn opencode_models_fallback() -> Vec<Model> {
    vec![Model::default_model(), Model::new("anthropic/claude-sonnet-5-5", "Claude Sonnet 5.5"), Model::new("anthropic/claude-opus-5-5", "Claude Opus 5.5"), Model::new("openai/gpt-5.5", "GPT-5.5")]
}

/// Codex keeps the models its account can use in `~/.codex/models_cache.json`
/// (`{"models": [{"slug": "...", "display_name": "..."}]}`); `None` if absent or unreadable.
pub fn codex_models_from_cache(text: &str) -> Option<Vec<Model>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let list = v.get("models").and_then(|m| m.as_array()).or_else(|| v.as_array())?;
    let mut out = vec![Model::default_model()];
    for m in list {
        let Some(slug) = m.get("slug").or_else(|| m.get("id")).and_then(|s| s.as_str()).filter(|s| !s.is_empty()) else { continue };
        if m.get("visibility").and_then(|v| v.as_str()).is_some_and(|v| v == "hide" || v == "hidden") {
            continue;
        }
        let label = m.get("display_name").and_then(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or(slug);
        out.push(Model::new(slug, label));
    }
    (out.len() > 1).then_some(out)
}

/// `opencode models` prints one `provider/model` per line.
pub fn parse_opencode_models(text: &str) -> Vec<Model> {
    let mut out = vec![Model::default_model()];
    for l in text.lines().map(str::trim) {
        if l.is_empty() || l.contains(char::is_whitespace) || !l.contains('/') || l.starts_with('/') || l.ends_with('/') {
            continue;
        }
        if !out.iter().any(|m| m.id.as_deref() == Some(l)) {
            out.push(Model::new(l, l));
        }
    }
    out
}

/// The model list of an installed CLI (may run `opencode models`, up to a few seconds: call it
/// off the UI thread).
pub fn models(cli: Cli, program: &Path, home: &Path) -> Vec<Model> {
    match cli {
        Cli::ClaudeCode => claude_models(),
        Cli::Codex => std::fs::read_to_string(home.join(".codex").join("models_cache.json")).ok().and_then(|t| codex_models_from_cache(&t)).unwrap_or_else(codex_models_fallback),
        Cli::Opencode => {
            let out = output_with_timeout(program, &["models"], Duration::from_secs(8));
            let list = out.map(|o| parse_opencode_models(&o)).unwrap_or_default();
            if list.len() > 1 { list } else { opencode_models_fallback() }
        }
    }
}

fn output_with_timeout(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program).args(args).env("PATH", child_path()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    reader.join().ok()
}

/// `$PATH` as the user's login shell sets it (apps opened from Finder get a bare one), asked
/// once; `None` if the shell can't be run.
pub fn login_shell_path() -> Option<String> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        if cfg!(windows) {
            return std::env::var("PATH").ok();
        }
        let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
        let out = Command::new(shell).args(["-l", "-c", "printf '%s' \"$PATH\""]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!p.is_empty()).then_some(p)
    })
    .clone()
}

/// PATH for children: the login shell's (Codex and opencode may need `node`), else ours.
pub fn child_path() -> String {
    login_shell_path().or_else(|| std::env::var("PATH").ok()).unwrap_or_default()
}

/// Places these CLIs install to, for when the login shell says nothing.
pub fn usual_dirs(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [".local/bin", ".claude/local", ".opencode/bin", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin", ".codex/bin"].iter().map(|d| home.join(d)).collect();
    if cfg!(windows) {
        if let Ok(a) = std::env::var("APPDATA") {
            v.push(PathBuf::from(a).join("npm"));
        }
    } else {
        v.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].iter().map(PathBuf::from));
    }
    v
}

/// The program for `cli`: on `path` (PATH-style), else in the usual places.
pub fn find(cli: Cli, path: Option<&str>, home: &Path) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) { ["exe", "cmd"].iter().map(|e| format!("{}.{e}", cli.binary())).collect() } else { vec![cli.binary().to_string()] };
    let dirs: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs.into_iter().chain(usual_dirs(home)).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Every installed CLI, in [`CLIS`] order.
pub fn detect(path: Option<&str>, home: &Path) -> Vec<(Cli, PathBuf)> {
    CLIS.iter().filter_map(|c| find(*c, path, home).map(|p| (*c, p))).collect()
}

/// [`detect`] with the login shell's PATH and the real home.
pub fn detect_system() -> Vec<(Cli, PathBuf)> {
    detect(login_shell_path().as_deref(), &otherwise_link::paths::home())
}
