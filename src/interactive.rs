use crate::nightly::Nightly;
use anyhow::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};


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

/// Format a nightly for display with a visual indicator if it's selected
fn format_nightly_for_display_with_indicator(nightly: &Nightly, is_selected: bool) -> String {
    let ts = nightly
        .sha_timestamp
        .unwrap_or(nightly.estimated_last_pushed);
    let base_format = format!(
        "{} ({})",
        nightly.tag.name.green(),
        ts.format("%Y-%m-%d %H:%M UTC").to_string().cyan()
    );
    
    if is_selected {
        format!("{} {}", base_format, "[SELECTED]".yellow())
    } else {
        base_format
    }
}

/// Check if a nightly is within the 1-month time distance from the selected nightly
fn is_within_month_distance(nightly: &Nightly, selected_nightly: &Nightly) -> bool {
    let nightly_ts = nightly.sha_timestamp.unwrap_or(nightly.estimated_last_pushed);
    let selected_ts = selected_nightly.sha_timestamp.unwrap_or(selected_nightly.estimated_last_pushed);
    
    let duration = if nightly_ts > selected_ts {
        nightly_ts - selected_ts
    } else {
        selected_ts - nightly_ts
    };
    
    duration.num_days() <= 30
}

/// Interactively select two nightlies to compare
///
/// # Errors
/// Returns an error if:
/// - User interaction fails
/// - No valid nightlies are available to compare
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

    if filtered.len() < 2 {
        anyhow::bail!("Need at least 2 nightlies to compare");
    }

    // Step 1: Select the first nightly
    let nightly_options: Vec<String> = filtered
        .iter()
        .map(|n| format_nightly_for_display(n))
        .collect();

    let first_selected = Select::with_theme(&theme)
        .with_prompt("Select first nightly to compare")
        .items(&nightly_options)
        .default(0)
        .interact()?;

    let first_nightly = filtered[first_selected];

    // Step 2: Select the second nightly with invalid options disabled
    let second_nightly_options: Vec<String> = filtered
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let is_selected = i == first_selected;
            let is_valid = i != first_selected && is_within_month_distance(n, first_nightly);
            
            if is_valid {
                format_nightly_for_display_with_indicator(n, is_selected)
            } else if is_selected {
                format!("{} {}", format_nightly_for_display(n), "[SELECTED]".yellow())
            } else {
                format!("{} {}", format_nightly_for_display(n), "[INVALID]".red())
            }
        })
        .collect();

    // Create a list of valid indices for the second selection
    let valid_indices: Vec<usize> = filtered
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            if i != first_selected && is_within_month_distance(n, first_nightly) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    if valid_indices.is_empty() {
        anyhow::bail!("No valid nightlies available to compare with (must be within 1 month)");
    }

    let second_selected_index = Select::with_theme(&theme)
        .with_prompt("Select second nightly to compare")
        .items(&second_nightly_options)
        .default(valid_indices[0])
        .interact()?;

    // Validate the selection
    if !valid_indices.contains(&second_selected_index) {
        anyhow::bail!("Invalid selection - must choose a valid nightly");
    }

    let second_nightly = filtered[second_selected_index];

    // Return the two nightlies in chronological order (older first)
    let first_ts = first_nightly.sha_timestamp.unwrap_or(first_nightly.estimated_last_pushed);
    let second_ts = second_nightly.sha_timestamp.unwrap_or(second_nightly.estimated_last_pushed);

    if first_ts > second_ts {
        Ok((second_nightly.sha.clone(), first_nightly.sha.clone()))
    } else {
        Ok((first_nightly.sha.clone(), second_nightly.sha.clone()))
    }
}
