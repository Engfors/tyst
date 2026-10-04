//! The meeting session's lifecycle, kept free of capture and UI so its races can be tested.
//!
//! Idle → Starting → Recording → Stopping → Naming → Saving → Idle. A start only begins from
//! Idle, so a start that lands while a stop is still joining threads or a save is still writing
//! does nothing. Stop or quit during Starting cancels the start: the loading thread finds its
//! generation gone and stops the meeting it just started instead of storing it.

/// One meeting's lifecycle. `M` is a running meeting, `S` a stopped one waiting for its name.
pub enum Session<M, S> {
    Idle,
    /// Loading models; `generation` identifies this start.
    Starting {
        generation: u64,
    },
    Recording(M),
    /// Capture stopped; the final decodes are running.
    Stopping,
    Naming {
        stopped: S,
        token: u64,
    },
    /// The Markdown file is being written.
    Saving,
}

/// What [`Session::cancel`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Cancel {
    /// A start was loading models; it won't record.
    Start,
    Nothing,
}

impl<M, S> Session<M, S> {
    /// Idle → Starting. Returns the start's generation, or `None` when not idle.
    pub fn start(&mut self, generation: u64) -> Option<u64> {
        if !matches!(self, Session::Idle) {
            return None;
        }
        *self = Session::Starting { generation };
        Some(generation)
    }

    /// Starting → Recording, only for the start that is still current. Otherwise the meeting
    /// comes back so the caller can stop it.
    pub fn begin(&mut self, generation: u64, meeting: M) -> Result<(), M> {
        match self {
            Session::Starting { generation: g } if *g == generation => {
                *self = Session::Recording(meeting);
                Ok(())
            }
            _ => Err(meeting),
        }
    }

    /// True while this start is still wanted.
    pub fn is_starting(&self, generation: u64) -> bool {
        matches!(self, Session::Starting { generation: g } if *g == generation)
    }

    /// A start that failed goes back to Idle (if it is still current).
    pub fn start_failed(&mut self, generation: u64) {
        if self.is_starting(generation) {
            *self = Session::Idle;
        }
    }

    /// Cancels a start that is still loading.
    pub fn cancel(&mut self) -> Cancel {
        if matches!(self, Session::Starting { .. }) {
            *self = Session::Idle;
            Cancel::Start
        } else {
            Cancel::Nothing
        }
    }

    /// Recording → Stopping, handing out the meeting to stop.
    pub fn stop(&mut self) -> Option<M> {
        match std::mem::replace(self, Session::Stopping) {
            Session::Recording(m) => Some(m),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Stopping → Naming once the threads have joined.
    pub fn stopped(&mut self, stopped: S, token: u64) {
        debug_assert!(matches!(self, Session::Stopping));
        *self = Session::Naming { stopped, token };
    }

    /// Stopping → Idle when stopping failed.
    pub fn stop_failed(&mut self) {
        if matches!(self, Session::Stopping) {
            *self = Session::Idle;
        }
    }

    /// Naming → Saving, handing out the stopped meeting and its token.
    pub fn save(&mut self) -> Option<(S, u64)> {
        match std::mem::replace(self, Session::Saving) {
            Session::Naming { stopped, token } => Some((stopped, token)),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Saving → Idle.
    pub fn saved(&mut self) {
        if matches!(self, Session::Saving) {
            *self = Session::Idle;
        }
    }

    /// Saving → Naming: a failed save keeps the meeting so it can be saved again.
    pub fn save_failed(&mut self, stopped: S, token: u64) {
        debug_assert!(matches!(self, Session::Saving));
        *self = Session::Naming { stopped, token };
    }

    pub fn naming_token(&self) -> Option<u64> {
        match self {
            Session::Naming { token, .. } => Some(*token),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type S = Session<&'static str, &'static str>;

    #[test]
    fn stop_while_starting_cancels_the_start() {
        let mut s = S::Idle;
        let generation = s.start(1).unwrap();
        assert_eq!(s.cancel(), Cancel::Start);
        // The loading thread finishes afterwards: its meeting comes back to be stopped.
        assert_eq!(s.begin(generation, "meeting"), Err("meeting"));
        assert!(matches!(s, Session::Idle));
    }

    #[test]
    fn a_new_start_after_a_cancel_is_not_taken_over_by_the_old_one() {
        let mut s = S::Idle;
        let old = s.start(1).unwrap();
        s.cancel();
        let new = s.start(2).unwrap();
        assert_eq!(s.begin(old, "old"), Err("old"));
        assert!(s.is_starting(new));
        s.start_failed(old);
        assert!(s.is_starting(new), "a stale failure must not reset the new start");
        assert_eq!(s.begin(new, "new"), Ok(()));
        assert!(matches!(s, Session::Recording("new")));
    }

    #[test]
    fn start_does_nothing_while_stopping_or_saving() {
        let mut s = S::Idle;
        s.start(1);
        s.begin(1, "m").unwrap();
        assert_eq!(s.stop(), Some("m"));
        assert_eq!(s.start(2), None, "start while the stop joins threads");
        s.stopped("stopped", 7);
        assert_eq!(s.naming_token(), Some(7));
        assert_eq!(s.start(3), None, "start while naming");
        let (stopped, token) = s.save().unwrap();
        assert_eq!(s.start(4), None, "start while saving");
        // A failed save keeps the meeting.
        s.save_failed(stopped, token);
        assert_eq!(s.naming_token(), Some(7));
        s.save().unwrap();
        s.saved();
        assert_eq!(s.start(5), Some(5));
    }

    #[test]
    fn stop_and_save_do_nothing_in_other_states() {
        let mut s = S::Idle;
        assert_eq!(s.stop(), None);
        assert!(s.save().is_none());
        assert!(matches!(s, Session::Idle));
        s.start(1);
        assert_eq!(s.stop(), None);
        assert!(s.is_starting(1));
        assert_eq!(S::Idle.cancel(), Cancel::Nothing);
    }

    #[test]
    fn failed_stop_returns_to_idle() {
        let mut s = S::Idle;
        s.start(1);
        s.begin(1, "m").unwrap();
        s.stop().unwrap();
        s.stop_failed();
        assert!(matches!(s, Session::Idle));
    }
}
