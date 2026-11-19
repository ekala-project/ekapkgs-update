//! Clear the failure backoff of failed updates so they are attempted again.
//!
//! For failures that were not the package's fault, such as a nix daemon in a
//! bad state: `run` skips packages in backoff and `watch` skips the same
//! failed version, so without this they would wait 2–6 days.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::commands::run::preservation::repo_from_label;
use crate::database::{Database, FailureResetFilter};

/// Options of the `reset-failures` command.
#[derive(Debug, Clone, Default)]
pub struct ResetFailuresOptions {
    pub database: String,
    pub repo: Option<String>,
    pub attr_paths: Vec<String>,
    pub sessions: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub error_match: Option<String>,
    pub all: bool,
    pub dry_run: bool,
}

/// Parse a time bound: `30m`/`6h`/`2d` (ago), `YYYY-MM-DD` (UTC midnight)
/// or an RFC 3339 timestamp.
pub fn parse_time_bound(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let value = value.trim();
    if let Some(unit) = value.chars().last() {
        let amount = &value[..value.len() - unit.len_utf8()];
        if let Ok(n) = amount.parse::<i64>() {
            let ago = match unit {
                'm' => Some(Duration::minutes(n)),
                'h' => Some(Duration::hours(n)),
                'd' => Some(Duration::days(n)),
                _ => None,
            };
            if let Some(ago) = ago {
                return Ok(now - ago);
            }
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        if let Some(midnight) = date.and_hms_opt(0, 0, 0) {
            return Ok(midnight.and_utc());
        }
    }
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .with_context(|| {
            format!("invalid time {value:?}: expected 30m/6h/2d, YYYY-MM-DD or RFC 3339")
        })
}

impl ResetFailuresOptions {
    fn filter(&self, now: DateTime<Utc>) -> Result<FailureResetFilter> {
        let filter = FailureResetFilter {
            // `default` selects the unnamed scope, as in the other commands.
            repo_name: self.repo.as_deref().map(|r| repo_from_label(r).to_owned()),
            attr_paths: self.attr_paths.clone(),
            session_ids: self.sessions.clone(),
            since: self
                .since
                .as_deref()
                .map(|s| parse_time_bound(s, now))
                .transpose()?,
            until: self
                .until
                .as_deref()
                .map(|s| parse_time_bound(s, now))
                .transpose()?,
            error_match: self.error_match.clone().filter(|m| !m.is_empty()),
        };
        let unfiltered = filter.repo_name.is_none()
            && filter.attr_paths.is_empty()
            && filter.session_ids.is_empty()
            && filter.since.is_none()
            && filter.until.is_none()
            && filter.error_match.is_none();
        if unfiltered && !self.all {
            bail!(
                "no filter given: pass --since, --session, --error-match, --repo or --attr, \
                 or --all to reset every failed update"
            );
        }
        Ok(filter)
    }
}

/// Clear the backoff of the matching failed updates (or list them).
pub async fn reset_failures(options: ResetFailuresOptions) -> Result<()> {
    let filter = options.filter(Utc::now())?;
    let expanded_path = shellexpand::tilde(&options.database).to_string();
    let db = Database::new(&expanded_path).await?;

    let matched = db.reset_failures(&filter, options.dry_run).await?;
    if matched.is_empty() {
        println!("No failed updates match.");
        return Ok(());
    }

    for row in &matched {
        let name = if row.repo_name.is_empty() {
            row.attr_path.clone()
        } else {
            format!("{}+{}", row.repo_name, row.attr_path)
        };
        let attempted = row.last_attempted.map_or_else(
            || "-".to_owned(),
            |t| t.format("%Y-%m-%d %H:%M").to_string(),
        );
        println!(
            "{name}  {}  attempted {attempted}  failures {}  {}",
            row.failed_version.as_deref().unwrap_or("-"),
            row.failure_count,
            row.last_error.as_deref().unwrap_or(""),
        );
    }

    let in_backoff = matched
        .iter()
        .filter(|r| r.next_attempt.is_some_and(|t| t > Utc::now()))
        .count();
    if options.dry_run {
        println!(
            "\n{} failed updates match ({in_backoff} in backoff). Dry run: nothing changed.",
            matched.len()
        );
    } else {
        println!(
            "\nReset {} failed updates ({in_backoff} were in backoff). They will be attempted \
             on the next run or watch poll.",
            matched.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_bounds() {
        let now = DateTime::parse_from_rfc3339("2026-11-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let cases = [
            ("30m", "2026-11-17T11:30:00+00:00"),
            ("6h", "2026-11-17T06:00:00+00:00"),
            ("2d", "2026-11-15T12:00:00+00:00"),
            ("2026-11-16", "2026-11-16T00:00:00+00:00"),
            ("2026-11-16T08:15:00+02:00", "2026-11-16T06:15:00+00:00"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_time_bound(input, now).unwrap().to_rfc3339(),
                expected,
                "{input}"
            );
        }
        for bad in ["", "yesterday", "5w", "2026-13-01"] {
            assert!(parse_time_bound(bad, now).is_err(), "{bad}");
        }
    }

    #[test]
    fn filter_required_unless_all() {
        let now = Utc::now();
        let none = ResetFailuresOptions::default();
        assert!(none.filter(now).is_err());
        let all = ResetFailuresOptions {
            all: true,
            ..Default::default()
        };
        assert!(all.filter(now).is_ok());
        for (label, expected) in [("default", ""), ("corepkgs", "corepkgs")] {
            let repo = ResetFailuresOptions {
                repo: Some(label.into()),
                ..Default::default()
            };
            assert_eq!(
                repo.filter(now).unwrap().repo_name.as_deref(),
                Some(expected)
            );
        }
        let since = ResetFailuresOptions {
            since: Some("1d".into()),
            ..Default::default()
        };
        assert!(since.filter(now).unwrap().since.is_some());
    }
}
