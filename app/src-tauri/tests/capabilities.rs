//! Each window may call exactly the app commands its UI uses (`capabilities/<window>.json`),
//! checked against the Svelte sources so a new call cannot be forgotten in the allowlist and a
//! window cannot quietly gain another window's commands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", p.as_ref().display()))
}

/// `api.name` -> command, from `api.ts` lines like `start: () => invoke<void>("meeting_start"),`.
fn api_commands(ui: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut name = None;
    for line in read(ui.join("lib/api.ts")).lines() {
        let t = line.trim();
        if let Some((n, _)) = t.split_once(':')
            && n.chars().all(|c| c.is_ascii_alphanumeric())
            && !n.is_empty()
        {
            name = Some(n.to_string());
        }
        if let Some(rest) = t.split("invoke").nth(1)
            && let Some(cmd) = rest.split('"').nth(1)
            && let Some(n) = name.clone()
        {
            out.insert(n, cmd.to_string());
        }
    }
    out
}

/// Commands a Svelte file (and the shared components it imports from `lib/`) calls.
fn used(ui: &Path, file: &str, api: &BTreeMap<String, String>) -> BTreeSet<String> {
    let src = read(ui.join(file));
    let mut out = BTreeSet::new();
    for part in src.split("api.").skip(1) {
        let name: String = part.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if let Some(cmd) = api.get(&name) {
            out.insert(cmd.clone());
        }
    }
    for line in src.lines() {
        if let Some(lib) = line.trim().strip_prefix("import ").and_then(|l| l.split("\"../lib/").nth(1))
            && let Some(f) = lib.strip_suffix("\";")
            && f.ends_with(".svelte")
        {
            out.extend(used(ui, &format!("lib/{f}"), api));
        }
    }
    out
}

fn allowed(window: &str) -> BTreeSet<String> {
    let cap: serde_json::Value = serde_json::from_str(&read(format!("capabilities/{window}.json"))).unwrap();
    assert_eq!(cap["windows"], serde_json::json!([window]));
    cap["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
        .map(|c| c.replace('-', "_"))
        .collect()
}

#[test]
fn each_window_may_call_exactly_what_its_ui_calls() {
    let ui = Path::new("../ui/src");
    let api = api_commands(ui);
    assert!(api.len() > 30, "parsed {} commands from api.ts", api.len());
    for (window, file) in [
        ("meeting", "meeting/Meeting.svelte"),
        ("pill", "pill/Pill.svelte"),
        ("settings", "settings/Settings.svelte"),
        ("onboarding", "onboarding/Onboarding.svelte"),
    ] {
        assert_eq!(allowed(window), used(ui, file, &api), "capabilities/{window}.json");
    }
    // The ones that matter most: paste and settings stay out of the meeting window.
    let meeting = allowed("meeting");
    assert!(!meeting.contains("dictation_paste") && !meeting.contains("config_set"));
}
