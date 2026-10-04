//! Running from an AppImage (SPEC 10.2, Phase 4).
//!
//! The AppImage's launcher points GTK, GIO and GSettings at the libraries and modules inside the
//! mounted image. Programs Tyst starts inherit that environment, so a GTK editor opened for a
//! transcript would load Tyst's bundled modules (and break once Tyst quits and the image is
//! unmounted). [`host_command`] starts a program with the host's own environment instead.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Variables the AppImage launcher (linuxdeploy's GTK hook and the runtime) sets for Tyst itself.
const LAUNCHER_VARS: &[&str] = &[
    "APPDIR",
    "APPIMAGE",
    "ARGV0",
    "OWD",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_MODULE_DIR",
    "GI_TYPELIB_PATH",
    "GSETTINGS_SCHEMA_DIR",
    "GTK_DATA_PREFIX",
    "GTK_EXE_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GTK_PATH",
    "GTK_THEME",
];

/// The AppImage file Tyst runs from, if any.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The command to start Tyst again: the AppImage file when running from one (the binary inside
/// it lives in a mount that disappears), else the binary.
pub fn launch_path() -> Option<PathBuf> {
    path().or_else(|| std::env::current_exe().ok())
}

/// `program` with the AppImage launcher's variables removed when Tyst runs from an AppImage.
pub fn host_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    let Some(appdir) = std::env::var_os("APPDIR") else {
        return cmd;
    };
    for var in LAUNCHER_VARS {
        cmd.env_remove(var);
    }
    if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
        match host_data_dirs(&dirs, Path::new(&appdir)) {
            Some(dirs) => cmd.env("XDG_DATA_DIRS", dirs),
            None => cmd.env_remove("XDG_DATA_DIRS"),
        };
    }
    cmd
}

/// `XDG_DATA_DIRS` without the entries inside the AppImage; `None` if nothing is left.
fn host_data_dirs(dirs: &OsStr, appdir: &Path) -> Option<OsString> {
    let kept: Vec<PathBuf> = std::env::split_paths(dirs).filter(|d| !d.starts_with(appdir)).collect();
    if kept.is_empty() { None } else { std::env::join_paths(kept).ok() }
}

/// Opens a file or folder with the desktop's default application (`xdg-open`).
pub fn open(path: &Path) -> Result<(), String> {
    std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut child = host_command("xdg-open").arg(path).spawn().map_err(|e| format!("xdg-open: {e}"))?;
    // xdg-open returns once the application is started; reap it so it does not linger.
    std::thread::spawn(move || {
        if let Ok(status) = child.wait()
            && !status.success()
        {
            log::warn!("xdg-open exited with {status}");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dirs_drop_appimage_entries() {
        let appdir = Path::new("/tmp/.mount_TystAb");
        let dirs = OsStr::new("/tmp/.mount_TystAb/usr/share:/usr/local/share:/usr/share");
        assert_eq!(host_data_dirs(dirs, appdir).unwrap(), "/usr/local/share:/usr/share");
        assert_eq!(host_data_dirs(OsStr::new("/tmp/.mount_TystAb/usr/share"), appdir), None);
        // A sibling directory with the same prefix is not inside the image.
        let sibling = OsStr::new("/tmp/.mount_TystAbc/share");
        assert_eq!(host_data_dirs(sibling, appdir).unwrap(), "/tmp/.mount_TystAbc/share");
    }
}
