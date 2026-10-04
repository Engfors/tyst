//! Update check (SPEC 9.7): asks GitHub for the repository's latest release and compares its
//! version with this build. This is the second of the two network uses SPEC 0 allows; it sends
//! nothing but the request (and the token, when the repository is private), never installs
//! anything, and the app lets the user turn it off.

use std::time::Duration;

use serde::Deserialize;

use crate::{Error, Result};

/// The repository releases come from.
pub const REPO: &str = "Engfors/tyst";

/// Release notes longer than this are cut (the Settings view links to the full page).
const MAX_NOTES: usize = 4000;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Release {
    /// The version without the tag's `v`, e.g. `0.2.0`.
    pub version: String,
    pub tag: String,
    /// The release page, where the user downloads it.
    pub url: String,
    pub notes: String,
    pub published_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
}

/// The newest published release (GitHub's "latest": not a draft, not a pre-release).
/// `token` is needed while the repository is private.
pub fn latest_release(repo: &str, token: Option<&str>, user_agent: &str) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let agent = crate::fetch::agent_with_timeout(Duration::from_secs(20));
    let mut req = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", user_agent);
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        req = req.header("Authorization", &format!("Bearer {}", t.trim()));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => return Err(Error::Other(status_message(code, token.is_some()))),
        Err(e) => return Err(Error::Other(format!("update check: {e}"))),
    };
    let api: ApiRelease = resp
        .into_body()
        .with_config()
        .limit(1 << 20)
        .read_json()
        .map_err(|e| Error::Other(format!("update check: unexpected answer from GitHub ({e})")))?;
    if api.draft || api.prerelease {
        return Err(Error::Other("update check: GitHub returned a draft or pre-release".into()));
    }
    let mut notes = api.body.unwrap_or_default().replace("\r\n", "\n");
    if notes.len() > MAX_NOTES {
        let mut cut = MAX_NOTES;
        while !notes.is_char_boundary(cut) {
            cut -= 1;
        }
        notes.truncate(cut);
        notes.push('…');
    }
    Ok(Release {
        version: api.tag_name.trim_start_matches('v').to_string(),
        tag: api.tag_name,
        url: api.html_url,
        notes,
        published_at: api.published_at,
    })
}

fn status_message(code: u16, with_token: bool) -> String {
    match code {
        401 => "The GitHub token was rejected. Check it in Settings › Updates.".into(),
        403 | 429 => "GitHub refused the update check for now (rate limit). Tyst tries again later.".into(),
        404 if with_token => "No release found. The token may not have access to the repository.".into(),
        404 => "No release found. While the repository is private, the check needs a GitHub token.".into(),
        c => format!("update check: GitHub answered {c}"),
    }
}

/// Whether `latest` is a newer version than `current` (semantic versioning; a pre-release is
/// older than its release). Unparsable versions never count as newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(l), Ok(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("latest", "0.1.0"));
    }

    #[test]
    fn explains_github_errors() {
        assert!(status_message(404, false).contains("token"));
        assert!(status_message(401, true).contains("rejected"));
    }
}
