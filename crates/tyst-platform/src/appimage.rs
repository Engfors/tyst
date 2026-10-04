//! Linux: starting other programs from inside an AppImage.
//!
//! The AppImage's launcher points Tyst at the libraries and modules inside the mounted image: its
//! `AppRun` prepends `$APPDIR` entries to `LD_LIBRARY_PATH`, `PATH`, `XDG_DATA_DIRS` and other
//! search paths, and linuxdeploy's GTK hook sets `GTK_PATH`, `GIO_MODULE_DIR` and the like. Tyst
//! itself needs that environment; programs it starts (`xdg-open` and the editor it opens,
//! `kwriteconfig6`, `ydotool`) must not get it, or they load Tyst's bundled libraries and break
//! when Tyst quits and the image is unmounted. [`host_command`] starts a program with the
//! environment the AppImage was started from: `$APPDIR` entries dropped from search paths (the
//! launcher appends the original value after its own), launcher-only variables removed.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Search paths the launcher prepends `$APPDIR` entries to (AppRun) or sets to one (GTK hook).
pub const SEARCH_PATHS: &[&str] = &[
    "GSETTINGS_SCHEMA_DIR",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "LD_LIBRARY_PATH",
    "PATH",
    "PERLLIB",
    "PYTHONPATH",
    "QT_PLUGIN_PATH",
    "XDG_DATA_DIRS",
];

/// Variables only the launcher sets; removed for other programs.
pub const LAUNCHER_ONLY: &[&str] = &[
    "APPDIR",
    "APPIMAGE",
    "ARGV0",
    "OWD",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_MODULE_DIR",
    "GI_TYPELIB_PATH",
    "GTK_DATA_PREFIX",
    "GTK_EXE_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GTK_PATH",
    "GTK_THEME",
    "PYTHONDONTWRITEBYTECODE",
    "PYTHONHOME",
];

/// `program`, started with the host's environment when Tyst runs from an AppImage.
pub fn host_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    let Some(appdir) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) else {
        return cmd;
    };
    let appdir = Path::new(&appdir);
    for var in LAUNCHER_ONLY {
        cmd.env_remove(var);
    }
    for var in SEARCH_PATHS {
        if let Some(value) = std::env::var_os(var) {
            match without_appdir(&value, appdir) {
                Some(value) => cmd.env(var, value),
                None => cmd.env_remove(var),
            };
        }
    }
    cmd
}

/// A `:`-separated path list without entries inside `appdir` (and without empty entries, which
/// the launcher leaves when the original value was unset); `None` if nothing is left.
pub fn without_appdir(list: &OsStr, appdir: &Path) -> Option<OsString> {
    let kept: Vec<PathBuf> =
        std::env::split_paths(list).filter(|d| !d.as_os_str().is_empty() && !d.starts_with(appdir)).collect();
    if kept.is_empty() { None } else { std::env::join_paths(kept).ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_entries_inside_the_image() {
        let appdir = Path::new("/tmp/.mount_TystAb");
        let list = OsStr::new("/tmp/.mount_TystAb/usr/lib/:/tmp/.mount_TystAb/lib/:/usr/local/lib:/usr/lib");
        assert_eq!(without_appdir(list, appdir).unwrap(), "/usr/local/lib:/usr/lib");
        // The launcher's own entries plus an empty original value: nothing left.
        assert_eq!(without_appdir(OsStr::new("/tmp/.mount_TystAb/usr/lib/:"), appdir), None);
        // A sibling directory with the same prefix is not inside the image.
        let sibling = OsStr::new("/tmp/.mount_TystAbc/share");
        assert_eq!(without_appdir(sibling, appdir).unwrap(), "/tmp/.mount_TystAbc/share");
    }
}
