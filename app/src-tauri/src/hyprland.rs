//! Hyprland (Omarchy and other Hyprland 0.53+ sessions): window rules for the meeting window
//! and the dictation pill.
//!
//! Wayland clients cannot keep themselves above other windows, refuse focus on show, or position
//! themselves. Hyprland does it from a rules file Tyst writes and the user's config `source`s.
//! Tyst never edits `hyprland.conf`. The syntax is the one Hyprland 0.53 introduced
//! (`windowrulev2`, `nofocus` and `noborder` are gone; see the Hyprland window-rules wiki).
//! Omarchy moved to that syntax in its 0.53 config migration.
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

/// The rules file. `source_path` is the line the user adds to their own Hyprland config.
pub fn rules_document(source_path: &Path) -> String {
    let path = source_path.display();
    format!(
        "\
# Tyst window rules for Hyprland {RULES_SINCE} and newer
# (https://wiki.hypr.land/Configuring/Window-Rules/).
#
# Tyst never edits hyprland.conf. Add this line to the Hyprland config you edit
# (on Omarchy, the local config it already loads) and reload Hyprland:
#
#   source = {path}
#
# Restart Tyst afterwards so the windows map under these rules.
# float and pin keep them above other windows on every workspace.
# no_initial_focus keeps a newly shown window from taking the keyboard.
# no_focus is not set: it would block typing into \"Name this meeting\".
# border_size 0 replaced noborder in Hyprland {RULES_SINCE}.

{meeting}
{pill}\
",
        meeting = rule_block("tyst-meeting-window", MEETING_TITLE),
        pill = rule_block("tyst-dictation-pill", PILL_TITLE),
    )
}

fn rule_block(name: &str, title: &str) -> String {
    // Class and title both have to match. A title alone is not unique.
    format!(
        "\
windowrule {{
    name = {name}
    match:class = {class}
    match:title = {title_re}

    float = on
    pin = on
    no_initial_focus = on
    border_size = 0
}}
",
        class = re2_exact(crate::shortcuts::APP_ID),
        title_re = re2_exact(title),
    )
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
    let text = rules_document(&path);
    let changed = write_if_changed(&path, text.as_bytes())?;
    if changed {
        match tyst_platform::hyprland::Hyprland::connect(crate::shortcuts::APP_ID) {
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
        "Hyprland window rules: {} (add `source = {}` to hyprland.conf if it is not there)",
        path.display(),
        path.display()
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
        assert!(doc.contains("match:class = ^(com\\.engfors\\.tyst)$"));
        assert!(doc.contains("match:title = ^(Tyst Meeting)$"));
        assert!(doc.contains("match:title = ^(Tyst Dictation)$"));
        let effects: Vec<&str> = doc.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
        let effects = effects.join("\n");
        assert!(effects.contains("float = on"));
        assert!(effects.contains("pin = on"));
        assert!(effects.contains("no_initial_focus = on"));
        assert!(effects.contains("border_size = 0"));
        assert!(!effects.lines().any(|l| l.trim_start().starts_with("no_focus")));
        assert_eq!(re2_exact("a.b"), r"^(a\.b)$");
        assert!(BIND_HINT.contains("tyst --dictate"));
        assert!(BIND_HINT.contains("tyst --toggle-meeting"));
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
