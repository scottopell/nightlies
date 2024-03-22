use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use git2::{Commit, Error, Repository};
use tracing::{debug, warn};

use crate::{nightly::Nightly, NightlyError};

fn get_agent_repo_path() -> Result<PathBuf, NightlyError> {
    let home = match home::home_dir() {
        Some(path) if !path.as_os_str().is_empty() => Some(path),
        _ => None,
    };
    let home = home.ok_or_else(|| Error::from_str("Could not find home directory"))?;

    Ok(Path::new(&home).join("./go/src/github.com/DataDog/datadog-agent"))
}

fn open_git_repo() -> Result<Repository, NightlyError> {
    Repository::open(get_agent_repo_path()?).map_err(NightlyError::GitError)
}

/// Starting from the given branch, walk backwards until we find the commit with the given sha
fn get_commit_by_sha<'a>(
    repo: &'a Repository,
    sha: &'a str,
    branch: &Commit,
) -> Result<Option<Commit<'a>>, Error> {
    let branch_time =
        DateTime::from_timestamp(branch.time().seconds(), 0).expect("git date invalid");
    debug!(
        "Searching for commit: {} starting from {:?} (timestamp: {})",
        sha,
        branch.id(),
        branch_time
    );
    let commit_oid = match repo.revparse_single(sha) {
        Ok(obj) => obj.id(),
        Err(e) => {
            warn!("Error finding sha: {}", e);
            return Ok(None);
        }
    };

    let mut revwalk = repo.revwalk()?;
    revwalk.push(branch.id())?;
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    // revwalk will now walk backwards from the specified branch
    // until we find our target commit

    for rev in revwalk {
        let rev = rev?;
        if rev == commit_oid {
            debug!("Found commit: {:?}", rev);
            let commit = repo.find_commit(rev)?;
            return Ok(Some(commit));
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
pub fn get_commit_timestamp(target_sha: &str) -> Result<DateTime<Utc>, NightlyError> {
    let repo = open_git_repo()?;
    let origin_main = repo
        .find_reference("refs/remotes/origin/main")?
        .peel_to_commit()?;

    let commit = get_commit_by_sha(&repo, target_sha, &origin_main)?;
    let commit = commit.ok_or_else(|| {
        print_friendly_git_may_be_stale_warning(target_sha);
        NightlyError::GenericError(format!("commit '{target_sha}' not found on 'main'"))
    })?;

    let timestamp = DateTime::from_timestamp(commit.time().seconds(), 0).ok_or(
        NightlyError::DateParseError(format!(
            "Couldn't use commit epoch value of {}",
            commit.time().seconds()
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
) -> Result<Nightly, NightlyError> {
    let repo = open_git_repo()?;
    let origin_main = repo
        .find_reference("refs/remotes/origin/main")?
        .peel_to_commit()?;

    let commit = get_commit_by_sha(&repo, change_sha, &origin_main)?;
    let Some(_commit) = commit else {
        print_friendly_git_may_be_stale_warning(change_sha);
        return Err(NightlyError::GenericError(format!(
            "commit '{change_sha}' not found on 'main'"
        )));
    };

    let mut containing_nightly: Option<Nightly> = None;

    debug!("Searching for nightly containing sha: {}", change_sha);
    for nightly in nightlies {
        debug!(
            "Checking if nightly-{} (last pushed: {}) contains the target sha",
            nightly.sha, nightly.estimated_last_pushed
        );

        // I may be able to simplify all this by using repo.graph_descendant_of() instead of calling get_commit_by_sha
        // I think these two do roughly the same thing
        let current_nightly_head_object = match repo.revparse_single(nightly.sha.as_str()) {
            Ok(obj) => obj,
            Err(e) => {
                warn!("Error finding nightly sha: {}", e);
                print_friendly_git_may_be_stale_warning(nightly.sha.as_str());
                continue;
            }
        };
        let current_nightly_head_commit = repo.find_commit(current_nightly_head_object.id())?;
        if let Some(_commit) = get_commit_by_sha(&repo, change_sha, &current_nightly_head_commit)? {
            containing_nightly = Some(nightly.clone());
        } else {
            debug!(
                "Didn't find commit: {} in nightly: {}",
                change_sha, nightly.sha
            );
        }
    }

    containing_nightly.ok_or_else(|| {
        NightlyError::GenericError(format!("No nightly found containing commit: {change_sha}"))
    })
}
