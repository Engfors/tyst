//! Pinned model manifest and local model storage (SPEC 5.2). Verification only: downloading is
//! the CLI's (and later the app's) job, so this crate stays offline.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// The manifest shipped with this build (`models/models.toml`).
pub const BUILTIN_MANIFEST: &str = include_str!("../../../models/models.toml");

pub const PIANISSIMO: &str = "pianissimo-sv-int8";
pub const PARAKEET: &str = "parakeet-v3-int8";
pub const SILERO_VAD: &str = "silero-vad";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub models: BTreeMap<String, ModelSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSpec {
    pub kind: String,
    #[serde(default)]
    pub lang: String,
    pub dir: String,
    pub hf_repo: String,
    pub revision: String,
    #[serde(default)]
    pub license: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    /// Local file name.
    pub name: String,
    /// Path in the Hugging Face repo, when different from `name`.
    #[serde(default)]
    pub path: Option<String>,
    pub size: u64,
    pub sha256: String,
}

impl ModelFile {
    pub fn remote_path(&self) -> &str {
        self.path.as_deref().unwrap_or(&self.name)
    }

    /// Download URL at the pinned revision.
    pub fn url(&self, spec: &ModelSpec) -> String {
        format!("https://huggingface.co/{}/resolve/{}/{}", spec.hf_repo, spec.revision, self.remote_path())
    }
}

impl ModelSpec {
    /// Id with short revision, used in transcripts: `pianissimo-sv-int8@63730c6`.
    pub fn tagged_id(&self, id: &str) -> String {
        format!("{id}@{}", &self.revision[..self.revision.len().min(7)])
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

impl Manifest {
    pub fn builtin() -> Self {
        Self::parse(BUILTIN_MANIFEST).expect("built-in model manifest is valid")
    }

    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::Config(format!("model manifest: {e}")))
    }

    pub fn get(&self, id: &str) -> Result<&ModelSpec> {
        self.models.get(id).ok_or_else(|| Error::Model(format!("unknown model '{id}'")))
    }
}

/// Default models directory: `$TYST_MODELS`, else the platform data dir from SPEC 5.2.
pub fn default_models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TYST_MODELS") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Tyst/models")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("tyst/models")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileStatus {
    Ok,
    Missing,
    WrongSize { actual: u64 },
    WrongChecksum,
}

/// Checks every file of a model. `full` also hashes the files (slow for the encoders).
pub fn verify(spec: &ModelSpec, models_dir: &Path, full: bool) -> Result<Vec<(String, FileStatus)>> {
    let dir = models_dir.join(&spec.dir);
    let mut out = Vec::new();
    for f in &spec.files {
        let path = dir.join(&f.name);
        let status = match std::fs::metadata(&path) {
            Err(_) => FileStatus::Missing,
            Ok(m) if m.len() != f.size => FileStatus::WrongSize { actual: m.len() },
            Ok(_) if full && sha256_file(&path)? != f.sha256 => FileStatus::WrongChecksum,
            Ok(_) => FileStatus::Ok,
        };
        out.push((f.name.clone(), status));
    }
    Ok(out)
}

/// Directory of a model whose files are present with the right sizes.
pub fn installed_dir(spec: &ModelSpec, models_dir: &Path) -> Result<PathBuf> {
    let bad: Vec<String> = verify(spec, models_dir, false)?
        .into_iter()
        .filter(|(_, s)| *s != FileStatus::Ok)
        .map(|(n, s)| format!("{n} ({s:?})"))
        .collect();
    if bad.is_empty() {
        Ok(models_dir.join(&spec.dir))
    } else {
        Err(Error::Model(format!(
            "{} not installed in {}: {}. Run `tyst-cli models fetch`.",
            spec.dir,
            models_dir.display(),
            bad.join(", ")
        )))
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_manifest_parses() {
        let m = Manifest::builtin();
        let p = m.get(PIANISSIMO).unwrap();
        assert_eq!(p.tagged_id(PIANISSIMO), "pianissimo-sv-int8@63730c6");
        assert!(p.files.iter().any(|f| f.name == "vocab.txt"));
        let vad = m.get(SILERO_VAD).unwrap();
        assert_eq!(vad.files[0].remote_path(), "silero_vad.onnx");
        assert!(vad.files[0].url(vad).ends_with("/resolve/fba88cd2e921609e7675c3aaf51e0b9b295da4bc/silero_vad.onnx"));
    }

    #[test]
    fn verify_reports_missing_and_wrong_files() {
        let dir = std::env::temp_dir().join(format!("tyst-models-{}", std::process::id()));
        let spec = ModelSpec {
            kind: "vad".into(),
            lang: String::new(),
            dir: "m".into(),
            hf_repo: "x/y".into(),
            revision: "abc".into(),
            license: String::new(),
            files: vec![
                ModelFile {
                    name: "a".into(),
                    path: None,
                    size: 3,
                    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                },
                ModelFile { name: "b".into(), path: None, size: 1, sha256: "00".into() },
                ModelFile { name: "c".into(), path: None, size: 1, sha256: "00".into() },
            ],
        };
        std::fs::create_dir_all(dir.join("m")).unwrap();
        std::fs::write(dir.join("m/a"), "abc").unwrap();
        std::fs::write(dir.join("m/b"), "zz").unwrap();
        let st = verify(&spec, &dir, true).unwrap();
        assert_eq!(st[0].1, FileStatus::Ok);
        assert_eq!(st[1].1, FileStatus::WrongSize { actual: 2 });
        assert_eq!(st[2].1, FileStatus::Missing);
        assert!(installed_dir(&spec, &dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
