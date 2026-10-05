//! Files only the user can read: transcripts, journals and settings are created `0600` in folders
//! created `0700` (Unix), whatever the umask. Other users on the machine see nothing.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Creates `dir` and missing parents; folders created here are `0700`. An existing folder keeps
/// its mode (it may be one the user chose and shares on purpose).
pub fn create_dir_all(dir: &Path) -> Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir).map_err(|e| Error::io(dir, e))
}

/// Like [`create_dir_all`], and also makes an existing `dir` `0700`: for folders that only ever
/// hold Tyst's own files (the journal folder, the settings folder).
pub fn create_own_dir(dir: &Path) -> Result<()> {
    create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir).map_err(|e| Error::io(dir, e))?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| Error::io(dir, e))?;
        }
    }
    Ok(())
}

/// Opens a new file `0600`; fails if it exists.
pub fn create_new(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

/// Writes `bytes` to `path` through a `0600` temp file next to it and a rename, so a crash or a
/// full disk never leaves half a file, and the old file stays until the new one is complete.
pub fn write_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = part_path(path);
    let _ = std::fs::remove_file(&tmp);
    write_part(&tmp, bytes)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(path, e)
    })
}

/// Writes `bytes` to `path` only if `path` does not exist yet, completely or not at all.
/// Returns `Ok(false)` when `path` exists.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    let tmp = part_path(path);
    let _ = std::fs::remove_file(&tmp);
    write_part(&tmp, bytes)?;
    // A hard link never replaces an existing file; filesystems without links get a rename after
    // the check above.
    let placed = match std::fs::hard_link(&tmp, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) if !path.exists() => std::fs::rename(&tmp, path).map(|_| true),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&tmp);
    placed.map_err(|e| Error::io(path, e))
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

fn write_part(tmp: &Path, bytes: &[u8]) -> Result<()> {
    let result = create_new(tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    result.map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        Error::io(tmp, e)
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tyst-private-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn files_and_new_folders_are_private() {
        let tmp = tempdir("files");
        let dir = tmp.join("a/b");
        create_dir_all(&dir).unwrap();
        assert_eq!(mode(&tmp.join("a")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        let f = dir.join("x.md");
        assert!(write_new(&f, b"hello").unwrap());
        assert_eq!(mode(&f), 0o600);
        let g = dir.join("c.toml");
        write_replace(&g, b"a = 1").unwrap();
        assert_eq!(mode(&g), 0o600);
    }

    #[test]
    fn own_folders_are_tightened() {
        let tmp = tempdir("own");
        let dir = tmp.join("own");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_own_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        // A folder the user chose keeps its mode.
        let chosen = tmp.join("chosen");
        std::fs::create_dir(&chosen).unwrap();
        std::fs::set_permissions(&chosen, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_dir_all(&chosen).unwrap();
        assert_eq!(mode(&chosen), 0o755);
    }

    #[test]
    fn write_new_never_overwrites_and_leaves_no_part_file() {
        let tmp = tempdir("new");
        let f = tmp.join("x.md");
        assert!(write_new(&f, b"first").unwrap());
        assert!(!write_new(&f, b"second").unwrap());
        assert_eq!(std::fs::read(&f).unwrap(), b"first");
        let names: Vec<_> = std::fs::read_dir(&tmp).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("x.md")]);
    }
}
