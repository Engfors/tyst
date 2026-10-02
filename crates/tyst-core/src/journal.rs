//! Crash-safety journal (SPEC 6.6): final segments are appended to
//! `<transcripts>/.tyst-journal/<session-id>.jsonl` as they arrive. An orphaned journal on the
//! next launch can be turned back into a Markdown file; the journal is deleted after a save.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::transcript::{Marker, Segment, Session, SessionInfo};
use crate::{Error, Result};

pub const JOURNAL_DIR: &str = ".tyst-journal";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Record {
    Session(SessionInfo),
    Segment(Segment),
    Marker(Marker),
    Title { title: String },
    End { ended_at: DateTime<FixedOffset> },
}

pub struct Journal {
    path: PathBuf,
    file: File,
}

impl Journal {
    /// Starts a journal for a new session.
    pub fn create(transcripts_dir: &Path, info: &SessionInfo) -> Result<Self> {
        let dir = transcripts_dir.join(JOURNAL_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let path = dir.join(format!("{}.jsonl", info.id));
        let file = OpenOptions::new().create_new(true).append(true).open(&path).map_err(|e| Error::io(&path, e))?;
        let mut j = Self { path, file };
        j.write(&Record::Session(info.clone()))?;
        Ok(j)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a final segment and flushes it to disk.
    pub fn append(&mut self, segment: &Segment) -> Result<()> {
        self.write(&Record::Segment(segment.clone()))
    }

    pub fn mark(&mut self, marker: Marker) -> Result<()> {
        self.write(&Record::Marker(marker))
    }

    pub fn set_title(&mut self, title: &str) -> Result<()> {
        self.write(&Record::Title { title: title.to_string() })
    }

    pub fn end(&mut self, ended_at: DateTime<FixedOffset>) -> Result<()> {
        self.write(&Record::End { ended_at })
    }

    /// Deletes the journal; call only after the Markdown file is safely written.
    pub fn remove(self) -> Result<()> {
        let path = self.path.clone();
        drop(self.file);
        std::fs::remove_file(&path).map_err(|e| Error::io(&path, e))
    }

    fn write(&mut self, record: &Record) -> Result<()> {
        let mut line = serde_json::to_string(record).map_err(|e| Error::Journal(e.to_string()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes()).map_err(|e| Error::io(&self.path, e))?;
        self.file.sync_data().map_err(|e| Error::io(&self.path, e))
    }
}

/// Journals left behind by sessions that never saved, oldest first.
pub fn find_orphans(transcripts_dir: &Path) -> Result<Vec<PathBuf>> {
    let dir = transcripts_dir.join(JOURNAL_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(&dir, e)),
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    out.sort();
    Ok(out)
}

/// Reads a journal back into a session. A torn last line (crash mid-write) is ignored. Without an
/// end record, the end time is the end of the last segment.
pub fn recover(path: &Path) -> Result<Session> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut info: Option<SessionInfo> = None;
    let mut segments = Vec::new();
    let mut markers = Vec::new();
    let mut title = None;
    let mut ended_at = None;
    let lines: Vec<String> =
        BufReader::new(file).lines().collect::<std::io::Result<_>>().map_err(|e| Error::io(path, e))?;
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(line) {
            Ok(Record::Session(s)) => info = Some(s),
            Ok(Record::Segment(s)) => segments.push(s),
            Ok(Record::Marker(m)) => markers.push(m),
            Ok(Record::Title { title: t }) => title = Some(t),
            Ok(Record::End { ended_at: e }) => ended_at = Some(e),
            Err(_) if i == last => break,
            Err(e) => return Err(Error::Journal(format!("{}: line {}: {e}", path.display(), i + 1))),
        }
    }
    let info = info.ok_or_else(|| Error::Journal(format!("{}: no session header", path.display())))?;
    let ended_at = ended_at.unwrap_or_else(|| {
        let last_end = segments.iter().map(|s| s.end).max().unwrap_or_default();
        info.started_at + chrono::Duration::from_std(last_end).unwrap_or_default()
    });
    Ok(Session { info, ended_at, title, segments, markers })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::transcript::{Channel, Lang, MarkerKind, SegState, SpeakerLabels};

    fn info(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            started_at: DateTime::parse_from_rfc3339("2026-10-02T10:15:00+02:00").unwrap(),
            app: "Tyst 0.1.0".into(),
            models: vec!["pianissimo-sv-int8@63730c6".into()],
            labels: SpeakerLabels::default(),
        }
    }

    fn seg(id: u64, end_s: u64) -> Segment {
        Segment {
            id,
            channel: Channel::Me,
            start: Duration::from_secs(end_s - 2),
            end: Duration::from_secs(end_s),
            lang: Lang::Sv,
            engine: "pianissimo-sv-int8@63730c6".into(),
            text: format!("mening {id}"),
            state: SegState::Final,
            edited: false,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tyst-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn recovers_an_orphaned_session() {
        let dir = tmp("orphan");
        let mut j = Journal::create(&dir, &info("s1")).unwrap();
        j.append(&seg(1, 5)).unwrap();
        j.append(&seg(2, 65)).unwrap();
        // Simulate a crash: the journal is never removed.
        drop(j);
        let orphans = find_orphans(&dir).unwrap();
        assert_eq!(orphans.len(), 1);
        let s = recover(&orphans[0]).unwrap();
        assert_eq!(s.segments.len(), 2);
        assert_eq!(s.info.id, "s1");
        assert_eq!(s.ended_at.format("%H:%M:%S").to_string(), "10:16:05");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_a_torn_last_line() {
        let dir = tmp("torn");
        let mut j = Journal::create(&dir, &info("s2")).unwrap();
        j.append(&seg(1, 5)).unwrap();
        j.mark(Marker { at: Duration::from_secs(6), kind: MarkerKind::Paused }).unwrap();
        j.set_title("Planering").unwrap();
        let path = j.path().to_path_buf();
        drop(j);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"type\":\"segment\",\"id\":2,\"chan").unwrap();
        let s = recover(&path).unwrap();
        assert_eq!(s.segments.len(), 1);
        assert_eq!(s.title.as_deref(), Some("Planering"));
        assert_eq!(s.markers, vec![Marker { at: Duration::from_secs(6), kind: MarkerKind::Paused }]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_middle_line_is_an_error() {
        let dir = tmp("corrupt");
        let j = Journal::create(&dir, &info("s3")).unwrap();
        let path = j.path().to_path_buf();
        drop(j);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"garbage\n{\"type\":\"title\",\"title\":\"x\"}\n").unwrap();
        assert!(recover(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn end_record_and_remove() {
        let dir = tmp("end");
        let mut j = Journal::create(&dir, &info("s4")).unwrap();
        j.append(&seg(1, 5)).unwrap();
        j.end(DateTime::parse_from_rfc3339("2026-10-02T11:00:00+02:00").unwrap()).unwrap();
        let s = recover(j.path()).unwrap();
        assert_eq!(s.ended_at.format("%H:%M").to_string(), "11:00");
        j.remove().unwrap();
        assert!(find_orphans(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
