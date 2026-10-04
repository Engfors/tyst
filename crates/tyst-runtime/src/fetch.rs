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

use tyst_core::models::{self, FileStatus, Manifest, ModelFile, ModelSpec, PIANISSIMO, SILERO_VAD};

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
        for (file, (_, st)) in spec.files.iter().zip(models::verify(spec, models_dir, false)?) {
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
            models::verify(spec, models_dir, false).is_ok_and(|v| v.iter().all(|(_, s)| *s == FileStatus::Ok))
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
    let status = models::verify(spec, models_dir, false)?;
    for (file, (_, st)) in spec.files.iter().zip(status) {
        if st == FileStatus::Ok {
            progress(Progress::Present { file: file.name.clone() });
            continue;
        }
        download(spec, file, &dir, cancel, progress)?;
    }
    for file in &spec.derived {
        let path = dir.join(&file.name);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == file.size) {
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
    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |e| Error::Core(tyst_core::Error::io(&p, e))
    };
    let target = dir.join(&file.name);
    let part = dir.join(format!("{}.part", file.name));
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let url = file.url(spec);
    let mut req = agent().get(&url);
    if have > 0 && have < file.size {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| Error::Download(format!("{}: {e}", file.name)))?;
    let resumed = resp.status().as_u16() == 206;
    let mut out = if resumed {
        OpenOptions::new().append(true).open(&part).map_err(io(&part))?
    } else {
        File::create(&part).map_err(io(&part))?
    };
    let mut done = if resumed { have } else { 0 };
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut last_report = 0;
    progress(Progress::Downloading { file: file.name.clone(), done, total: file.size });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Download("cancelled".into()));
        }
        let n = reader.read(&mut buf).map_err(|e| Error::Download(format!("{}: {e}", file.name)))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(io(&part))?;
        done += n as u64;
        if done - last_report >= 4 << 20 {
            progress(Progress::Downloading { file: file.name.clone(), done, total: file.size });
            last_report = done;
        }
    }
    out.sync_all().map_err(io(&part))?;
    drop(out);
    let digest = models::sha256_file(&part)?;
    if digest != file.sha256 {
        std::fs::remove_file(&part).map_err(io(&part))?;
        return Err(Error::Download(format!("{}: checksum mismatch", file.name)));
    }
    std::fs::rename(&part, &target).map_err(io(&target))?;
    progress(Progress::Verified { file: file.name.clone() });
    Ok(())
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into()
}

/// An HTTPS client with the platform's certificates that gives up after `timeout`.
pub(crate) fn agent_with_timeout(timeout: std::time::Duration) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .timeout_global(Some(timeout))
        .build()
        .into()
}
