//! Running from an AppImage (SPEC 10.2, Phase 4): which file to launch Tyst from, and opening
//! files without passing on the AppImage's environment ([`tyst_platform::appimage`]).

use std::path::{Path, PathBuf};

pub use tyst_platform::appimage::host_command;

/// The AppImage file Tyst runs from. `$APPIMAGE` is trusted only when this process runs from
/// inside `$APPDIR` (or from an unpacked image, `APPIMAGE_EXTRACT_AND_RUN`) and the file exists,
/// so a value inherited from another AppImage never becomes Tyst's login entry.
pub fn path() -> Option<PathBuf> {
    let image = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from)?;
    if !image.is_file() {
        return None;
    }
    let extracted = std::env::var_os("APPIMAGE_EXTRACT_AND_RUN").is_some_and(|v| v == "1");
    let inside = std::env::var_os("APPDIR").filter(|d| !d.is_empty()).is_some_and(|dir| {
        let exe = std::env::current_exe().and_then(|p| p.canonicalize());
        let dir = Path::new(&dir).canonicalize();
        matches!((exe, dir), (Ok(exe), Ok(dir)) if exe.starts_with(&dir))
    });
    (inside || extracted).then_some(image)
}

/// The file to start Tyst from: the AppImage when running from one (the binary inside lives in a
/// mount that disappears), else the binary.
pub fn launch_path() -> Option<PathBuf> {
    path().or_else(|| std::env::current_exe().ok())
}

/// `path` as one argument in a desktop entry's `Exec=` (Desktop Entry Specification, "The Exec
/// key"): double-quoted with `"`, `` ` ``, `$` and `\` escaped, `%` doubled so it is not a field
/// code, then backslashes doubled for the string-value escaping that is undone first.
pub fn exec_arg(path: &Path) -> Result<String, String> {
    let s = path.to_str().ok_or_else(|| format!("{} is not valid UTF-8", path.display()))?;
    if s.chars().any(char::is_control) {
        return Err(format!("{} contains a control character", path.display()));
    }
    let mut quoted = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(quoted.replace('\\', "\\\\"))
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
    fn exec_arg_quotes_and_escapes() {
        let arg = |s: &str| exec_arg(Path::new(s)).unwrap();
        assert_eq!(arg("/home/emil/Tyst.AppImage"), r#""/home/emil/Tyst.AppImage""#);
        assert_eq!(arg("/home/emil/My Apps/Tyst.AppImage"), r#""/home/emil/My Apps/Tyst.AppImage""#);
        assert_eq!(arg("/a/%s/b"), r#""/a/%%s/b""#);
        assert_eq!(arg("/a/$HOME`x`"), r#""/a/\\$HOME\\`x\\`""#);
        assert_eq!(arg(r#"/a/"b"\c"#), r#""/a/\\"b\\"\\\\c""#);
        assert!(exec_arg(Path::new("/a/b\nc")).is_err());
    }
}
