//! Language routing (SPEC 6.3, ADR 0002, ADR 0003).
//!
//! v1 sends every segment to Pianissimo unless the user forces English, which uses Parakeet v3.
//! There is no language-ID model, so the language *label* (Markdown `languages`, per-turn tags)
//! comes from the forced setting or a text guess on the final text, with hysteresis: short
//! segments ("ok", "precis") inherit the channel's current language instead of flipping it.

use std::time::Duration;

use crate::text::guess_lang;
use crate::transcript::Lang;

/// User-selected session language (meeting window / pill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageMode {
    #[default]
    Auto,
    Swedish,
    English,
}

impl LanguageMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "sv" | "swedish" | "svenska" => Some(Self::Swedish),
            "en" | "english" => Some(Self::English),
            _ => None,
        }
    }
}

/// Which engine decodes a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineRole {
    /// Pianissimo.
    Primary,
    /// Parakeet v3, loaded on first use.
    English,
}

pub trait LanguageRouter: Send {
    /// Engine for the next segment.
    fn engine(&self) -> EngineRole;
    /// Language label for a decoded segment; updates the running language.
    fn label(&mut self, text: &str, duration: Duration) -> Lang;
    fn set_mode(&mut self, mode: LanguageMode);
    fn mode(&self) -> LanguageMode;
}

#[derive(Debug, Clone)]
pub struct FixedRouter {
    mode: LanguageMode,
    current: Lang,
    short_segment: Duration,
}

impl FixedRouter {
    /// `initial` is the meeting language prior; segments shorter than `short_segment` keep it.
    pub fn new(mode: LanguageMode, initial: Lang, short_segment: Duration) -> Self {
        Self { mode, current: initial, short_segment }
    }

    pub fn current(&self) -> Lang {
        self.current
    }
}

impl Default for FixedRouter {
    fn default() -> Self {
        Self::new(LanguageMode::Auto, Lang::Sv, Duration::from_millis(1500))
    }
}

impl LanguageRouter for FixedRouter {
    fn engine(&self) -> EngineRole {
        match self.mode {
            LanguageMode::English => EngineRole::English,
            LanguageMode::Auto | LanguageMode::Swedish => EngineRole::Primary,
        }
    }

    fn label(&mut self, text: &str, duration: Duration) -> Lang {
        let lang = match self.mode {
            LanguageMode::Swedish => Lang::Sv,
            LanguageMode::English => Lang::En,
            LanguageMode::Auto if duration < self.short_segment => self.current,
            LanguageMode::Auto => guess_lang(text).unwrap_or(self.current),
        };
        self.current = lang;
        lang
    }

    fn set_mode(&mut self, mode: LanguageMode) {
        self.mode = mode;
    }

    fn mode(&self) -> LanguageMode {
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    #[test]
    fn auto_uses_pianissimo_and_guesses_labels() {
        let mut r = FixedRouter::default();
        assert_eq!(r.engine(), EngineRole::Primary);
        assert_eq!(r.label("vi måste fixa vår state drift innan deploy", secs(3.0)), Lang::Sv);
        assert_eq!(r.label("sorry I'm late, should we switch to English?", secs(3.0)), Lang::En);
        assert_eq!(r.current(), Lang::En);
    }

    #[test]
    fn short_segments_keep_the_current_language() {
        let mut r = FixedRouter::default();
        r.label("we should start with the agenda for today", secs(4.0));
        // "precis" is Swedish, but a short reply must not flip the label.
        assert_eq!(r.label("Precis, och det är bra.", secs(1.2)), Lang::En);
        assert_eq!(r.current(), Lang::En);
        // A long Swedish turn does flip it.
        assert_eq!(r.label("Precis, och det är bra att vi tar det nu.", secs(2.5)), Lang::Sv);
    }

    #[test]
    fn no_signal_keeps_the_current_language() {
        let mut r = FixedRouter::new(LanguageMode::Auto, Lang::En, secs(1.5));
        assert_eq!(r.label("Terraform Kubernetes Helm", secs(3.0)), Lang::En);
    }

    #[test]
    fn forced_modes_pick_engine_and_label() {
        let mut r = FixedRouter::default();
        r.set_mode(LanguageMode::English);
        assert_eq!(r.engine(), EngineRole::English);
        assert_eq!(r.label("vi måste fixa det här", secs(3.0)), Lang::En);
        r.set_mode(LanguageMode::Swedish);
        assert_eq!(r.engine(), EngineRole::Primary);
        assert_eq!(r.label("we need to fix this", secs(3.0)), Lang::Sv);
    }
}
