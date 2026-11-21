//! Attachments: files dropped, pasted or picked are *copied* into the session's
//! `attachments/` folder (the agent never gets a path into the user's own folders, and the
//! conversation keeps working if the original moves), then passed on the way each provider
//! takes them: Claude Code image / PDF content blocks, Codex `--image`, opencode `--file`, and
//! for anything else the copied path in the prompt (the agent may `Read` inside the session).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Largest file copied in (bigger ones are refused, not truncated).
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest image / PDF sent inline as a content block (Claude's own request limit is ~32 MB).
pub const MAX_INLINE: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Image,
    Pdf,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// The copy inside the session folder.
    pub path: PathBuf,
    /// The original file name, for display.
    pub name: String,
    pub kind: Kind,
    pub mime: String,
    pub bytes: u64,
}

pub fn mime_of(path: &Path) -> (&'static str, Kind) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => ("image/png", Kind::Image),
        "jpg" | "jpeg" => ("image/jpeg", Kind::Image),
        "gif" => ("image/gif", Kind::Image),
        "webp" => ("image/webp", Kind::Image),
        "pdf" => ("application/pdf", Kind::Pdf),
        "txt" | "md" | "csv" | "log" => ("text/plain", Kind::Other),
        "json" => ("application/json", Kind::Other),
        _ => ("application/octet-stream", Kind::Other),
    }
}

#[derive(Debug)]
pub enum AttachError {
    NotAFile(PathBuf),
    TooLarge(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::NotAFile(p) => write!(f, "not a file: {}", p.display()),
            AttachError::TooLarge(p) => write!(f, "file too large: {}", p.display()),
            AttachError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// File names that are safe everywhere and keep the extension: `Recibo café (1).pdf` →
/// `Recibo_caf_1.pdf`.
pub fn safe_name(name: &str) -> String {
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    let clean = |s: &str, max: usize| -> String {
        let mut out = String::new();
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                out.push(c);
            } else if !out.ends_with('_') {
                out.push('_');
            }
        }
        out.trim_matches(['_', '.']).chars().take(max).collect()
    };
    let stem = clean(stem, 60);
    let stem = if stem.is_empty() { "file".to_string() } else { stem };
    let ext = clean(ext, 10).to_ascii_lowercase();
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

/// A path in `dir` for `name` that doesn't exist yet (`a.png`, `a-2.png`, …).
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str());
    (2..)
        .map(|n| {
            dir.join(match ext {
                Some(e) => format!("{stem}-{n}.{e}"),
                None => format!("{stem}-{n}"),
            })
        })
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn describe(path: PathBuf, name: String) -> Result<Attachment, AttachError> {
    let bytes = std::fs::metadata(&path).map_err(AttachError::Io)?.len();
    let (mime, kind) = mime_of(&path);
    Ok(Attachment { path, name, kind, mime: mime.into(), bytes })
}

/// Copy `src` into `dir` (created if needed).
pub fn import(src: &Path, dir: &Path) -> Result<Attachment, AttachError> {
    let meta = std::fs::metadata(src).map_err(|_| AttachError::NotAFile(src.to_path_buf()))?;
    if !meta.is_file() {
        return Err(AttachError::NotAFile(src.to_path_buf()));
    }
    if meta.len() > MAX_BYTES {
        return Err(AttachError::TooLarge(src.to_path_buf()));
    }
    // already ours (re-attached from this session)
    if let (Ok(a), Ok(b)) = (src.parent().map(Path::to_path_buf).unwrap_or_default().canonicalize(), dir.canonicalize())
        && a == b
    {
        let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
        return describe(src.to_path_buf(), name);
    }
    std::fs::create_dir_all(dir).map_err(AttachError::Io)?;
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let dest = free_path(dir, &safe_name(&name));
    std::fs::copy(src, &dest).map_err(AttachError::Io)?;
    describe(dest, name)
}

/// Save pasted image bytes (`ext` like `png`) as a new file in `dir`.
pub fn save_bytes(bytes: &[u8], ext: &str, dir: &Path) -> Result<Attachment, AttachError> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AttachError::TooLarge(dir.to_path_buf()));
    }
    std::fs::create_dir_all(dir).map_err(AttachError::Io)?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = format!("pasted-{secs}.{}", safe_name(ext).trim_start_matches('.'));
    let dest = free_path(dir, &name);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&dest).map_err(AttachError::Io)?;
    std::io::Write::write_all(&mut f, bytes).map_err(AttachError::Io)?;
    describe(dest, name)
}

/// A path quoted for a POSIX shell / the agent's input line: `'/a b/it'\''s.png'`; plain paths
/// stay as they are. On Windows, double quotes.
pub fn quote_path(path: &Path) -> String {
    let s = path.display().to_string();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c) || (cfg!(windows) && c == '\\')) {
        return s;
    }
    if cfg!(windows) { format!("\"{}\"", s.replace('"', "")) } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

/// Dropped / pasted paths as typed text: each quoted, space separated, with a trailing space.
pub fn paths_to_text(paths: &[PathBuf]) -> String {
    let mut s: String = paths.iter().map(|p| format!("{} ", quote_path(p))).collect();
    if s.is_empty() {
        return s;
    }
    s.insert(0, ' ');
    s
}

/// Standard base64 (for inline content blocks).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Human size: `812 B`, `14 KB`, `3.2 MB`.
pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}
