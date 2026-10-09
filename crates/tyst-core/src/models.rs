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
/// SentencePiece model in Pianissimo's directory, for phrase boosting (optional).
pub const TOKENIZER: &str = "tokenizer.model";

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
    /// Files computed locally from downloaded ones (no download), pinned the same way.
    #[serde(default)]
    pub derived: Vec<DerivedFile>,
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
    /// Fetched with the model, but the model counts as installed without it (a file added after
    /// a release, such as the tokenizer for phrase boosting). [`verify`] skips it.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DerivedFile {
    pub name: String,
    /// The downloaded file it is computed from.
    pub from: String,
    pub transform: Transform,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transform {
    /// [`crate::encoder_rewrite`]: block-local attention rewritten as banded full attention.
    BandedAttention,
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

    /// The files a model needs to count as installed.
    pub fn required_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.files.iter().filter(|f| !f.optional)
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

/// How hard [`verify`] looks at a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Size only: cheap, for "is it there" in the UI. Says nothing about the contents.
    Size,
    /// Size, and a SHA-256 that matched the pin for exactly this file (same size and mtime).
    /// A file without such a stamp is hashed once and stamped. What loading and fetching use.
    Stamp,
    /// Hash every file again (slow for the encoders) and refresh the stamps.
    Full,
}

/// Checks every required and derived file of a model (optional files: [`optional_file`]).
pub fn verify(spec: &ModelSpec, models_dir: &Path, check: Check) -> Result<Vec<(String, FileStatus)>> {
    let dir = models_dir.join(&spec.dir);
    let pinned = spec.required_files().map(|f| (&f.name, f.size, &f.sha256));
    let derived = spec.derived.iter().map(|d| (&d.name, d.size, &d.sha256));
    let mut out = Vec::new();
    for (name, size, sha256) in pinned.chain(derived) {
        out.push((name.clone(), file_status(&dir.join(name), size, sha256, check)?));
    }
    Ok(out)
}

/// Path of the optional file `name` when it is present and matches its pin ([`Check::Stamp`]).
pub fn optional_file(spec: &ModelSpec, models_dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    let Some(file) = spec.files.iter().find(|f| f.name == name) else { return Ok(None) };
    let path = models_dir.join(&spec.dir).join(name);
    Ok((file_status(&path, file.size, &file.sha256, Check::Stamp)? == FileStatus::Ok).then_some(path))
}

/// Status of one model file (see [`Check`]).
pub fn file_status(path: &Path, size: u64, sha256: &str, check: Check) -> Result<FileStatus> {
    let meta = match std::fs::metadata(path) {
        Err(_) => return Ok(FileStatus::Missing),
        Ok(m) if m.len() != size => return Ok(FileStatus::WrongSize { actual: m.len() }),
        Ok(m) => m,
    };
    if check == Check::Size || (check == Check::Stamp && stamp_matches(path, &meta, sha256)) {
        return Ok(FileStatus::Ok);
    }
    if sha256_file(path)? != sha256 {
        let _ = std::fs::remove_file(stamp_path(path));
        return Ok(FileStatus::WrongChecksum);
    }
    write_stamp(path, sha256)?;
    Ok(FileStatus::Ok)
}

/// `<name>.verified` next to a model file: its size, mtime and SHA-256 when it last matched.
fn stamp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".verified");
    path.with_file_name(name)
}

fn stamp_line(meta: &std::fs::Metadata, sha256: &str) -> Option<String> {
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{} {} {}\n", meta.len(), mtime, sha256))
}

fn stamp_matches(path: &Path, meta: &std::fs::Metadata, sha256: &str) -> bool {
    let Some(expected) = stamp_line(meta, sha256) else { return false };
    std::fs::read_to_string(stamp_path(path)).is_ok_and(|s| s == expected)
}

/// Records that `path` (as it is now) hashed to `sha256`. Call only after checking the hash.
pub fn write_stamp(path: &Path, sha256: &str) -> Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    let Some(line) = stamp_line(&meta, sha256) else { return Ok(()) };
    let stamp = stamp_path(path);
    crate::private_fs::write_replace(&stamp, line.as_bytes())
}

/// Computes a derived file from its (already downloaded) source and checks it against the pin.
/// Writes `<name>.part` first, so a failed or interrupted run leaves no wrong file in place.
pub fn derive(spec: &ModelSpec, models_dir: &Path, file: &DerivedFile) -> Result<()> {
    let dir = models_dir.join(&spec.dir);
    let src = dir.join(&file.from);
    let part = dir.join(format!("{}.part", file.name));
    match file.transform {
        Transform::BandedAttention => {
            crate::encoder_rewrite::band_attention_file(&src, &part)?;
        }
    }
    let actual = (std::fs::metadata(&part).map_err(|e| Error::io(&part, e))?.len(), sha256_file(&part)?);
    if actual != (file.size, file.sha256.clone()) {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Model(format!(
            "{}: derived file does not match its pin (got {} bytes, sha256 {}; expected {} bytes, sha256 {})",
            file.name, actual.0, actual.1, file.size, file.sha256
        )));
    }
    let target = dir.join(&file.name);
    std::fs::rename(&part, &target).map_err(|e| Error::io(&target, e))?;
    write_stamp(&target, &file.sha256)
}

/// Directory of a model whose files are present and match their pins ([`Check::Stamp`]: hashed
/// once, then trusted while size and mtime stay the same).
pub fn installed_dir(spec: &ModelSpec, models_dir: &Path) -> Result<PathBuf> {
    let bad: Vec<String> = verify(spec, models_dir, Check::Stamp)?
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

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn builtin_manifest_parses() {
        let m = Manifest::builtin();
        let p = m.get(PIANISSIMO).unwrap();
        assert_eq!(p.tagged_id(PIANISSIMO), "pianissimo-sv-int8@63730c6");
        assert!(p.files.iter().any(|f| f.name == "vocab.txt"));
        assert!(p.files.iter().any(|f| f.name == TOKENIZER && f.optional));
        let banded = &p.derived[0];
        assert_eq!(banded.transform, Transform::BandedAttention);
        assert!(p.files.iter().any(|f| f.name == banded.from));
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
                    optional: false,
                },
                ModelFile { name: "b".into(), path: None, size: 1, sha256: "00".into(), optional: false },
                ModelFile { name: "c".into(), path: None, size: 1, sha256: "00".into(), optional: false },
                ModelFile { name: "e".into(), path: None, size: 3, sha256: ABC.into(), optional: true },
            ],
            derived: vec![DerivedFile {
                name: "d".into(),
                from: "a".into(),
                transform: Transform::BandedAttention,
                size: 1,
                sha256: "00".into(),
            }],
        };
        std::fs::create_dir_all(dir.join("m")).unwrap();
        std::fs::write(dir.join("m/a"), "abc").unwrap();
        std::fs::write(dir.join("m/b"), "zz").unwrap();
        let st = verify(&spec, &dir, Check::Full).unwrap();
        assert_eq!(st[0].1, FileStatus::Ok);
        assert_eq!(st[1].1, FileStatus::WrongSize { actual: 2 });
        assert_eq!(st[2].1, FileStatus::Missing);
        assert_eq!(st[3], ("d".to_string(), FileStatus::Missing));
        // The optional file is not part of the check.
        assert_eq!(st.len(), 4);
        assert!(installed_dir(&spec, &dir).is_err());
        assert_eq!(optional_file(&spec, &dir, "e").unwrap(), None);
        std::fs::write(dir.join("m/e"), "abc").unwrap();
        assert_eq!(optional_file(&spec, &dir, "e").unwrap(), Some(dir.join("m/e")));
        assert_eq!(optional_file(&spec, &dir, "a").unwrap(), Some(dir.join("m/a")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stamp_catches_a_same_size_change() {
        let dir = std::env::temp_dir().join(format!("tyst-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a");
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        std::fs::write(&f, "abc").unwrap();
        // Not stamped yet: hashed once, then stamped.
        assert_eq!(file_status(&f, 3, abc, Check::Stamp).unwrap(), FileStatus::Ok);
        assert!(stamp_path(&f).exists());
        assert_eq!(file_status(&f, 3, abc, Check::Stamp).unwrap(), FileStatus::Ok);
        // Same size, different bytes (and a new mtime): the stamp no longer applies.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&f, "abd").unwrap();
        assert_eq!(file_status(&f, 3, abc, Check::Size).unwrap(), FileStatus::Ok);
        assert_eq!(file_status(&f, 3, abc, Check::Stamp).unwrap(), FileStatus::WrongChecksum);
        assert!(!stamp_path(&f).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
