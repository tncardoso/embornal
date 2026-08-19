//! Facts and their signal strength.
//!
//! A fact is one small statement that belongs to a path. Facts do not change:
//! to correct a fact, write a new one that supersedes the old one. This keeps
//! the history of what the memory believed at each moment.

use crate::memory::path::{PathId, WikiPath};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

/// The stability, in days, of a fact that nobody recalled yet.
pub const INITIAL_STABILITY_DAYS: f64 = 1.0;

/// How strongly one recall pushes the stability up.
///
/// The gain is scaled by how much the fact was forgotten, so a recall of a
/// fact that is almost gone helps much more than a recall of a fact that the
/// reader saw one minute ago. This is the spacing effect.
///
/// The value is calibrated: a fact that the reader recalls on day 2, day 8,
/// day 30 and day 90 reaches a stability of about 120 days. One recall of a
/// fact that is fully lost multiplies its stability by 3.75.
pub const STABILITY_GAIN: f64 = 2.75;

/// The largest stability that a fact can reach, in days. Ten years.
pub const MAX_STABILITY_DAYS: f64 = 3650.0;

/// The primary key of a row in `facts`. Internal to one database file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FactId(pub i64);

impl fmt::Display for FactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A fact as it is stored.
///
/// The embedding is not part of this record. It is large and it is rarely
/// needed, so the reader loads it on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: FactId,
    /// The public identifier. Use this in output, not [`FactId`].
    pub ulid: Ulid,
    pub path_id: PathId,
    pub path: WikiPath,
    pub content: String,
    /// The subject that wrote the fact.
    pub owner: String,
    pub created_at: DateTime<Utc>,
    /// The recall state of the fact.
    pub signal: Signal,
    /// The fact that this one replaces, if any.
    pub supersedes_id: Option<FactId>,
    /// The moment of the soft delete. A live fact holds `None`.
    pub deleted_at: Option<DateTime<Utc>>,
    /// The name of the model that produced the stored embedding.
    pub embedding_model: Option<String>,
}

impl Fact {
    /// Returns `true` if the fact is live.
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// What the memory writes when it stores a new fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewFact {
    pub path: WikiPath,
    pub content: String,
    pub tags: Vec<crate::memory::tag::Tag>,
    pub supersedes_id: Option<FactId>,
}

/// The recall state of a fact.
///
/// The memory keeps the raw counters and the stability. The strength itself
/// is a function of the clock, so it is computed and never stored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    /// The moment of the last recall. `None` if nobody recalled the fact.
    pub last_recall_at: Option<DateTime<Utc>>,
    pub recall_count: u32,
    /// The time constant of the forgetting curve, in days.
    pub stability_days: f64,
    /// The moment from which the decay is measured. This is the creation of
    /// the fact until the first recall happens.
    reference: DateTime<Utc>,
}

impl Signal {
    /// Builds the signal of a fact that was just written.
    pub fn new(created_at: DateTime<Utc>) -> Self {
        Self {
            last_recall_at: None,
            recall_count: 0,
            stability_days: INITIAL_STABILITY_DAYS,
            reference: created_at,
        }
    }

    /// Rebuilds the signal from the stored columns.
    pub fn from_parts(
        created_at: DateTime<Utc>,
        last_recall_at: Option<DateTime<Utc>>,
        recall_count: u32,
        stability_days: f64,
    ) -> Self {
        Self {
            last_recall_at,
            recall_count,
            stability_days,
            reference: last_recall_at.unwrap_or(created_at),
        }
    }

    /// Returns the strength of the fact at `now`, between 0.0 and 1.0.
    ///
    /// The curve is `exp(-elapsed / stability)`. A fact that was just seen has
    /// a strength near 1.0. A fact that nobody read for much longer than its
    /// stability goes to 0.0.
    pub fn strength_at(&self, now: DateTime<Utc>) -> f64 {
        let elapsed = (now - self.reference).max(Duration::zero());
        let elapsed_days = elapsed.as_seconds_f64() / 86_400.0;
        (-elapsed_days / self.stability_days).exp()
    }

    /// Applies one recall at `now` and returns the new signal.
    ///
    /// The stability grows by an amount that depends on how much the fact was
    /// forgotten. A recall of a strong fact adds almost nothing.
    #[must_use]
    pub fn reinforce(&self, now: DateTime<Utc>) -> Self {
        let strength = self.strength_at(now);
        let stability = (self.stability_days * (1.0 + STABILITY_GAIN * (1.0 - strength)))
            .min(MAX_STABILITY_DAYS);
        Self {
            last_recall_at: Some(now),
            recall_count: self.recall_count.saturating_add(1),
            stability_days: stability,
            reference: now,
        }
    }

    /// Returns the moment at which the strength falls to `target`.
    ///
    /// This is the point at which a refresh is worth the effort.
    pub fn decays_to(&self, target: f64) -> Option<DateTime<Utc>> {
        if !(0.0..1.0).contains(&target) || target <= 0.0 {
            return None;
        }
        let days = -target.ln() * self.stability_days;
        let seconds = (days * 86_400.0).round();
        Duration::try_seconds(seconds as i64).map(|d| self.reference + d)
    }
}

/// How `cat` and `recall` sort the facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderBy {
    /// Oldest first. This is how a document reads.
    #[default]
    Date,
    /// Strongest first.
    Signal,
}

impl std::str::FromStr for OrderBy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "date" => Ok(Self::Date),
            "signal" => Ok(Self::Signal),
            other => Err(format!("unknown order '{other}': use 'date' or 'signal'")),
        }
    }
}

impl fmt::Display for OrderBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Date => f.write_str("date"),
            Self::Signal => f.write_str("signal"),
        }
    }
}

/// A fact together with the score that a search gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredFact {
    pub fact: Fact,
    /// The relevance of the keyword match, if the keyword index answered.
    pub keyword_score: Option<f64>,
    /// The relevance of the vector match, if the vector index answered.
    pub vector_score: Option<f64>,
    /// The strength of the fact at the moment of the search.
    pub signal_strength: f64,
    /// The value that decides the final order.
    pub score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(days: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + Duration::days(days)
    }

    #[test]
    fn a_new_fact_is_strong() {
        let signal = Signal::new(at(0));
        assert!((signal.strength_at(at(0)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn strength_falls_with_time() {
        let signal = Signal::new(at(0));
        let day1 = signal.strength_at(at(1));
        let day7 = signal.strength_at(at(7));
        assert!(day1 < 1.0 && day7 < day1);
        // One stability unit gives exactly 1/e.
        assert!((day1 - std::f64::consts::E.recip()).abs() < 1e-9);
    }

    #[test]
    fn recall_of_a_forgotten_fact_helps_most() {
        let base = Signal::new(at(0));
        let soon = base.reinforce(at(0)).stability_days;
        let late = base.reinforce(at(30)).stability_days;
        assert!(late > soon);
        // A recall at the moment of writing adds almost nothing.
        assert!((soon - INITIAL_STABILITY_DAYS).abs() < 1e-6);
        // A recall of a lost fact multiplies the stability by the full gain.
        let expected = (1.0 + STABILITY_GAIN) * INITIAL_STABILITY_DAYS;
        assert!((late - expected).abs() < 1e-3, "{late}");
    }

    #[test]
    fn repeated_recalls_hold_the_fact_longer() {
        let mut signal = Signal::new(at(0));
        for day in [2, 8, 30, 90] {
            signal = signal.reinforce(at(day));
        }
        assert_eq!(signal.recall_count, 4);
        // This is the calibration scenario of STABILITY_GAIN: four recalls
        // over three months lift the stability from one day to about four
        // months.
        assert!(
            (signal.stability_days - 120.0).abs() < 5.0,
            "{}",
            signal.stability_days
        );

        // The fact that the reader came back to keeps a much higher strength
        // than a fact of the same age that nobody read again.
        let never_recalled = Signal::new(at(90));
        let later = at(90 + 60);
        assert!(signal.strength_at(later) > 100.0 * never_recalled.strength_at(later));
        // Two months after the last recall the fact is still more than half
        // as strong as it was.
        assert!(signal.strength_at(later) > 0.6);
    }

    #[test]
    fn stability_has_a_ceiling() {
        let mut signal = Signal::new(at(0));
        for year in 1..50 {
            signal = signal.reinforce(at(year * 400));
        }
        assert!(signal.stability_days <= MAX_STABILITY_DAYS);
    }

    #[test]
    fn the_clock_never_runs_backwards() {
        let signal = Signal::new(at(10));
        assert!((signal.strength_at(at(0)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reports_when_the_fact_fades() {
        let signal = Signal::new(at(0));
        let half = signal.decays_to(0.5).unwrap();
        // The half life of an exponential decay is ln(2) stability units.
        let expected = at(0) + Duration::seconds((2f64.ln() * 86_400.0).round() as i64);
        assert_eq!(half, expected);
        assert_eq!(signal.decays_to(0.0), None);
    }

    #[test]
    fn parses_the_order_method() {
        assert_eq!("date".parse::<OrderBy>().unwrap(), OrderBy::Date);
        assert_eq!("SIGNAL".parse::<OrderBy>().unwrap(), OrderBy::Signal);
        assert!("ease".parse::<OrderBy>().is_err());
    }
}
