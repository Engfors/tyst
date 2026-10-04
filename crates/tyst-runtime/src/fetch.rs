//! Model download (SPEC 5.2): from Hugging Face at the pinned revision, resuming partial
//! downloads, and checking every file against its SHA-256 before moving it into place. Files the
//! manifest derives locally (the banded encoder) are computed afterwards, without network.
//!
//! This is one of the two network uses the SPEC allows. It only runs when the user asks for it
//! (`tyst-cli models fetch`, or the app's onboarding / Models settings).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tyst_core::models::{self, Check, FileStatus, Manifest, ModelFile, ModelSpec, PIANISSIMO, SILERO_VAD};

use crate::{Error, Result};

/// What a plain fetch installs: everything a Swedish/auto session needs.
pub const DEFAULT_MODELS: [&str; 2] = [PIANISSIMO, SILERO_VAD];

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Starting a model.
    Model {
        id: String,
        bytes: u64,
    },
    /// Already installed and the right size.
    Present {
        file: String,
    },
    Downloading {
        file: String,
        done: u64,
        total: u64,
    },
    /// Computing a derived file locally.
    Deriving {
        file: String,
    },
    /// Downloaded (or derived) and the checksum matched.
    Verified {
        file: String,
    },
}

/// Bytes still to download for these models (0 when all are installed).
pub fn missing_bytes(ids: &[&str], models_dir: &Path) -> Result<u64> {
    let manifest = Manifest::builtin();
    let mut total = 0;
    for id in ids {
        let spec = manifest.get(id)?;
        for (file, (_, st)) in spec.files.iter().zip(models::verify(spec, models_dir, Check::Size)?) {
            if st != FileStatus::Ok {
                total += file.size;
            }
        }
    }
    Ok(total)
}

/// True when the models are installed (sizes checked, derived files present).
pub fn installed(ids: &[&str], models_dir: &Path) -> bool {
    let manifest = Manifest::builtin();
    ids.iter().all(|id| {
        manifest.get(id).is_ok_and(|spec| {
            let dir = models_dir.join(&spec.dir);
            models::verify(spec, models_dir, Check::Size).is_ok_and(|v| v.iter().all(|(_, s)| *s == FileStatus::Ok))
                && spec.derived.iter().all(|d| std::fs::metadata(dir.join(&d.name)).is_ok_and(|m| m.len() == d.size))
        })
    })
}

/// Downloads and derives the given models. `cancel` stops between 1 MB reads, keeping the
/// `.part` file so the next call resumes.
pub fn fetch(ids: &[&str], models_dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let manifest = Manifest::builtin();
    for id in ids {
        let spec = manifest.get(id)?;
        fetch_one(id, spec, models_dir, cancel, progress)?;
    }
    Ok(())
}

fn fetch_one(
    id: &str,
    spec: &ModelSpec,
    models_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let dir = models_dir.join(&spec.dir);
    std::fs::create_dir_all(&dir).map_err(|e| tyst_core::Error::io(&dir, e))?;
    log::info!("fetching {id} from {}@{} ({} bytes)", spec.hf_repo, &spec.revision[..7], spec.total_size());
    progress(Progress::Model { id: id.to_string(), bytes: spec.total_size() });
    // Stamped check: a file that is the right size but was never hashed in this install (or
    // changed since) is hashed now, and downloaded again if it does not match.
    let status = models::verify(spec, models_dir, Check::Stamp)?;
    for (file, (_, st)) in spec.files.iter().zip(status) {
        if st == FileStatus::Ok {
            progress(Progress::Present { file: file.name.clone() });
            continue;
        }
        download(spec, file, &dir, cancel, progress)?;
    }
    for file in &spec.derived {
        let path = dir.join(&file.name);
        if models::file_status(&path, file.size, &file.sha256, Check::Stamp)? == FileStatus::Ok {
            progress(Progress::Present { file: file.name.clone() });
            continue;
        }
        progress(Progress::Deriving { file: file.name.clone() });
        models::derive(spec, models_dir, file)?;
        progress(Progress::Verified { file: file.name.clone() });
    }
    Ok(())
}

/// Downloads to `<name>.part` (resuming a partial download), verifies, then renames.
fn download(
    spec: &ModelSpec,
    file: &ModelFile,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let target = Target { name: &file.name, size: file.size, sha256: &file.sha256 };
    download_url(&agent(), &file.url(spec), &target, dir, cancel, progress)
}

/// One file to download: where it goes and what it must hash to.
struct Target<'a> {
    name: &'a str,
    size: u64,
    sha256: &'a str,
}

/// The download itself, kept apart from the manifest so it can be tested against a local
/// server. A partial download is only ever extended at its exact end, never past the expected
/// size, and is kept whenever the server ends early or answers oddly, so the next try resumes.
fn download_url(
    agent: &ureq::Agent,
    url: &str,
    file: &Target,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |e| Error::Core(tyst_core::Error::io(&p, e))
    };
    let fail = |msg: String| Error::Download(format!("{}: {msg}", file.name));
    let target = dir.join(file.name);
    let part = dir.join(format!("{}.part", file.name));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > file.size {
        std::fs::remove_file(&part).map_err(io(&part))?;
        have = 0;
    }
    if have == file.size {
        // A partial that is complete (stopped between the last read and the rename).
        if models::sha256_file(&part)? == file.sha256 {
            return finish(&part, &target, file, progress);
        }
        std::fs::remove_file(&part).map_err(io(&part))?;
        have = 0;
    }
    let mut req = agent.get(url);
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| fail(e.to_string()))?;
    let resumed = resp.status().as_u16() == 206;
    if resumed {
        let start = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(range_start);
        if start != Some(have) {
            // Keep the partial: appending here would corrupt it.
            return Err(fail(format!("the server resumed at {start:?}, not at byte {have}; try again later")));
        }
    }
    let base = if resumed { have } else { 0 };
    let mut out = if resumed {
        OpenOptions::new().append(true).open(&part).map_err(io(&part))?
    } else {
        File::create(&part).map_err(io(&part))?
    };
    let mut done = base;
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut last_report = 0;
    let mut overflow = false;
    progress(Progress::Downloading { file: file.name.to_string(), done, total: file.size });
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(Error::Download("cancelled".into()));
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => break Err(fail(e.to_string())),
        };
        if n == 0 {
            break Ok(());
        }
        if done + n as u64 > file.size {
            overflow = true;
            break Err(fail(format!("the server sent more than the expected {} bytes", file.size)));
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            break Err(io(&part)(e));
        }
        done += n as u64;
        if done - last_report >= 4 << 20 {
            progress(Progress::Downloading { file: file.name.to_string(), done, total: file.size });
            last_report = done;
        }
    };
    out.sync_all().map_err(io(&part))?;
    drop(out);
    if let Err(e) = result {
        if overflow {
            // Drop what this response wrote; keep what an earlier one did.
            OpenOptions::new().write(true).open(&part).and_then(|f| f.set_len(base)).map_err(io(&part))?;
        }
        return Err(e);
    }
    if done < file.size {
        return Err(fail(format!("the download ended at {done} of {} bytes; run it again to resume", file.size)));
    }
    if models::sha256_file(&part)? != file.sha256 {
        // Right length at the right offsets but the wrong bytes: nothing in it can be trusted.
        std::fs::remove_file(&part).map_err(io(&part))?;
        return Err(fail("checksum mismatch".into()));
    }
    finish(&part, &target, file, progress)
}

fn finish(part: &Path, target: &Path, file: &Target, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    std::fs::rename(part, target).map_err(|e| Error::Core(tyst_core::Error::io(target, e)))?;
    models::write_stamp(target, file.sha256)?;
    progress(Progress::Verified { file: file.name.to_string() });
    Ok(())
}

/// First byte of a `Content-Range: bytes <start>-<end>/<total>` header.
fn range_start(v: &str) -> Option<u64> {
    v.trim().strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    use super::*;

    const BODY: &[u8] = b"0123456789abcdefghij";
    const SHA: &str = "6bc14bdc4517a7a682c6910de2e2946eb8e1ecd04090728fef6d092a7ceb62c5";

    /// Answers a request with the given range start: (status, content-range, body).
    type Reply = Box<dyn Fn(Option<u64>) -> (u16, Option<String>, Vec<u8>) + Send>;

    /// Serves one request per entry.
    fn serve(replies: Vec<Reply>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut range = None;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse().ok();
                    }
                }
                let (status, content_range, body) = reply(range);
                let mut head =
                    format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                if let Some(cr) = content_range {
                    head.push_str(&format!("Content-Range: {cr}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        url
    }

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder().proxy(None).http_status_as_error(true).build().into()
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tyst-fetch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn get(url: &str, d: &Path) -> Result<()> {
        let target = Target { name: "f", size: BODY.len() as u64, sha256: SHA };
        download_url(&agent(), url, &target, d, &AtomicBool::new(false), &mut |_| {})
    }

    fn part(d: &Path) -> Option<Vec<u8>> {
        std::fs::read(d.join("f.part")).ok()
    }

    #[test]
    fn full_download_is_verified_and_stamped() {
        let d = dir("full");
        let url = serve(vec![Box::new(|_| (200, None, BODY.to_vec()))]);
        get(&url, &d).unwrap();
        assert_eq!(std::fs::read(d.join("f")).unwrap(), BODY);
        assert!(d.join("f.verified").exists());
        assert!(part(&d).is_none());
    }

    #[test]
    fn good_resume_appends_at_the_end() {
        let d = dir("resume");
        std::fs::write(d.join("f.part"), &BODY[..8]).unwrap();
        let url = serve(vec![Box::new(|start| {
            assert_eq!(start, Some(8));
            (206, Some("bytes 8-19/20".into()), BODY[8..].to_vec())
        })]);
        get(&url, &d).unwrap();
        assert_eq!(std::fs::read(d.join("f")).unwrap(), BODY);
    }

    #[test]
    fn resume_at_the_wrong_offset_keeps_the_partial() {
        let d = dir("wrongstart");
        std::fs::write(d.join("f.part"), &BODY[..8]).unwrap();
        let url = serve(vec![Box::new(|_| (206, Some("bytes 0-19/20".into()), BODY.to_vec()))]);
        assert!(get(&url, &d).is_err());
        assert_eq!(part(&d).unwrap(), &BODY[..8]);
        assert!(!d.join("f").exists());
    }

    #[test]
    fn short_response_keeps_what_arrived_for_the_next_try() {
        let d = dir("short");
        std::fs::write(d.join("f.part"), &BODY[..8]).unwrap();
        let url = serve(vec![
            Box::new(|_| (206, Some("bytes 8-11/20".into()), BODY[8..12].to_vec())),
            Box::new(|start| {
                assert_eq!(start, Some(12));
                (206, Some("bytes 12-19/20".into()), BODY[12..].to_vec())
            }),
        ]);
        let e = get(&url, &d).unwrap_err().to_string();
        assert!(e.contains("ended at 12"), "{e}");
        assert_eq!(part(&d).unwrap(), &BODY[..12]);
        get(&url, &d).unwrap();
        assert_eq!(std::fs::read(d.join("f")).unwrap(), BODY);
    }

    #[test]
    fn oversized_response_stops_at_the_expected_size() {
        let d = dir("oversized");
        std::fs::write(d.join("f.part"), &BODY[..8]).unwrap();
        // A server that ignores the range and sends far more than the file.
        let url = serve(vec![Box::new(|_| (200, None, vec![b'x'; 5 << 20]))]);
        assert!(get(&url, &d).is_err());
        // A 200 replaces the partial; it stops at the size limit and drops what it wrote.
        assert_eq!(part(&d).map(|p| p.len()), Some(0));
        assert!(!d.join("f").exists());
    }

    #[test]
    fn wrong_bytes_are_never_installed() {
        let d = dir("mismatch");
        let url = serve(vec![Box::new(|_| (200, None, vec![b'x'; BODY.len()]))]);
        assert!(get(&url, &d).unwrap_err().to_string().contains("checksum"));
        assert!(!d.join("f").exists());
        assert!(part(&d).is_none());
    }

    #[test]
    fn content_range_start() {
        assert_eq!(range_start("bytes 8-19/20"), Some(8));
        assert_eq!(range_start("bytes */20"), None);
    }
}
