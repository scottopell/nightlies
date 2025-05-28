use crate::nightly::Nightly;
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc, Weekday};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};

/// Represents the direction of diff relative to selected nightly
pub enum DiffDirection {
    Previous,
    Next,
}

/// Returns true if the timestamp falls on a weekend (Saturday or Sunday in UTC)
fn is_weekend(ts: DateTime<Utc>) -> bool {
    let weekday = ts.weekday();
    weekday == Weekday::Sat || weekday == Weekday::Sun
}

/// Calculate the number of business days between two timestamps
fn business_days_between(start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    let days = (end - start).num_days();
    if days <= 0 {
        return days;
    }

    // Count weekends between the dates and subtract them
    let mut weekends = 0;
    let mut current = start;
    while current <= end {
        if is_weekend(current) {
            weekends += 1;
        }
        current += Duration::days(1);
    }
    days - weekends
}

/// Find the next consecutive nightly after the given timestamp
fn find_next_consecutive<'a>(
    nightlies: &'a [&'a Nightly],
    from_ts: DateTime<Utc>,
    skip_weekends: bool,
) -> Option<&'a Nightly> {
    nightlies
        .iter()
        .filter(|n| {
            let ts = n.sha_timestamp.unwrap_or(n.estimated_last_pushed);

            // Must be after from_ts
            if ts <= from_ts {
                return false;
            }

            if skip_weekends {
                // For weekday-only mode:
                // - If current nightly is on weekend, skip it
                // - Must be within 1 business day
                if is_weekend(ts) {
                    return false;
                }
                business_days_between(from_ts, ts) <= 1
            } else {
                // For all-days mode:
                // Must be within 1 calendar day
                (ts - from_ts).num_days() <= 1
            }
        })
        .copied()
        .min_by_key(|n| n.sha_timestamp.unwrap_or(n.estimated_last_pushed))
}

/// Find the previous consecutive nightly before the given timestamp
fn find_prev_consecutive<'a>(
    nightlies: &'a [&'a Nightly],
    from_ts: DateTime<Utc>,
    skip_weekends: bool,
) -> Option<&'a Nightly> {
    nightlies
        .iter()
        .filter(|n| {
            let ts = n.sha_timestamp.unwrap_or(n.estimated_last_pushed);

            // Must be before from_ts
            if ts >= from_ts {
                return false;
            }

            if skip_weekends {
                // For weekday-only mode:
                // - If current nightly is on weekend, skip it
                // - Must be within 1 business day
                if is_weekend(ts) {
                    return false;
                }
                business_days_between(ts, from_ts) <= 1
            } else {
                // For all-days mode:
                // Must be within 1 calendar day
                (from_ts - ts).num_days() <= 1
            }
        })
        .copied()
        .max_by_key(|n| n.sha_timestamp.unwrap_or(n.estimated_last_pushed))
}

/// Format a nightly for display in the selection menu
fn format_nightly_for_display(nightly: &Nightly) -> String {
    let ts = nightly
        .sha_timestamp
        .unwrap_or(nightly.estimated_last_pushed);
    format!(
        "{} ({})",
        nightly.tag.name.green(),
        ts.format("%Y-%m-%d %H:%M UTC").to_string().cyan()
    )
}

/// Interactively select a nightly and diff direction
///
/// # Errors
/// Returns an error if:
/// - User interaction fails
/// - No consecutive nightlies are available to compare
pub fn select_nightlies_to_diff(
    nightlies: &[Nightly],
    skip_weekends: bool,
) -> Result<(String, String)> {
    let theme = ColorfulTheme::default();

    // Create a sorted list of nightlies (newest first)
    let mut nightly_refs: Vec<&Nightly> = nightlies.iter().collect();
    nightly_refs
        .sort_by_key(|n| std::cmp::Reverse(n.sha_timestamp.unwrap_or(n.estimated_last_pushed)));

    // Filter weekends if requested
    let filtered: Vec<&Nightly> = if skip_weekends {
        nightly_refs
            .into_iter()
            .filter(|n| !n.is_weekend_build())
            .collect()
    } else {
        nightly_refs
    };

    // Step 1: Select the base nightly
    let nightly_options: Vec<String> = filtered
        .iter()
        .map(|n| format_nightly_for_display(n))
        .collect();

    let selected = Select::with_theme(&theme)
        .with_prompt("Select a nightly to compare")
        .items(&nightly_options)
        .default(0)
        .interact()?;

    let base_nightly = filtered[selected];
    let base_ts = base_nightly
        .sha_timestamp
        .unwrap_or(base_nightly.estimated_last_pushed);

    // Step 2: Determine available directions
    let has_prev = find_prev_consecutive(&filtered, base_ts, skip_weekends).is_some();
    let has_next = find_next_consecutive(&filtered, base_ts, skip_weekends).is_some();

    let mut direction_options = Vec::new();
    if has_prev {
        direction_options.push("Compare with previous nightly");
    }
    if has_next {
        direction_options.push("Compare with next nightly");
    }

    if direction_options.is_empty() {
        anyhow::bail!("No consecutive nightlies available to compare with");
    }

    let direction = Select::with_theme(&theme)
        .with_prompt("Select comparison direction")
        .items(&direction_options)
        .default(0)
        .interact()?;

    let other_nightly = if direction_options[direction].contains("previous") {
        find_prev_consecutive(&filtered, base_ts, skip_weekends)
            .ok_or_else(|| anyhow::anyhow!("Failed to find previous consecutive nightly"))?
    } else {
        find_next_consecutive(&filtered, base_ts, skip_weekends)
            .ok_or_else(|| anyhow::anyhow!("Failed to find next consecutive nightly"))?
    };

    // Return the two nightlies in chronological order (older first)
    if base_nightly
        .sha_timestamp
        .unwrap_or(base_nightly.estimated_last_pushed)
        > other_nightly
            .sha_timestamp
            .unwrap_or(other_nightly.estimated_last_pushed)
    {
        Ok((other_nightly.sha.clone(), base_nightly.sha.clone()))
    } else {
        Ok((base_nightly.sha.clone(), other_nightly.sha.clone()))
    }
}
