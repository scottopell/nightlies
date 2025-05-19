use crate::nightly::Nightly;
use crate::repo::get_agent_repo_path;
use anyhow::Result;
use chrono::{Datelike, Weekday};
use colored::*;
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, warn};

/// Returns true if the given timestamp is a Saturday or Sunday (UTC).
fn is_weekend(ts: &chrono::DateTime<chrono::Utc>) -> bool {
    let weekday = ts.weekday();
    weekday == Weekday::Sat || weekday == Weekday::Sun
}

/// Run a git command in the agent repository and return stdout as a UTF-8 string.
async fn git_command(args: &[&str], repo_path: PathBuf) -> Result<String> {
    debug!(?args, "Running git command");
    let output = Command::new("git")
        .current_dir(&repo_path)
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        warn!(
            status = ?output.status,
            stderr = %String::from_utf8_lossy(&output.stderr),
            "git command failed"
        );
        anyhow::bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Extract insertions and deletions for a commit
async fn get_commit_stats(sha: &str, repo_path: PathBuf) -> Result<(u32, u32)> {
    // Run git show with shortstat and empty format to only get stats lines
    let output = git_command(&["show", "--shortstat", "--format=", sha], repo_path).await?;

    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?P<ins>\d+) insertion[\w\(\)\+]*,?\s*(?P<del>\d+) deletion").unwrap()
    });

    for line in output.lines() {
        if let Some(caps) = RE.captures(line) {
            let ins: u32 = caps["ins"].parse().unwrap_or(0);
            let del: u32 = caps["del"].parse().unwrap_or(0);
            return Ok((ins, del));
        }
    }

    // If no stats found, return zeros
    Ok((0, 0))
}

/// Show a concise source diff between the two most-recent nightlies (respecting weekend filter).
///
/// 1. Chooses the latest two nightlies after applying the `include_weekends` rule.
/// 2. Prints commit list, file summary and short per-file diffs.
pub async fn show_diff_between_latest_two(
    nightlies: &[Nightly],
    include_weekends: bool,
) -> Result<()> {
    // Filter weekend builds if requested
    let mut filtered: Vec<&Nightly> = nightlies
        .iter()
        .filter(|n| include_weekends || !is_weekend(&n.estimated_last_pushed))
        .collect();

    // Sort newest first using SHA timestamp when available
    filtered.sort_by_key(|n| std::cmp::Reverse(n.sha_timestamp.unwrap_or(n.estimated_last_pushed)));

    if filtered.len() < 2 {
        anyhow::bail!("Need at least two nightlies to compute a diff (after filtering)");
    }

    let newer = filtered[0];
    let older = filtered[1];

    let repo_path = get_agent_repo_path()?;

    // Run git commands sequentially (diff generation is fast enough)
    let log_range = format!("{}..{}", older.sha, newer.sha);

    let commits_output = git_command(
        &["log", "--oneline", "--no-merges", &log_range],
        repo_path.clone(),
    )
    .await?;

    let stat_output = git_command(
        &["diff", "--stat", older.sha.as_str(), newer.sha.as_str()],
        repo_path.clone(),
    )
    .await?;

    // We fetch only commit log and statistics; file-level patches are omitted per user preference

    // Print final report
    println!(
        "{}",
        format!(
            "┌─ Diff between {} and {}",
            newer.tag.name.green(),
            older.tag.name.green()
        )
        .bold()
    );

    let commit_lines: Vec<&str> = commits_output.lines().collect();
    println!("│ {} commits:", commit_lines.len());
    for line in commit_lines.iter().take(25) {
        let sha = line.split_whitespace().next().unwrap_or("");
        match get_commit_stats(sha, repo_path.clone()).await {
            Ok((ins, del)) => {
                println!("│   {} (+{}, -{})", line, ins, del);
            }
            Err(_) => {
                // Fallback to original line without stats
                println!("│   {}", line);
            }
        }
    }
    if commit_lines.len() > 25 {
        println!("│   …");
    }

    println!("│\n│ File summary:");

    let mut binary_count = 0u32;
    for line in stat_output.lines() {
        // Split line on '|' to isolate stats section, if present
        if let Some((_, stats_part)) = line.split_once('|') {
            if stats_part.trim_start().starts_with("Bin") {
                binary_count += 1;
                continue;
            }
        }

        println!("│   {}", line);
    }

    if binary_count > 0 {
        println!("│   ({} binary files changed)", binary_count);
    }

    println!("└─────────────────────────────────────");

    Ok(())
}
