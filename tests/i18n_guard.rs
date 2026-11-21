//! The i18n rule: every language has every key with the same placeholders, every key the code
//! uses exists, every catalog key is used, and UI code passes no hard-coded text to the screen
//! (escape hatch: end the line with `// i18n-ok`).
use otherwise_agent_panel::i18n;
use std::collections::BTreeSet;
use std::path::Path;

/// Keys built at runtime (`format!("panel.perm.{}", …)`); every key under them counts as used.
const DYNAMIC: &[&str] = &["panel.perm.", "panel.dock."];

fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push((p.display().to_string(), std::fs::read_to_string(&p).unwrap()));
        }
    }
}

fn placeholders(v: &str) -> BTreeSet<String> {
    v.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(a, _)| a.to_string())).collect()
}

#[test]
fn catalogs_agree() {
    let reference: BTreeSet<_> = i18n::keys(i18n::DEFAULT).into_iter().collect();
    assert!(reference.len() > 40);
    let mut problems = Vec::new();
    for lang in i18n::languages() {
        let keys: BTreeSet<_> = i18n::keys(lang).into_iter().collect();
        problems.extend(reference.difference(&keys).map(|k| format!("{lang}: missing {k}")));
        problems.extend(keys.difference(&reference).map(|k| format!("{lang}: extra {k}")));
        for k in &keys {
            let v = i18n::lookup(lang, k).unwrap();
            if v.is_empty() {
                problems.push(format!("{lang}: empty {k}"));
            }
            if placeholders(v) != placeholders(i18n::lookup(i18n::DEFAULT, k).unwrap_or("")) {
                problems.push(format!("{lang}: placeholders of {k} differ"));
            }
        }
    }
    assert_eq!(i18n::languages(), ["en", "es", "fr", "pt"]);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn code_and_catalogs_agree_and_no_hard_coded_text() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    for (name, src) in &files {
        for (n, line) in src.lines().enumerate() {
            // every "panel.x.y" literal is a key
            for part in line.split('"').skip(1).step_by(2) {
                if part.starts_with("panel.") && !part.contains(' ') && !part.ends_with('.') && !part.contains('{') {
                    used.insert(part.to_string());
                }
            }
            if line.contains("i18n-ok") || line.trim_start().starts_with("//") || !name.contains("/ui/") {
                continue;
            }
            for sink in [".child(\"", "Tooltip::new(\"", "chip(\"", "menu_title(s, \""] {
                if let Some(i) = line.find(sink) {
                    let lit: String = line[i + sink.len()..].chars().take_while(|c| *c != '"').collect();
                    if lit.chars().any(|c| c.is_alphabetic()) {
                        problems.push(format!("{name}:{}: hard-coded text {lit:?}", n + 1));
                    }
                }
            }
        }
    }
    let catalog: BTreeSet<String> = i18n::keys(i18n::DEFAULT).into_iter().map(str::to_string).collect();
    for k in &used {
        if !catalog.contains(k) {
            problems.push(format!("key used in code but missing from the catalogs: {k}"));
        }
    }
    for k in &catalog {
        if !used.contains(k) && !DYNAMIC.iter().any(|p| k.starts_with(p)) {
            problems.push(format!("catalog key never used: {k}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn lookup_locale_and_overrides() {
    // one test touches the global locale and overrides, to avoid races between tests
    i18n::set_locale("en-US");
    assert_eq!(i18n::locale(), "en");
    assert_eq!(otherwise_agent_panel::t!("panel.title", app = "expense"), "expense assistant");
    i18n::set_locale("pt_BR");
    assert_eq!(otherwise_agent_panel::t!("panel.perm.ask"), "Perguntar antes de alterar");
    i18n::set_locale("xx");
    assert_eq!(i18n::locale(), "es");
    assert_eq!(otherwise_agent_panel::PermissionMode::ReadOnly.label(), "Solo lectura");
    i18n::override_label("panel.title", "Asistente {app} Pro");
    assert_eq!(otherwise_agent_panel::t!("panel.title", app = "expense"), "Asistente expense Pro");
    i18n::clear_overrides();
    assert_eq!(otherwise_agent_panel::t!("no.such.key"), "no.such.key");
    assert_eq!(otherwise_agent_panel::t!("panel.title", app = "expense"), "Asistente de expense");
}
