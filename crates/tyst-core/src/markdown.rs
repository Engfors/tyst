//! Markdown output (SPEC 7).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::transcript::{Channel, Session};
use crate::{Error, Result};

const DEFAULT_TITLE: &str = "Meeting";
const MAX_TITLE_CHARS: usize = 100;

/// Renders the transcript: YAML front matter, a heading, then one paragraph per turn
/// (consecutive segments of the same channel merged).
pub fn render(session: &Session) -> String {
    let info = &session.info;
    let title = display_title(session);
    let minutes = (session.ended_at - info.started_at).num_seconds().max(0) as f64 / 60.0;
    let languages: Vec<&str> = session.languages().iter().map(|l| l.code()).collect();
    let models: Vec<String> = info.models.iter().map(|m| yaml_plain_or_quoted(m)).collect();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("title: {}\n", yaml_plain_or_quoted(&title)));
    out.push_str(&format!("date: {}\n", info.started_at.format("%Y-%m-%d")));
    out.push_str(&format!("start: \"{}\"\n", info.started_at.format("%H:%M")));
    out.push_str(&format!("end: \"{}\"\n", session.ended_at.format("%H:%M")));
    out.push_str(&format!("duration: {}\n", format_duration(minutes)));
    out.push_str(&format!("languages: [{}]\n", languages.join(", ")));
    out.push_str(&format!("app: {}\n", yaml_plain_or_quoted(&info.app)));
    out.push_str(&format!("models: [{}]\n", models.join(", ")));
    out.push_str("---\n\n");
    out.push_str(&format!("# {title}\n"));

    for (channel, text) in turns(session) {
        out.push_str(&format!("\n**{}:** {}\n", info.labels.get(channel), text));
    }
    out
}

/// Writes the transcript into `dir` as `YYYY-MM-DD HHmm <title>.md`, never overwriting an
/// existing file. Returns the path written.
pub fn save(session: &Session, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let body = render(session);
    let stem =
        format!("{} {}", session.info.started_at.format("%Y-%m-%d %H%M"), sanitize_file_name(&display_title(session)));
    for n in 1.. {
        let name = if n == 1 { format!("{stem}.md") } else { format!("{stem} ({n}).md") };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(body.as_bytes()).map_err(|e| Error::io(&path, e))?;
                f.sync_all().map_err(|e| Error::io(&path, e))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    unreachable!()
}

/// Consecutive same-channel segments merged, in start order.
pub fn turns(session: &Session) -> Vec<(Channel, String)> {
    let mut segs: Vec<_> = session.segments.iter().filter(|s| !s.text.trim().is_empty()).collect();
    segs.sort_by_key(|s| (s.start, s.id));
    let mut out: Vec<(Channel, String)> = Vec::new();
    for s in segs {
        let text = s.text.trim();
        match out.last_mut() {
            Some((ch, acc)) if *ch == s.channel => {
                acc.push(' ');
                acc.push_str(text);
            }
            _ => out.push((s.channel, text.to_string())),
        }
    }
    out
}

fn display_title(session: &Session) -> String {
    let t = session.title.as_deref().map(str::trim).unwrap_or("");
    if t.is_empty() { DEFAULT_TITLE.to_string() } else { t.chars().filter(|c| !c.is_control()).collect() }
}

/// Makes a title safe as a file name on macOS and Linux (and harmless on Windows shares).
pub fn sanitize_file_name(title: &str) -> String {
    let cleaned: String =
        title
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    ' '
                } else {
                    c
                }
            })
            .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String =
        collapsed.trim_matches(|c: char| c == '.' || c.is_whitespace()).chars().take(MAX_TITLE_CHARS).collect();
    let trimmed = trimmed.trim_end().to_string();
    if trimmed.is_empty() { DEFAULT_TITLE.to_string() } else { trimmed }
}

fn format_duration(minutes: f64) -> String {
    let total = minutes.round() as i64;
    if total < 60 { format!("{total}m") } else { format!("{}h {:02}m", total / 60, total % 60) }
}

/// A YAML scalar: plain when that is unambiguous, otherwise double-quoted.
fn yaml_plain_or_quoted(s: &str) -> String {
    let needs_quotes = s.is_empty()
        || s.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c) || c.is_whitespace())
        || s.ends_with(char::is_whitespace)
        || s.contains(": ")
        || s.contains(" #")
        || s.contains([',', '[', ']', '{', '}'])
        || matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "null" | "~" | "on" | "off")
        || s.parse::<f64>().is_ok();
    if !needs_quotes {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::DateTime;

    use super::*;
    use crate::transcript::{Lang, SegState, Segment, SessionInfo, SpeakerLabels};

    fn seg(id: u64, channel: Channel, start_s: u64, lang: Lang, text: &str) -> Segment {
        Segment {
            id,
            channel,
            start: Duration::from_secs(start_s),
            end: Duration::from_secs(start_s + 2),
            lang,
            engine: "pianissimo-sv-int8@63730c6".into(),
            text: text.into(),
            state: SegState::Final,
            edited: false,
        }
    }

    fn session(title: Option<&str>) -> Session {
        Session {
            info: SessionInfo {
                id: "s1".into(),
                started_at: DateTime::parse_from_rfc3339("2026-10-02T10:15:00+02:00").unwrap(),
                app: "Tyst 0.1.0".into(),
                models: vec!["pianissimo-sv-int8@63730c6".into()],
                labels: SpeakerLabels::default(),
            },
            ended_at: DateTime::parse_from_rfc3339("2026-10-02T10:52:20+02:00").unwrap(),
            title: title.map(String::from),
            segments: vec![
                seg(3, Channel::Others, 9, Lang::Sv, "Låter bra."),
                seg(1, Channel::Me, 0, Lang::Sv, "Hej allihop,"),
                seg(2, Channel::Me, 3, Lang::Sv, "vi börjar med Terraform."),
                seg(4, Channel::Others, 12, Lang::Sv, "Vi har frågor."),
                seg(5, Channel::Others, 20, Lang::En, "Sorry I'm late."),
                seg(6, Channel::Me, 25, Lang::En, "  "),
                seg(7, Channel::Me, 30, Lang::En, "Sure, no problem."),
            ],
        }
    }

    #[test]
    fn renders_spec_layout() {
        let md = render(&session(Some("Customer sync – Acme")));
        let expected = "---\n\
title: Customer sync – Acme\n\
date: 2026-10-02\n\
start: \"10:15\"\n\
end: \"10:52\"\n\
duration: 37m\n\
languages: [sv, en]\n\
app: Tyst 0.1.0\n\
models: [pianissimo-sv-int8@63730c6]\n\
---\n\
\n\
# Customer sync – Acme\n\
\n\
**Me:** Hej allihop, vi börjar med Terraform.\n\
\n\
**Others:** Låter bra. Vi har frågor. Sorry I'm late.\n\
\n\
**Me:** Sure, no problem.\n";
        assert_eq!(md, expected);
    }

    #[test]
    fn quotes_titles_that_would_break_yaml() {
        let md = render(&session(Some("Q3: plan, \"budget\"")));
        assert!(md.contains("title: \"Q3: plan, \\\"budget\\\"\"\n"), "{md}");
        assert!(md.contains("# Q3: plan, \"budget\"\n"));
    }

    #[test]
    fn uses_custom_labels_and_default_title() {
        let mut s = session(None);
        s.info.labels = SpeakerLabels { me: "Emil".into(), others: "Övriga".into() };
        let md = render(&s);
        assert!(md.contains("title: Meeting\n"));
        assert!(md.contains("**Emil:** Hej allihop"));
        assert!(md.contains("**Övriga:** Låter bra."));
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("a/b\\c: d*e?\"f\"<g>|h"), "a b c d e f g h");
        assert_eq!(sanitize_file_name("  ..hidden.  "), "hidden");
        assert_eq!(sanitize_file_name("???"), "Meeting");
        assert_eq!(sanitize_file_name(&"x".repeat(300)).len(), 100);
    }

    #[test]
    fn save_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("tyst-md-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = session(Some("Sync / weekly"));
        let a = save(&s, &dir).unwrap();
        let b = save(&s, &dir).unwrap();
        assert_eq!(a.file_name().unwrap(), "2026-10-02 1015 Sync weekly.md");
        assert_eq!(b.file_name().unwrap(), "2026-10-02 1015 Sync weekly (2).md");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), render(&s));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn formats_long_durations() {
        assert_eq!(format_duration(37.3), "37m");
        assert_eq!(format_duration(125.0), "2h 05m");
    }
}
