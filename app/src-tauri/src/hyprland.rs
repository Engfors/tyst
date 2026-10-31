//! Hyprland (Omarchy and other Hyprland 0.53+ sessions): window rules for the meeting window
//! and the dictation pill.
//!
//! Wayland clients cannot keep themselves above other windows, refuse focus on show, or position
//! themselves. Hyprland does it from a rules file Tyst writes and the user loads. Tyst never
//! edits the user's Hyprland config. Classic configs (Hyprland 0.53+) `source` `hyprland.conf`.
//! Omarchy's Lua config (Hyprland 0.56) cannot `source` that file; it `dofile`s `hyprland.lua`,
//! which calls upstream `hl.window_rule`. Omarchy's `o.window` is a helper around the same rules.
//!
//! `no_initial_focus` keeps a newly shown window from taking the keyboard. `no_focus` is not
//! set: it would block typing into "Name this meeting". IPC ([`tyst_platform::hyprland`]) places
//! the pill, focuses it when dictation needs the keyboard, and saves the meeting window's
//! position, which a rule cannot remember for a window that hides instead of closing.

use std::path::{Path, PathBuf};

use tyst_platform::hyprland::is_hyprland;

use crate::windows::{MEETING_TITLE, PILL_TITLE};

/// Hyprland release whose window-rule syntax this file is written for, and every release after it.
pub const RULES_SINCE: &str = "0.53";

/// Binds to add when the GlobalShortcuts portal does not bind a key. `aring` is the Å keysym.
pub const BIND_HINT: &str = "\
bind = CTRL, aring, exec, tyst --dictate
bind = CTRL SHIFT, aring, exec, tyst --toggle-meeting";

pub fn rules_path() -> PathBuf {
    crate::config::config_dir().join("hyprland.conf")
}

pub fn lua_rules_path() -> PathBuf {
    crate::config::config_dir().join("hyprland.lua")
}

/// Confirmed Wayland class regex. `None` until `hyprctl clients -j` shows the real class
/// (expected `^(tyst)$`). Setting this is the only edit needed to match on class as well as title.
const WINDOW_CLASS: Option<&str> = None;

/// Line to add in `~/.config/hypr/hyprland.lua` after `require("default.hypr.omarchy")`.
/// The path is a Lua string, so a newline in it cannot start a new statement.
pub fn omarchy_dofile(lua_path: &Path) -> String {
    format!("dofile(\"{}\")", lua_string(&lua_path.display().to_string()))
}

/// The rules file. `source_path` is the line the user adds to their own Hyprland config.
pub fn rules_document(source_path: &Path) -> String {
    let path = source_path.display();
    format!(
        "\
# Tyst window rules for a classic Hyprland {RULES_SINCE} config (and newer, until Lua-only).
# https://wiki.hypr.land/Configuring/Window-Rules/
#
# This file is not valid in a Lua-only config (current Omarchy). Use hyprland.lua there.
# Tyst never edits your config. For a classic config, add:
#
#   source = {path}
#
# Rules match the window title. The Wayland app id is the GTK program name (`tyst` in
# StartupWMClass) unless enableGTKAppId is set, which it is not, so the class is not matched.
# no_initial_focus keeps a newly shown window from taking the keyboard.
# no_focus is not set: it would block typing into \"Name this meeting\".

{meeting}
{pill}\
",
        meeting = rule_block("tyst-meeting-window", MEETING_TITLE),
        pill = rule_block("tyst-dictation-pill", PILL_TITLE),
    )
}

fn rule_block(name: &str, title: &str) -> String {
    let class = WINDOW_CLASS.map(|c| format!("    match:class = {c}\n")).unwrap_or_default();
    format!(
        "\
windowrule {{
    name = {name}
{class}    match:title = {title_re}

    float = on
    pin = on
    no_initial_focus = on
    border_size = 0
}}
",
        title_re = re2_exact(title),
    )
}

/// Hyprland 0.56 Lua rules (`hl.window_rule`). Omarchy's `o.window(match, effects)` is a local
/// helper; this file calls the upstream function so it loads without `o`.
pub fn lua_rules_document(dofile_path: &Path) -> String {
    let line = omarchy_dofile(dofile_path);
    format!(
        "\
-- Tyst window rules for Hyprland 0.56 Lua config.
-- https://wiki.hypr.land/Configuring/Basics/Window-Rules/
--
-- Omarchy's o.window(match, effects) wraps the same rules. This file calls hl.window_rule
-- so it does not depend on that helper.
--
-- In ~/.config/hypr/hyprland.lua, after require(\"default.hypr.omarchy\"):
--
--   {line}
--
-- A classic config should source hyprland.conf instead. This file is not a hyprland.conf.
-- no_focus is not set: it would block typing into \"Name this meeting\".

{meeting}
{pill}\
",
        meeting = lua_rule("tyst-meeting-window", MEETING_TITLE),
        pill = lua_rule("tyst-dictation-pill", PILL_TITLE),
    )
}

fn lua_rule(name: &str, title: &str) -> String {
    let class = WINDOW_CLASS.map(|c| format!(" class = \"{}\",", lua_string(c))).unwrap_or_default();
    format!(
        "\
hl.window_rule({{
  name = \"{name}\",
  match = {{ title = \"{title}\",{class} }},
  float = true,
  pin = true,
  no_initial_focus = true,
  border_size = 0,
}})
",
        title = lua_string(&re2_exact(title)),
    )
}

/// Escape a Lua double-quoted string.
fn lua_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// A RE2 regex that matches `title` and nothing else. Hyprland expands `$NAME` only when a
/// variable name follows the `$`, so the end anchor stays literal.
fn re2_exact(title: &str) -> String {
    let mut out = String::from("^(");
    for c in title.chars() {
        if matches!(c, '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push_str(")$");
    out
}

/// Writes (or refreshes) the rules file and asks Hyprland to reload when the file changed.
/// Does nothing outside Hyprland. The reload only applies the rules once the user's config
/// sources the file.
pub fn install_rule() -> Result<(), String> {
    if !is_hyprland() {
        return Ok(());
    }
    let path = rules_path();
    let lua_path = lua_rules_path();
    let changed = write_if_changed(&path, rules_document(&path).as_bytes())?;
    let changed = write_if_changed(&lua_path, lua_rules_document(&lua_path).as_bytes())? || changed;
    if changed {
        match tyst_platform::hyprland::Hyprland::connect() {
            Ok(h) => {
                // The socket read must not run on the UI thread. The process stays up, so the
                // thread finishing after setup returns is fine.
                if let Err(e) = std::thread::Builder::new().name("hyprland-reload".into()).spawn(move || {
                    if let Err(e) = h.reload_blocking() {
                        log::warn!("could not ask Hyprland to reload; the rules apply on the next reload: {e}");
                    } else {
                        log::info!("Hyprland reloaded its config for the Tyst window rules");
                    }
                }) {
                    log::warn!("could not start the Hyprland reload: {e}");
                }
            }
            Err(e) => log::warn!("could not ask Hyprland to reload; the rules apply on the next reload: {e}"),
        }
    }
    log::info!(
        "Hyprland window rules: {} and {} (Omarchy: dofile the lua file from hyprland.lua)",
        path.display(),
        lua_path.display()
    );
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        tyst_core::private_fs::create_own_dir(dir).map_err(|e| e.to_string())?;
    }
    tyst_core::private_fs::write_replace(path, bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_target_hyprland_0_53_and_leave_the_name_field_usable() {
        let doc = rules_document(Path::new("/home/me/.config/tyst/hyprland.conf"));
        assert!(doc.contains("Hyprland 0.53"));
        assert!(doc.contains("source = /home/me/.config/tyst/hyprland.conf"));
        assert!(doc.contains("match:title = ^(Tyst Meeting)$"));
        assert!(doc.contains("match:title = ^(Tyst Dictation)$"));
        let effects: Vec<&str> = doc.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
        let effects = effects.join("\n");
        assert!(!effects.contains("match:class"));
        assert!(effects.contains("float = on"));
        assert!(effects.contains("pin = on"));
        assert!(effects.contains("no_initial_focus = on"));
        assert!(effects.contains("border_size = 0"));
        assert!(!effects.lines().any(|l| l.trim_start().starts_with("no_focus")));
        assert_eq!(re2_exact("a.b"), r"^(a\.b)$");
        assert!(BIND_HINT.contains("tyst --dictate"));
        assert!(BIND_HINT.contains("tyst --toggle-meeting"));
        let lua_path = Path::new("/home/me/.config/tyst/hyprland.lua");
        let lua = lua_rules_document(lua_path);
        assert!(lua.contains("hl.window_rule({"));
        assert!(lua.contains("title = \"^(Tyst Meeting)$\""));
        assert!(lua.contains("title = \"^(Tyst Dictation)$\""));
        assert!(lua.contains("float = true"));
        assert!(lua.contains("no_initial_focus = true"));
        assert!(lua.contains(&omarchy_dofile(lua_path)));
        let lua_code: Vec<&str> = lua.lines().filter(|l| !l.trim_start().starts_with("--")).collect();
        let lua_code = lua_code.join("\n");
        assert!(!lua_code.contains("no_focus"));
        assert!(!lua_code.contains("o.window"));
        let hostile = Path::new("/tmp/tyst\nos.execute('bad')");
        let line = omarchy_dofile(hostile);
        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains("\\n"));
        let hostile_lua = lua_rules_document(hostile);
        assert!(
            hostile_lua.lines().all(|l| {
                let t = l.trim_start();
                t.is_empty()
                    || t.starts_with("--")
                    || t.starts_with("hl.window_rule")
                    || t.starts_with("name ")
                    || t.starts_with("match ")
                    || t.starts_with("float ")
                    || t.starts_with("pin ")
                    || t.starts_with("no_initial_focus")
                    || t.starts_with("border_size")
                    || t.starts_with('}')
            }),
            "{hostile_lua}"
        );
    }

    #[test]
    fn helpers_place_the_pill_and_ignore_a_window_title() {
        use tyst_platform::hyprland::{self, Monitor};
        assert!(hyprland::detect_hyprland(false, Some("Hyprland"), None));
        assert!(!hyprland::detect_hyprland(true, Some("Hyprland"), Some("sig")));
        let monitor = Monitor {
            x: 1920,
            y: 0,
            width: 3840,
            height: 2160,
            scale: 2.0,
            focused: true,
            reserved: tyst_platform::hyprland::Reserved::default(),
        };
        assert_eq!(hyprland::pill_top_left(&monitor, (560, 168), 48), (2600, 864));
        let body = r#"{"address":"0xabc","class":"kitty","title":"user typed this","at":[1,2],"size":[3,4]}"#;
        let window = hyprland::parse_active_window(body).unwrap().unwrap();
        assert_eq!(window.class, "kitty");
        assert!(!format!("{window:?}").contains("user typed"));
        assert_eq!(tyst_platform::portal::wtype_args(false), &["-M", "ctrl", "-k", "v", "-m", "ctrl"]);
    }
}
