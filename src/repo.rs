use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use chrono::{DateTime, Utc};

use gix::{Commit, Id, Repository};
use tracing::{debug, info, warn};

use crate::{nightly::Nightly, NightlyError};

// Cache expiration time for git fetch operations (5 minutes)
const FETCH_CACHE_EXPIRATION: Duration = Duration::from_secs(5 * 60);

fn get_agent_repo_path() -> Result<PathBuf> {
    let home = match home::home_dir() {
        Some(path) if !path.as_os_str().is_empty() => Some(path),
        _ => None,
    };
    let home = home
        .ok_or_else(|| NightlyError::GenericError(String::from("Could not find home directory")))?;

    Ok(Path::new(&home).join("./go/src/github.com/DataDog/datadog-agent"))
}

fn open_git_repo() -> Result<Repository> {
    let repo_path = get_agent_repo_path()?;
    debug!("Opening Git repository at {}", repo_path.display());

    let repo = gix::open(repo_path)?;

    Ok(repo)
}

/// Open and fetch the git repo only if needed
fn open_git_repo_with_fetch(no_fetch: bool, force_fetch: bool) -> Result<Repository> {
    let repo = open_git_repo()?;

    if no_fetch {
        debug!("Skipping fetch due to no_fetch flag");
        return Ok(repo);
    }

    // Check if we need to fetch from remote or if it's forced
    if force_fetch || should_fetch(&repo) {
        // Fetch from default remote (usually 'origin')
        match fetch_default_remote(&repo) {
            Ok(()) => info!("Successfully fetched from default remote"),
            Err(e) => warn!("Failed to fetch from default remote: {}", e),
        }
    } else {
        debug!("Skipping fetch as it was recently performed");
    }

    Ok(repo)
}

// Check if we should perform a fetch based on time since last fetch
fn should_fetch(repo: &Repository) -> bool {
    let fetch_marker_path = repo.path().join("FETCH_TIMESTAMP");

    // If the marker doesn't exist, we should fetch
    if !fetch_marker_path.exists() {
        return true;
    }

    // Read the last fetch time
    match fs::read_to_string(&fetch_marker_path) {
        Ok(timestamp_str) => {
            match timestamp_str.trim().parse::<u64>() {
                Ok(timestamp) => {
                    let last_fetch_time = SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp);
                    let now = SystemTime::now();

                    // Check if enough time has passed since the last fetch
                    match now.duration_since(last_fetch_time) {
                        Ok(elapsed) => elapsed > FETCH_CACHE_EXPIRATION,
                        Err(_) => true, // System time went backwards? Fetch to be safe
                    }
                }
                Err(_) => true, // Invalid timestamp format, fetch to be safe
            }
        }
        Err(_) => true, // Couldn't read file, fetch to be safe
    }
}

// Update the fetch timestamp after a successful fetch
fn update_fetch_timestamp(repo: &Repository) -> Result<()> {
    let fetch_marker_path = repo.path().join("FETCH_TIMESTAMP");

    // Get current timestamp
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("Failed to get system time: {}", e))?;

    // Write timestamp to file
    fs::write(fetch_marker_path, now.as_secs().to_string())
        .map_err(|e| anyhow::anyhow!("Failed to write fetch timestamp: {}", e))?;

    Ok(())
}

/// Fetch from the default remote (usually 'origin')
fn fetch_default_remote(repo: &Repository) -> Result<()> {
    // Find default remote for fetch direction
    debug!("Looking for default remote for fetch operation");
    let remote_name =
        if let Some(remote_result) = repo.find_default_remote(gix::remote::Direction::Fetch) {
            let remote = remote_result?;
            let name = remote
                .name()
                .map_or_else(|| "<unnamed>".to_string(), |n| n.as_bstr().to_string());

            let url = remote
                .url(gix::remote::Direction::Fetch)
                .map_or_else(|| "<unknown>".to_string(), |u| u.to_bstring().to_string());

            debug!("Found default remote '{}' with URL '{}'", name, url);
            name
        } else {
            warn!("No default remote configured for fetch");
            return Err(anyhow::anyhow!("No default remote configured for fetch"));
        };

    // Configure fetch operation and execute it
    info!("Fetching updates from remote {}", remote_name);

    // Execute a git fetch command via gix
    debug!(
        "Starting git fetch operation from repo path: {}",
        repo.path().display()
    );

    // Create a command to run git fetch
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(repo.path())
        .arg("fetch")
        .arg("--quiet") // Suppress output unless there's an error
        .arg("--no-tags")
        .arg(&remote_name)
        .arg("refs/heads/main:refs/remotes/origin/main");

    debug!(
        "Executing command: git fetch --quiet --no-tags {} refs/heads/main:refs/remotes/origin/main",
        remote_name
    );

    // Execute the command
    let _output = match cmd.output() {
        Ok(output) => {
            if output.status.success() {
                // Only log output if there's actually something to log
                if !output.stdout.is_empty() {
                    debug!(
                        "Git fetch stdout: {}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                }

                if !output.stderr.is_empty() {
                    // Git often writes progress to stderr, so use debug level
                    debug!(
                        "Git fetch stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }

                info!("Git fetch completed successfully");

                // Update the fetch timestamp
                if let Err(e) = update_fetch_timestamp(repo) {
                    warn!("Failed to update fetch timestamp: {}", e);
                }

                output
            } else {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                warn!(
                    "Git fetch failed with status {}: {}",
                    output.status, error_msg
                );
                return Err(anyhow::anyhow!("Git fetch failed: {}", error_msg));
            }
        }
        Err(e) => {
            warn!("Failed to execute git fetch command: {}", e);
            return Err(anyhow::anyhow!(
                "Failed to execute git fetch command: {}",
                e
            ));
        }
    };

    debug!("Fetch completed successfully");
    Ok(())
}

/// Starting from the given branch, walk backwards until we find the commit with the given sha
fn get_commit_by_sha<'a>(
    repo: &'a Repository,
    sha: &str,
    branch_id: &Id,
) -> Result<Option<Commit<'a>>> {
    let commit_oid = match repo.rev_parse_single(sha) {
        Ok(obj) => obj,
        Err(e) => {
            warn!("Error finding sha: {}", e);
            return Ok(None);
        }
    };

    let revwalk = repo
        .rev_walk(Some(branch_id.detach()))
        .sorting(gix::traverse::commit::simple::Sorting::ByCommitTimeNewestFirst)
        .all()?
        .filter_map(Result::ok);

    //revwalk.push(branch.id())?;
    //revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    // revwalk will now walk backwards from the specified branch
    // until we find our target commit

    for rev in revwalk {
        let cm = rev.object()?;
        if cm.id() == commit_oid {
            return Ok(Some(cm));
        }
    }

    Ok(None)
}

fn print_friendly_git_may_be_stale_warning(target_sha: &str) {
    let git_path = get_agent_repo_path().expect("Could not find agent repo path");
    warn!(
        "Could not find the target commit: {} on 'main' of your datadog-agent checkout at {}",
        target_sha,
        git_path.display()
    );
    warn!(
        "Consider running 'git -C {} fetch --all --tags'",
        git_path.display()
    );
}

/// Given a sha that exists in the 'main' branch of the datadog-agent repo
/// return the timestamp of that commit
///
/// # Errors
/// - If the given sha is not found on the main branch
/// - If the git repo cannot be opened
/// - If the commit timestamp cannot be parsed
pub fn get_commit_timestamp(target_sha: &str) -> Result<DateTime<Utc>> {
    let repo = open_git_repo_with_fetch(false, false)?;
    let origin_main = repo
        .find_reference("refs/remotes/origin/main")?
        .into_fully_peeled_id()?;

    let commit = get_commit_by_sha(&repo, target_sha, &origin_main)?;
    let commit = commit.ok_or_else(|| {
        print_friendly_git_may_be_stale_warning(target_sha);
        NightlyError::GenericError(format!("commit '{target_sha}' not found on 'main'"))
    })?;

    let timestamp = DateTime::from_timestamp(commit.time()?.seconds, 0).ok_or(
        NightlyError::DateParseError(format!(
            "Couldn't use commit epoch value of {}",
            commit.time()?.seconds
        )),
    )?;

    Ok(timestamp)
}

/// Given a sha that exists in the 'main' branch of the datadog-agent repo, print
/// the first nightly build that contains that change
/// nightlies is assumed to be ordered from newest to oldest
///
/// # Errors
/// - If the given sha is not found on the main branch
/// - If no nightly is found containing the given sha
/// - If the git repo cannot be opened
pub fn get_first_nightly_containing_change(
    nightlies: &[Nightly],
    change_sha: &str,
    no_fetch: bool,
    force_fetch: bool,
) -> Result<Nightly> {
    let repo = open_git_repo_with_fetch(no_fetch, force_fetch)?;
    let origin_main = repo
        .find_reference("refs/remotes/origin/main")?
        .into_fully_peeled_id()?;

    // First check if the commit exists in main
    let commit = get_commit_by_sha(&repo, change_sha, &origin_main)?;
    let Some(commit_obj) = commit else {
        print_friendly_git_may_be_stale_warning(change_sha);
        anyhow::bail!("commit '{change_sha}' not found on 'main'");
    };

    // Get the commit timestamp
    let commit_timestamp = DateTime::from_timestamp(commit_obj.time()?.seconds, 0).ok_or(
        NightlyError::DateParseError(format!(
            "Couldn't use commit epoch value of {}",
            commit_obj.time()?.seconds
        )),
    )?;

    debug!(
        "Target commit {} has timestamp: {}",
        change_sha, commit_timestamp
    );

    // Filter and sort nightlies where build timestamp is after commit timestamp
    let mut candidate_nightlies: Vec<&Nightly> = nightlies
        .iter()
        .filter(|n| {
            // Get the timestamp for the nightly's SHA if available, otherwise use estimated_last_pushed
            if let Some(sha_timestamp) = n.sha_timestamp {
                // Only consider nightlies built after the commit was made
                sha_timestamp >= commit_timestamp
            } else {
                // If we don't know the SHA timestamp, use the estimated push time
                n.estimated_last_pushed >= commit_timestamp
            }
        })
        .collect();

    // Sort nightlies by timestamp (oldest first, so the first match is the earliest nightly)
    candidate_nightlies.sort_by(|a, b| {
        let a_time = a.sha_timestamp.unwrap_or(a.estimated_last_pushed);
        let b_time = b.sha_timestamp.unwrap_or(b.estimated_last_pushed);
        a_time.cmp(&b_time) // Ascending order (oldest first)
    });

    debug!(
        "Filtered to {} candidate nightlies with builds after the target commit",
        candidate_nightlies.len()
    );

    let mut containing_nightly: Option<Nightly> = None;

    debug!("Searching for nightly containing sha: {}", change_sha);

    // Only check the candidates
    for nightly in candidate_nightlies {
        debug!(
            "Checking if nightly-{} (timestamp: {}) contains the target sha",
            nightly.sha,
            nightly
                .sha_timestamp
                .unwrap_or(nightly.estimated_last_pushed)
        );

        // Parse nightly SHA and create an Id object that won't be dropped too early
        let nightly_obj = match repo.rev_parse_single(nightly.sha.as_str()) {
            Ok(obj) => obj,
            Err(e) => {
                warn!("Error finding nightly sha: {}", e);
                print_friendly_git_may_be_stale_warning(nightly.sha.as_str());
                continue;
            }
        };

        // Use the simple approach of walking the commit history
        if let Some(_commit) = get_commit_by_sha(&repo, change_sha, &nightly_obj)? {
            containing_nightly = Some(nightly.clone());
            debug!("Found target commit in nightly {}", nightly.sha);
            // Since we're sorted by oldest first, we can break at first match
            break;
        } else {
            debug!(
                "Didn't find commit: {} in nightly: {}",
                change_sha, nightly.sha
            );
        }
    }

    containing_nightly.ok_or_else(|| {
        anyhow::Error::msg(format!("No nightly found containing commit: {change_sha}"))
    })
}
