//! KDE Plasma (Wayland and X11): KWin window rules for the meeting window and the dictation pill
//! (SPEC 10.2, option 1).
//!
//! Wayland clients cannot keep themselves above other windows, refuse focus or position
//! themselves, so KWin does it: keep above, skip taskbar/pager/switcher, extreme focus-stealing
//! prevention (showing the window never takes focus; clicking into the name field still works),
//! no border. KWin scripts ([`tyst_platform::kwin`]) do the rest: they place the pill and, while
//! dictating, activate it, and they read the meeting window's position when it hides and restore it
//! when it shows (a "remember position" rule never stored anything for a window that only hides). The rules are written
//! with `kwriteconfig6` into `kwinrulesrc` under fixed group names, so reinstalling replaces
//! them, and KWin reloads its config over D-Bus.

use crate::appimage::host_command;
use crate::windows::{MEETING_TITLE, PILL_TITLE};

const RULE: &str = "tyst-meeting-window";
const PILL_RULE: &str = "tyst-dictation-pill";

use tyst_platform::kwin::is_kde;

fn kwriteconfig(group: &str, key: &str, value: &str) -> Result<(), String> {
    let out = host_command("kwriteconfig6")
        .args(["--file", "kwinrulesrc", "--group", group, "--key", key, value])
        .output()
        .map_err(|e| format!("kwriteconfig6: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("kwriteconfig6 exited with {}", out.status)) }
}

fn kdeleteconfig(group: &str, key: &str) -> Result<(), String> {
    let out = host_command("kwriteconfig6")
        .args(["--file", "kwinrulesrc", "--group", group, "--key", key, "--delete"])
        .output()
        .map_err(|e| format!("kwriteconfig6: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("kwriteconfig6 exited with {}", out.status)) }
}

fn kreadconfig(group: &str, key: &str) -> String {
    host_command("kreadconfig6")
        .args(["--file", "kwinrulesrc", "--group", group, "--key", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Installs (or refreshes) the rules. Does nothing outside KDE.
pub fn install_rule() -> Result<(), String> {
    if !is_kde() {
        return Ok(());
    }
    install_meeting_rule()?;
    install_pill_rule()?;
    let reload = host_command("dbus-send")
        .args(["--session", "--type=method_call", "--dest=org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"])
        .status();
    if !reload.is_ok_and(|s| s.success()) {
        log::warn!("could not ask KWin to reload its rules; they apply after the next login");
    }
    log::info!("KWin rules for the meeting window and the pill installed");
    Ok(())
}

fn install_pill_rule() -> Result<(), String> {
    let settings: &[(&str, &str)] = &[
        ("Description", "Tyst dictation pill (installed by Tyst)"),
        ("title", PILL_TITLE),
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
    ];
    for (k, v) in settings {
        kwriteconfig(PILL_RULE, k, v)?;
    }
    add_to_rule_list(PILL_RULE)
}

fn add_to_rule_list(rule: &str) -> Result<(), String> {
    let rules = kreadconfig("General", "rules");
    let mut ids: Vec<&str> = rules.split(',').filter(|s| !s.is_empty()).collect();
    if !ids.contains(&rule) {
        ids.push(rule);
        kwriteconfig("General", "rules", &ids.join(","))?;
        kwriteconfig("General", "count", &ids.len().to_string())?;
    }
    Ok(())
}

fn install_meeting_rule() -> Result<(), String> {
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
    ];
    for (k, v) in settings {
        kwriteconfig(RULE, k, v)?;
    }
    // Earlier versions asked KWin to remember position and size, which it never stored for this
    // window (it hides rather than closes); Tyst now restores the position through a KWin script.
    for k in ["position", "positionrule", "size", "sizerule"] {
        kdeleteconfig(RULE, k)?;
    }
    add_to_rule_list(RULE)
}
