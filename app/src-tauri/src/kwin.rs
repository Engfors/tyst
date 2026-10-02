//! KDE Plasma (Wayland and X11): a KWin window rule for the meeting window (SPEC 10.2, option 1).
//!
//! Wayland clients cannot keep themselves above other windows, refuse focus or position
//! themselves, so KWin does it: keep above, skip taskbar/pager/switcher, extreme focus-stealing
//! prevention (showing the window never takes focus; clicking into the name field still works),
//! no border, and remembered position and size. The rule is written with `kwriteconfig6` into
//! `kwinrulesrc` under a fixed group name, so reinstalling replaces it, and KWin reloads its
//! config over D-Bus.

use std::process::Command;

use crate::windows::MEETING_TITLE;

const RULE: &str = "tyst-meeting-window";

pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
}

fn kwriteconfig(group: &str, key: &str, value: &str) -> Result<(), String> {
    let out = Command::new("kwriteconfig6")
        .args(["--file", "kwinrulesrc", "--group", group, "--key", key, value])
        .output()
        .map_err(|e| format!("kwriteconfig6: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("kwriteconfig6 exited with {}", out.status)) }
}

fn kreadconfig(group: &str, key: &str) -> String {
    Command::new("kreadconfig6")
        .args(["--file", "kwinrulesrc", "--group", group, "--key", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Installs (or refreshes) the rule. Does nothing outside KDE.
pub fn install_rule() -> Result<(), String> {
    if !is_kde() {
        return Ok(());
    }
    // Rule values: 2 = force, 3 = apply initially, 4 = remember. Match: 1 = exact, 2 = substring.
    let settings: &[(&str, &str)] = &[
        ("Description", "Tyst meeting window (installed by Tyst)"),
        ("title", MEETING_TITLE),
        ("titlematch", "1"),
        ("wmclass", "tyst"),
        ("wmclassmatch", "2"),
        ("above", "true"),
        ("aboverule", "2"),
        ("skiptaskbar", "true"),
        ("skiptaskbarrule", "2"),
        ("skippager", "true"),
        ("skippagerrule", "2"),
        ("skipswitcher", "true"),
        ("skipswitcherrule", "2"),
        ("fsplevel", "4"),
        ("fsplevelrule", "2"),
        ("noborder", "true"),
        ("noborderrule", "2"),
        ("position", "0,0"),
        ("positionrule", "4"),
        ("size", "420,260"),
        ("sizerule", "4"),
    ];
    // Keep a position/size KWin already remembered.
    let remembered = kreadconfig(RULE, "positionrule") == "4";
    for (k, v) in settings {
        if remembered && matches!(*k, "position" | "size") {
            continue;
        }
        kwriteconfig(RULE, k, v)?;
    }
    let rules = kreadconfig("General", "rules");
    let mut ids: Vec<&str> = rules.split(',').filter(|s| !s.is_empty()).collect();
    if !ids.contains(&RULE) {
        ids.push(RULE);
        kwriteconfig("General", "rules", &ids.join(","))?;
        kwriteconfig("General", "count", &ids.len().to_string())?;
    }
    let reload = Command::new("dbus-send")
        .args(["--session", "--type=method_call", "--dest=org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"])
        .status();
    if !reload.is_ok_and(|s| s.success()) {
        log::warn!("could not ask KWin to reload its rules; they apply after the next login");
    }
    log::info!("KWin rule for the meeting window installed");
    Ok(())
}
