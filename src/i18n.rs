//! The panel's own strings, so every app that adopts it gets the same words in es / en / pt / fr.
//!
//! Catalogs are `locales/<lang>/panel.ftl` (one `key = value` per line, `#` comments, `{name}`
//! placeholders), embedded at build time. The host sets the language with [`set_locale`] (usually
//! whatever its own i18n uses) and may replace any label with [`override_label`] — for example
//! to call the panel "Asistente de expense". Every key exists in every language
//! (`tests/i18n_guard.rs`), and every key used in `src/` exists in the catalogs.
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

pub const DEFAULT: &str = "es";

static CATALOG: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en/panel.ftl")),
    ("es", include_str!("../locales/es/panel.ftl")),
    ("fr", include_str!("../locales/fr/panel.ftl")),
    ("pt", include_str!("../locales/pt/panel.ftl")),
];

type Table = HashMap<&'static str, HashMap<&'static str, &'static str>>;

fn tables() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| CATALOG.iter().map(|(lang, src)| (*lang, parse(src).collect())).collect())
}

fn parse(src: &'static str) -> impl Iterator<Item = (&'static str, &'static str)> {
    src.lines().filter_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((k.trim(), v.trim()))
    })
}

static CURRENT: RwLock<&'static str> = RwLock::new(DEFAULT);
static OVERRIDES: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// The bundled language codes.
pub fn languages() -> Vec<&'static str> {
    CATALOG.iter().map(|(c, _)| *c).collect()
}

pub fn locale() -> &'static str {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// Switch language: `"es"`, `"en-US"`, `"pt_BR"`… Unknown languages fall back to Spanish.
pub fn set_locale(code: &str) {
    let base = code.split(['-', '_', '.']).next().unwrap_or("").to_lowercase();
    let found = CATALOG.iter().map(|(c, _)| *c).find(|c| *c == base).unwrap_or(DEFAULT);
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = found;
}

/// Replace one label in every language (the host's wording wins over the catalog).
pub fn override_label(key: &str, text: impl Into<String>) {
    OVERRIDES.write().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key.to_string(), text.into());
}

pub fn clear_overrides() {
    *OVERRIDES.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The text of `key` in `lang` (no overrides); used by tests.
pub fn lookup(lang: &str, key: &str) -> Option<&'static str> {
    tables().get(lang).and_then(|m| m.get(key)).copied()
}

/// Every key of a language, sorted.
pub fn keys(lang: &str) -> Vec<&'static str> {
    let mut v: Vec<_> = tables().get(lang).map(|m| m.keys().copied().collect()).unwrap_or_default();
    v.sort_unstable();
    v
}

/// Translate with `{name}` placeholders. Unknown key → the key itself (visible, never panics).
pub fn tr(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let over = OVERRIDES.read().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(key).cloned());
    let mut s = over.unwrap_or_else(|| lookup(locale(), key).or_else(|| lookup(DEFAULT, key)).unwrap_or(key).to_string()).replace("\\n", "\n");
    for (name, val) in args {
        s = s.replace(&format!("{{{name}}}"), &val.to_string());
    }
    s
}

/// `t!("key")`, `t!("key", name = value, …)`
#[macro_export]
macro_rules! t {
    ($key:expr) => { $crate::i18n::tr($key, &[]) };
    ($key:expr, $($name:ident = $val:expr),+ $(,)?) => { $crate::i18n::tr($key, &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),+]) };
}
