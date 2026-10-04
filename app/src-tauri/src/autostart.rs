//! Linux: launch at login (SPEC 9.6) through `~/.config/autostart/Tyst.desktop`.
//!
//! Written here rather than by the autostart plugin: `Exec` points at the AppImage (or binary)
//! Tyst runs from, quoted so a path with spaces works, and is refreshed on every start so it
//! follows a moved AppImage. Only the keys Tyst owns are rewritten; anything the user added (say
//! `X-GNOME-Autostart-Delay`) stays, and the file is only written when something changed.

use std::path::{Path, PathBuf};

use crate::appimage;
use crate::shortcuts::APP_ID;

const GROUP: &str = "[Desktop Entry]";

fn file() -> Result<PathBuf, String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config")))
        .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(config.join("autostart").join("Tyst.desktop"))
}

/// Turns launch at login on (creating or refreshing the entry) or off (removing it).
pub fn set(on: bool) -> Result<(), String> {
    let path = file()?;
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        };
    }
    let exe = appimage::launch_path().ok_or("no executable path")?;
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let new = merge(&old, &owned_keys(&exe)?);
    if new == old {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))
}

fn owned_keys(exe: &Path) -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![
        ("Type", "Application".into()),
        ("Name", "Tyst".into()),
        ("Comment", "Local meeting transcription and dictation".into()),
        ("Exec", appimage::exec_arg(exe)?),
        ("Icon", APP_ID.into()),
        ("Terminal", "false".into()),
        ("StartupNotify", "false".into()),
    ])
}

/// `old` with the owned keys of its `[Desktop Entry]` group set to `keys` (added at the end of the
/// group when missing), every other line kept.
fn merge(old: &str, keys: &[(&str, String)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut written = vec![false; keys.len()];
    let mut in_group = false;
    let mut saw_group = false;
    let flush = |out: &mut Vec<String>, written: &mut Vec<bool>| {
        // Insert missing keys after the group's last non-blank line.
        let at = out.iter().rposition(|l| !l.trim().is_empty()).map_or(out.len(), |i| i + 1);
        let missing: Vec<String> =
            keys.iter().zip(written.iter()).filter(|(_, w)| !**w).map(|((k, v), _)| format!("{k}={v}")).collect();
        out.splice(at..at, missing);
        written.iter_mut().for_each(|w| *w = true);
    };
    for line in old.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_group {
                flush(&mut out, &mut written);
            }
            in_group = trimmed == GROUP;
            saw_group |= in_group;
            out.push(line.to_string());
            continue;
        }
        if in_group
            && let Some((key, _)) = trimmed.split_once('=')
            && let Some(i) = keys.iter().position(|(k, _)| *k == key.trim())
        {
            // A repeated owned key is dropped.
            if !written[i] {
                out.push(format!("{}={}", keys[i].0, keys[i].1));
                written[i] = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if in_group {
        flush(&mut out, &mut written);
    }
    if !saw_group {
        let mut group = vec![GROUP.to_string()];
        group.extend(keys.iter().map(|(k, v)| format!("{k}={v}")));
        if !out.is_empty() {
            group.push(String::new());
        }
        out.splice(0..0, group);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<(&'static str, String)> {
        owned_keys(Path::new("/home/emil/My Apps/Tyst.AppImage")).unwrap()
    }

    #[test]
    fn new_entry() {
        let s = merge("", &keys());
        assert!(s.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(s.contains("Exec=\"/home/emil/My Apps/Tyst.AppImage\"\n"));
        assert!(s.contains("Icon=com.engfors.tyst\n"));
    }

    #[test]
    fn keeps_user_keys_and_replaces_owned_ones() {
        // What tauri-plugin-autostart wrote before, plus a key the user added.
        let old = "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Tyst\nComment=Tyststartup script\n\
                   Exec=/old/path/tyst \nStartupNotify=false\nTerminal=false\nX-GNOME-Autostart-Delay=5\n";
        let s = merge(old, &keys());
        assert!(s.contains("Version=1.0\n"));
        assert!(s.contains("X-GNOME-Autostart-Delay=5\n"));
        assert!(s.contains("Exec=\"/home/emil/My Apps/Tyst.AppImage\"\n"));
        assert!(s.contains("Comment=Local meeting transcription and dictation\n"));
        assert!(!s.contains("/old/path"));
        assert_eq!(s.matches("Exec=").count(), 1);
        assert!(s.contains("Icon=com.engfors.tyst\n"), "missing owned key added");
        // Running it again changes nothing, so the file is not rewritten.
        assert_eq!(merge(&s, &keys()), s);
    }

    #[test]
    fn other_groups_are_kept() {
        let old = "[Desktop Entry]\nExec=x\n\n[Desktop Action quit]\nName=Quit\nExec=y\n";
        let s = merge(old, &keys());
        assert!(s.contains("[Desktop Action quit]\nName=Quit\nExec=y\n"));
        let entry = s.split("[Desktop Action quit]").next().unwrap();
        assert!(entry.contains("Icon=com.engfors.tyst\n"));
        assert!(entry.ends_with("\n\n"), "blank line before the next group stays");
    }
}
