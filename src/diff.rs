use crate::nightly::Nightly;
use crate::repo::get_agent_repo_path;
use anyhow::Result;
use chrono::{Datelike, Weekday};
use colored::Colorize;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, warn};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Regex to identify PR references like "(#12345)" in commit messages
static PR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(#(?P<num>\d+)\)").unwrap());

/// Struct to deserialize the release.json file from datadog-agent repository
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ReleaseJson {
    #[serde(rename = "base_branch")]
    base_branch: Option<String>,
    #[serde(rename = "current_milestone")]
    current_milestone: Option<String>,
    dependencies: HashMap<String, String>,
    #[serde(rename = "last_stable")]
    last_stable: Option<HashMap<String, String>>,
}

/// Status of a component between two nightlies
#[derive(Debug, Clone, PartialEq)]
enum ComponentStatus {
    Same,
    Updated,
    New,
    Removed,
}

/// Represents a component version comparison
#[derive(Debug, Clone)]
struct ComponentDiff {
    name: String,
    base_version: Option<String>,
    comparison_version: Option<String>,
    status: ComponentStatus,
}

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

/// Fetch release.json from a specific commit SHA
async fn get_release_json(sha: &str, repo_path: PathBuf) -> Result<ReleaseJson> {
    let output = git_command(&["show", &format!("{}:release.json", sha)], repo_path).await?;
    let release_json: ReleaseJson = serde_json::from_str(&output)
        .map_err(|e| anyhow::anyhow!("Failed to parse release.json from SHA {}: {}", sha, e))?;
    Ok(release_json)
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

/// Compare component versions between two release.json files
async fn compare_components(
    older_sha: &str,
    newer_sha: &str,
    repo_path: PathBuf,
) -> Result<Vec<ComponentDiff>> {
    let older_release = get_release_json(older_sha, repo_path.clone()).await?;
    let newer_release = get_release_json(newer_sha, repo_path).await?;

    let mut diffs = Vec::new();
    let mut all_components = std::collections::HashSet::new();

    // Collect all component names from both releases
    for name in older_release.dependencies.keys() {
        all_components.insert(name.clone());
    }
    for name in newer_release.dependencies.keys() {
        all_components.insert(name.clone());
    }

    // Compare each component
    for name in all_components {
        let older_version = older_release.dependencies.get(&name).cloned();
        let newer_version = newer_release.dependencies.get(&name).cloned();

        let status = match (&older_version, &newer_version) {
            (Some(old), Some(new)) => {
                if old == new {
                    ComponentStatus::Same
                } else {
                    ComponentStatus::Updated
                }
            }
            (None, Some(_)) => ComponentStatus::New,
            (Some(_), None) => ComponentStatus::Removed,
            (None, None) => continue, // Shouldn't happen but skip if it does
        };

        diffs.push(ComponentDiff {
            name,
            base_version: older_version,
            comparison_version: newer_version,
            status,
        });
    }

    // Sort by component name for consistent output
    diffs.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(diffs)
}

/// Add component version differences to a report string
fn add_component_diff_to_report(report: &mut String, component_diffs: &[ComponentDiff]) -> Result<()> {
    if component_diffs.is_empty() {
        writeln!(report, "│ No component version changes found.")?;
        return Ok(());
    }

    writeln!(report, "│")?;
    writeln!(report, "│ 🔧 Component version changes:")?;

    for diff in component_diffs {
        match diff.status {
            ComponentStatus::Same => {
                // Skip displaying unchanged components for cleaner output
            }
            ComponentStatus::Updated => {
                let old_version = diff.base_version.as_deref().unwrap_or("unknown");
                let new_version = diff.comparison_version.as_deref().unwrap_or("unknown");
                writeln!(
                    report,
                    "│   {} {} → {}",
                    diff.name,
                    old_version,
                    new_version
                )?;
            }
            ComponentStatus::New => {
                let new_version = diff.comparison_version.as_deref().unwrap_or("unknown");
                writeln!(
                    report,
                    "│   {} added {}",
                    diff.name,
                    new_version
                )?;
            }
            ComponentStatus::Removed => {
                let old_version = diff.base_version.as_deref().unwrap_or("unknown");
                writeln!(
                    report,
                    "│   {} removed {}",
                    diff.name,
                    old_version
                )?;
            }
        }
    }
    Ok(())
}

/// Internal function to generate a diff report between two SHAs
async fn generate_diff_report(
    older_sha: &str,
    newer_sha: &str,
    older_name: &str,
    newer_name: &str,
) -> Result<String> {
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

    // Build report string
    let mut report = String::new();
    
    writeln!(report, "┌─ Diff between {} and {}", newer_name, older_name)?;

    let commit_lines: Vec<&str> = commits_output.lines().collect();
    writeln!(report, "│ {} commits:", commit_lines.len())?;

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

        // Fetch commit stats
        match get_commit_stats(sha_token, repo_path.clone()).await {
            Ok((ins, del)) => {
                if let Some(link) = pr_link_opt.as_deref() {
                    writeln!(report, "│   {} {} {} (+{}, -{})", sha_short, message_part, link, ins, del)?;
                } else {
                    writeln!(report, "│   {} {} (+{}, -{})", sha_short, message_part, ins, del)?;
                }
            }
            Err(_) => {
                // Fallback to original (non-colored) line
                if let Some(link) = pr_link_opt.as_deref() {
                    writeln!(report, "│   {} {} {}", sha_short, message_part, link)?;
                } else {
                    writeln!(report, "│   {} {}", sha_short, message_part)?;
                }
            }
        }
    }

    // Add component version comparison
    match compare_components(older_sha, newer_sha, repo_path.clone()).await {
        Ok(component_diffs) => {
            add_component_diff_to_report(&mut report, &component_diffs)?;
        }
        Err(e) => {
            warn!("Failed to compare component versions: {}", e);
            writeln!(report, "│")?;
            writeln!(report, "│ ⚠️ Component version comparison failed: {}", e)?;
        }
    }

    writeln!(report, "│\n│ File summary:")?;

    let mut binary_count = 0u32;
    for line in stat_output.lines() {
        // Split line on '|' to isolate stats section, if present
        if let Some((_, stats_part)) = line.split_once('|') {
            if stats_part.trim_start().starts_with("Bin") {
                binary_count += 1;
                continue;
            }
        }

        writeln!(report, "│   {}", line)?;
    }

    if binary_count > 0 {
        writeln!(report, "│   ({} binary files changed)", binary_count)?;
    }

    writeln!(report, "└─────────────────────────────────────")?;

    Ok(report)
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

    // Generate the report
    let report = generate_diff_report(&older.sha, &newer.sha, &older.tag.name, &newer.tag.name).await?;
    print!("{}", report);

    // Generate the full diff
    let repo_path = get_agent_repo_path()?;
    let full_diff = git_command(&["diff", &older.sha, &newer.sha], repo_path).await?;
    
    let line_count = full_diff.lines().count();
    
    // Use short SHAs for file names
    let older_name = &older.sha[..7];
    let newer_name = &newer.sha[..7];
    
    // Save report to tmp file
    let report_path = format!("/tmp/nightlies_report_{}_{}.txt", older_name, newer_name);
    std::fs::write(&report_path, &report)?;
    
    // Save patch to tmp file
    let mut patch_content = String::new();
    writeln!(patch_content, "# Diff between {} and {}", newer.tag.name, older.tag.name)?;
    writeln!(patch_content, "# Generated on {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))?;
    writeln!(patch_content, "# Lines: {}", line_count)?;
    writeln!(patch_content)?;
    patch_content.push_str(&full_diff);
    
    let patch_path = format!("/tmp/nightlies_diff_{}_{}.patch", older_name, newer_name);
    std::fs::write(&patch_path, &patch_content)?;
    
    println!("\n{}", format!("Report saved to: {}", report_path).cyan());
    println!("{}", format!("Patch saved to: {}", patch_path).cyan());
    
    Ok(())
}

/// Show a diff between two specific SHAs
///
/// # Errors
/// Returns an error if:
/// - Git commands fail to execute
/// - Repository path cannot be found
/// - File operations fail when storing diffs
pub async fn show_diff_between_shas(older_sha: String, newer_sha: String) -> Result<()> {
    // For SHA-based diffs, use the short SHA as the display name
    let older_name = &older_sha[..7];
    let newer_name = &newer_sha[..7];

    // Generate the report
    let report = generate_diff_report(&older_sha, &newer_sha, older_name, newer_name).await?;
    print!("{}", report);

    // Generate the full diff
    let repo_path = get_agent_repo_path()?;
    let full_diff = git_command(&["diff", &older_sha, &newer_sha], repo_path).await?;
    
    let line_count = full_diff.lines().count();
    
    // Save report to tmp file
    let report_path = format!("/tmp/nightlies_report_{}_{}.txt", older_name, newer_name);
    std::fs::write(&report_path, &report)?;
    
    // Save patch to tmp file
    let mut patch_content = String::new();
    writeln!(patch_content, "# Diff between {} and {}", newer_name, older_name)?;
    writeln!(patch_content, "# Generated on {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))?;
    writeln!(patch_content, "# Lines: {}", line_count)?;
    writeln!(patch_content)?;
    patch_content.push_str(&full_diff);
    
    let patch_path = format!("/tmp/nightlies_diff_{}_{}.patch", older_name, newer_name);
    std::fs::write(&patch_path, &patch_content)?;
    
    println!("\n{}", format!("Report saved to: {}", report_path).cyan());
    println!("{}", format!("Patch saved to: {}", patch_path).cyan());
    
    // Show the diff in a pager
    println!("\n{}", "Opening full diff in pager...".green());
    let _ = Command::new("less")
        .arg(&patch_path)
        .status()
        .await;
    
    Ok(())
}
