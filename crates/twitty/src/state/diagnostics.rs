//! The diagnostics section of the settings page: a one-line verdict on
//! whether the provider's data is still understood.

use twitty_model::RequestHealth;

/// What the diagnostics section says overall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No requests yet (or a backend that doesn't track health).
    Nothing,
    /// Every row was understood.
    Healthy,
    /// `unreadable` rows were not understood: the provider changed its format.
    Drift { unreadable: u64 },
}

impl Verdict {
    pub fn text(&self) -> String {
        match self {
            Verdict::Nothing => "Nothing to report (no X requests yet, or demo data).".into(),
            Verdict::Healthy => "Every row X sent was understood.".into(),
            Verdict::Drift { unreadable } => format!(
                "{unreadable} row(s) couldn't be read: X may have changed its format (see the runbook in docs/x-api.md)."
            ),
        }
    }
}

/// The verdict for `health`.
pub fn verdict(health: &[RequestHealth]) -> Verdict {
    if health.is_empty() {
        return Verdict::Nothing;
    }
    match health.iter().map(|h| h.unsupported).sum() {
        0 => Verdict::Healthy,
        unreadable => Verdict::Drift { unreadable },
    }
}

/// Whether a request type should be highlighted.
pub fn flagged(health: &RequestHealth) -> bool {
    health.unsupported > 0 || health.errors > 0 || health.warnings > 0
}

/// The counters of one request type, on one line.
pub fn line(health: &RequestHealth) -> String {
    format!(
        "{} ok · {} rows · {} unreadable · {} errors",
        health.ok, health.rows, health.unsupported, health.errors
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, unsupported: u64, errors: u64) -> RequestHealth {
        RequestHealth {
            name: name.into(),
            ok: 3,
            rows: 60,
            unsupported,
            errors,
            ..Default::default()
        }
    }

    #[test]
    fn verdicts() {
        assert_eq!(verdict(&[]), Verdict::Nothing);
        assert_eq!(verdict(&[h("HomeTimeline", 0, 2)]), Verdict::Healthy, "errors aren't drift");
        assert_eq!(
            verdict(&[h("HomeTimeline", 2, 0), h("Bookmarks", 1, 0)]),
            Verdict::Drift { unreadable: 3 }
        );
        assert!(Verdict::Drift { unreadable: 3 }.text().starts_with("3 row(s)"));
    }

    #[test]
    fn lines_and_flags() {
        assert_eq!(line(&h("x", 1, 2)), "3 ok · 60 rows · 1 unreadable · 2 errors");
        assert!(flagged(&h("x", 0, 1)));
        assert!(!flagged(&h("x", 0, 0)));
    }
}
