//! Transcript model (SPEC 6.5).

use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// The user's microphone.
    Me,
    /// System audio.
    Others,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Sv,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Sv => "sv",
            Lang::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sv" | "swedish" | "svenska" => Some(Lang::Sv),
            "en" | "english" => Some(Lang::En),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegState {
    Partial,
    Final,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: u64,
    pub channel: Channel,
    /// Offset from the session start.
    #[serde(with = "duration_ms")]
    pub start: Duration,
    #[serde(with = "duration_ms")]
    pub end: Duration,
    pub lang: Lang,
    /// Engine id with revision, e.g. `pianissimo-sv-int8@63730c6`.
    pub engine: String,
    pub text: String,
    pub state: SegState,
    #[serde(default)]
    pub edited: bool,
}

/// Speaker labels written in the Markdown file (SPEC 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerLabels {
    pub me: String,
    pub others: String,
}

impl Default for SpeakerLabels {
    fn default() -> Self {
        Self { me: "Me".into(), others: "Others".into() }
    }
}

impl SpeakerLabels {
    pub fn get(&self, channel: Channel) -> &str {
        match channel {
            Channel::Me => &self.me,
            Channel::Others => &self.others,
        }
    }
}

/// What a recording session knows about itself, apart from its segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub started_at: DateTime<FixedOffset>,
    pub app: String,
    /// Engine ids with revisions.
    pub models: Vec<String>,
    #[serde(default)]
    pub labels: SpeakerLabels,
}

/// A finished (or recovered) meeting.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub info: SessionInfo,
    pub ended_at: DateTime<FixedOffset>,
    pub title: Option<String>,
    /// Final segments; order does not matter, the writer sorts by start time.
    pub segments: Vec<Segment>,
}

impl Session {
    /// Languages present, in a stable order.
    pub fn languages(&self) -> Vec<Lang> {
        let mut langs: Vec<Lang> = self.segments.iter().map(|s| s.lang).collect();
        langs.sort();
        langs.dedup();
        langs
    }
}

/// A new random-enough session id: start time plus process-unique counter, no extra crates.
pub fn new_session_id(started_at: &DateTime<FixedOffset>) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{n}", started_at.format("%Y%m%dT%H%M%S"), std::process::id())
}

mod duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(d)?))
    }
}
