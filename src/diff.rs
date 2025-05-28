use crate::nightly::Nightly;
use crate::repo::get_agent_repo_path;
use anyhow::Result;
use chrono::{Datelike, Weekday};
use colored::Colorize;
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, warn};

/// Regex to identify PR references like "(#12345)" in commit messages
static PR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(#(?P<num>\d+)\)").unwrap());

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
    // Separate regexes for insertion and deletion counts (handles singular/plural)
    static INS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<num>\d+) insertion(?:s)?").unwrap());
    static DEL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<num>\d+) deletion(?:s)?").unwrap());

    // Run git show with shortstat and empty format to only get stats lines
    let output = git_command(&["show", "--shortstat", "--format=", sha], repo_path).await?;

    for line in output.lines() {
        let ins: u32 = INS_RE
            .captures(line)
            .and_then(|c| c.name("num"))
            .map_or(0, |m| m.as_str().parse().unwrap_or(0));

        let del: u32 = DEL_RE
            .captures(line)
            .and_then(|c| c.name("num"))
            .map_or(0, |m| m.as_str().parse().unwrap_or(0));

        if ins > 0 || del > 0 {
            return Ok((ins, del));
        }
    }

    // If no stats found, return zeros
    Ok((0, 0))
}

/// Internal function to display a diff between two SHAs with consistent formatting
async fn display_diff(
    older_sha: &str,
    newer_sha: &str,
    older_name: &str,
    newer_name: &str,
) -> Result<()> {
    let repo_path = get_agent_repo_path()?;

    // Run git commands sequentially (diff generation is fast enough)
    let log_range = format!("{}..{}", older_sha, newer_sha);

    let commits_output = git_command(
        &["log", "--oneline", "--no-merges", &log_range],
        repo_path.clone(),
    )
    .await?;

    let stat_output =
        git_command(&["diff", "--stat", older_sha, newer_sha], repo_path.clone()).await?;

    // Print final report
    println!(
        "{}",
        format!(
            "┌─ Diff between {} and {}",
            newer_name.green(),
            older_name.green()
        )
        .bold()
    );

    let commit_lines: Vec<&str> = commits_output.lines().collect();
    println!("│ {} commits:", commit_lines.len());

    for line in &commit_lines {
        // First token is the SHA
        let _sha = line.split_whitespace().next().unwrap_or("");

        // Build commit line, removing the "(#1234)" fragment if present
        let mut base_line = PR_RE.replace(line, "").to_string();
        base_line = base_line.trim_end().to_string();

        // Extract pr link (if present) from original line
        let pr_link_opt = PR_RE.captures(line).map(|caps| {
            format!(
                "https://github.com/DataDog/datadog-agent/pull/{}",
                &caps["num"]
            )
        });

        // Split into SHA and message part
        let (sha_token, message_part) = base_line
            .split_once(' ')
            .map_or((base_line.as_str(), ""), |(s, rest)| (s, rest.trim()));

        // Short SHA (7 chars for aesthetics)
        let sha_short = if sha_token.len() > 7 {
            &sha_token[..7]
        } else {
            sha_token
        };
        let sha_colored = sha_short.cyan();

        // Colored link if present
        let link_colored = pr_link_opt
            .as_deref()
            .map(|l| l.blue().underline().to_string())
            .unwrap_or_default();

        // Fetch commit stats
        match get_commit_stats(sha_token, repo_path.clone()).await {
            Ok((ins, del)) => {
                let plus = format!("+{ins}").green();
                let minus = format!("-{del}").red();

                if link_colored.is_empty() {
                    println!("│   {sha_colored} {message_part} ({plus}, {minus})");
                } else {
                    println!("│   {sha_colored} {message_part} {link_colored} ({plus}, {minus})");
                }
            }
            Err(_) => {
                // Fallback to original (non-colored) line
                if link_colored.is_empty() {
                    println!("│   {sha_colored} {message_part}");
                } else {
                    println!("│   {sha_colored} {message_part} {link_colored}");
                }
            }
        }
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

        println!("│   {line}");
    }

    if binary_count > 0 {
        println!("│   ({binary_count} binary files changed)");
    }

    println!("└─────────────────────────────────────");

    Ok(())
}

/// Show a concise source diff between the two most-recent nightlies (respecting weekend filter).
///
/// 1. Chooses the latest two nightlies after applying the `include_weekends` rule.
/// 2. Prints commit list, file summary and short per-file diffs.
///
/// # Errors
///
/// This function will return an error if:
/// - There are fewer than two nightlies after filtering
/// - Git commands fail to execute
/// - Repository path cannot be found
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

    display_diff(&older.sha, &newer.sha, &older.tag.name, &newer.tag.name).await
}

/// Show a diff between two specific SHAs
pub async fn show_diff_between_shas(older_sha: String, newer_sha: String) -> Result<()> {
    // For SHA-based diffs, use the short SHA as the display name
    let older_name = &older_sha[..7];
    let newer_name = &newer_sha[..7];

    display_diff(&older_sha, &newer_sha, older_name, newer_name).await
}
