use crate::{repo::get_commit_timestamp, NightlyError};
use chrono::{DateTime, Datelike, Utc, Weekday};
use colored::Colorize;
use once_cell::sync::Lazy;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::{debug, info, trace, warn};

// Updated URL for nightly-full tags
const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Tag {
    pub name: String,
    #[serde(rename = "tag_last_pushed")]
    pub last_pushed: DateTime<Utc>,
    pub digest: String,
}

impl Tag {
    // Updated to extract SHA from nightly-full-main-SHA-jmx format
    #[must_use]
    pub fn get_sha(&self) -> Option<&str> {
        if self.name.starts_with("nightly-full-main-") && self.name.ends_with("-jmx") {
            if let Some(sha) = self.name.split('-').nth(3) {
                if sha.len() == 8 {
                    return Some(sha);
                }
            }
        }
        None
    }
}

// Simplified Nightly struct - only contains a single tag
#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Nightly {
    pub sha: String,
    pub estimated_last_pushed: DateTime<Utc>,
    pub sha_timestamp: Option<DateTime<Utc>>,
    pub tag: Tag,
}

static CACHE_FILE: Lazy<PathBuf> = Lazy::new(|| {
    // get a 'stable' temp dir that can be used to cache the results from previous runs
    let dir = std::env::temp_dir();
    dir.join("agent_nightlies.json")
});

pub fn find_nightly_by_sha<'a, 'b>(nightlies: &'a [Nightly], sha: &'b str) -> Option<&'a Nightly>
where
    'b: 'a,
{
    info!("Searching for nightly image with sha: {}", sha);
    nightlies.iter().find(move |nightly| nightly.sha == sha)
}

pub fn find_tags_by_sha<'a, 'b>(tags: &'a [Tag], sha: &'b str) -> impl Iterator<Item = &'a Tag> + 'a
where
    'b: 'a,
{
    info!("Searching for tag with sha: {}", sha);
    tags.iter().filter(move |t| t.name.contains(sha))
}

/// Given a list of tags, find any tags that represent nightlies
/// not already tracked in 'nightlies' and add them to 'nightlies'
///
/// # Errors
/// - Errors if any of the tags cannot be parsed into a nightly
/// - Errors if any of the tags are missing a sha
pub fn enrich_nightlies(tags: &[Tag], nightlies: &mut Vec<Nightly>) -> Result<(), NightlyError> {
    let initial_nightlies_len = nightlies.len();

    debug!("Processing {} tags to enrich nightlies", tags.len());
    // Filter tags to just those with 'nightly-full-main' prefix and '-jmx' suffix
    let valid_tags: Vec<&Tag> = tags
        .iter()
        .filter(|tag| {
            let has_sha = tag.get_sha().is_some();
            trace!("Tag {}: has_sha={}", tag.name, has_sha);
            has_sha
        })
        .collect();

    debug!("Found {} valid nightly-full tags", valid_tags.len());

    for tag in valid_tags {
        let Some(sha) = tag.get_sha() else {
            unreachable!("Tag {} missing SHA, but just validated it.", tag.name);
        };
        // Skip if we already have this nightly
        if nightlies.iter().any(|n| n.sha == sha) {
            trace!("Skipping already tracked nightly for SHA: {}", sha);
            continue;
        }

        // Create the new nightly
        debug!(
            "Creating new nightly for SHA: {} with tag: {}",
            sha, tag.name
        );

        let sha_timestamp = match get_commit_timestamp(sha) {
            Ok(timestamp) => Some(timestamp),
            Err(e) => {
                warn!(
                    "Error getting commit timestamp for nightly sha {}: {}",
                    sha, e
                );
                None
            }
        };

        let nightly = Nightly {
            sha: sha.to_string(),
            estimated_last_pushed: tag.last_pushed,
            sha_timestamp,
            tag: tag.clone(),
        };

        nightlies.push(nightly);
    }

    debug!(
        "Added {} new nightlies from tags",
        nightlies.len() - initial_nightlies_len
    );

    Ok(())
}

#[must_use]
pub fn tags_to_nightlies(tags: &[Tag]) -> Vec<Nightly> {
    let mut nightlies = Vec::new();

    debug!("Converting {} tags to nightlies", tags.len());
    // Filter to just nightly-full tags
    let valid_tags: Vec<&Tag> = tags.iter().filter(|tag| tag.get_sha().is_some()).collect();

    debug!("Found {} valid nightly-full tags", valid_tags.len());

    for tag in valid_tags {
        let Some(sha) = tag.get_sha() else {
            unreachable!("Tag {} missing SHA, but just validated it.", tag.name);
        };

        let sha_timestamp = match get_commit_timestamp(sha) {
            Ok(timestamp) => Some(timestamp),
            Err(e) => {
                warn!(
                    "Error getting commit timestamp for nightly sha {}: {}",
                    sha, e
                );
                None
            }
        };

        let nightly = Nightly {
            sha: sha.to_string(),
            estimated_last_pushed: tag.last_pushed,
            sha_timestamp,
            tag: tag.clone(),
        };

        nightlies.push(nightly);
    }

    // Sort nightlies by last_pushed date
    nightlies.sort_by(|a, b| b.estimated_last_pushed.cmp(&a.estimated_last_pushed));

    nightlies
}

/// Fetches the first `num_pages` of results from the docker registry API
/// Page size is hardcoded to 100
///
/// # Panics
/// - Panics if unexpected data is returned from the docker registry api
///
/// # Errors
/// - Errors if there is a problem fetching data from the docker registry api
pub async fn fetch_docker_registry_tags(num_pages: usize) -> Result<Vec<Tag>, NightlyError> {
    // Updated to search for nightly-full-main prefix
    let mut url = format!("{URL}?page_size=100&name=nightly-full-main-");

    let mut tags: Vec<Tag> = Vec::new();
    let mut num_pages_fetched = 0;
    debug!("Starting to fetch Docker registry tags with prefix 'nightly-full-main-'");

    loop {
        if num_pages_fetched >= num_pages {
            break;
        }

        debug!("Fetching page {} from URL: {}", num_pages_fetched + 1, url);
        let response: Value = reqwest::get(&url).await?.json().await?;
        let results = response["results"].as_array().unwrap();
        debug!("Received {} results from Docker registry", results.len());

        let mut tag_results: Vec<Tag> = results
            .iter()
            .filter_map(|t| match serde_json::from_value::<Tag>(t.clone()) {
                Ok(tag) => {
                    // Only keep tags ending with -jmx
                    if !tag.name.ends_with("-jmx") {
                        debug!("Skipping tag not ending with -jmx: {}", tag.name);
                        return None;
                    }

                    // Check SHA is valid
                    if tag.get_sha().is_none() {
                        debug!("Skipping tag with invalid SHA format: {}", tag.name);
                        return None;
                    }

                    trace!("Found valid nightly-full tag: {}", tag.name);
                    Some(tag)
                }
                Err(e) => {
                    warn!("Error parsing tag: {}", e);
                    None
                }
            })
            .collect::<Vec<_>>();

        debug!(
            "Processed {} valid nightly-full tags from response",
            tag_results.len()
        );

        tags.append(&mut tag_results);

        if response["next"].is_null() {
            break;
        }
        url = response["next"].as_str().unwrap().to_string();
        num_pages_fetched += 1;
    }

    debug!("Fetched a total of {} nightly-full tags", tags.len());
    Ok(tags)
}

/// Print the given nightly
///
/// # Panics
/// - If the writer encounters an error while writing output
pub fn print<W>(mut writer: W, nightly: &Nightly, all_tags: bool, print_digest: bool)
where
    W: std::io::Write,
{
    // Extract SHA for URI coloring
    let sha = nightly.tag.get_sha().unwrap_or(&nightly.sha);

    // Get formatted date for the header - using a more human-readable format
    let date = nightly.tag.last_pushed.format("%B %eth").to_string();

    // Header with date and SHA
    writeln!(
        writer,
        "{}",
        format!("┌─ {} Agent Nightly ({})", date.yellow(), sha.bright_blue()).bold()
    )
    .expect("Error writing to writer");

    // Add pushed timestamp as a separate row
    let pushed_time = nightly
        .tag
        .last_pushed
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    writeln!(
        writer,
        "│  {} {}",
        "Image Pushed At:".cyan(),
        pushed_time.yellow()
    )
    .expect("Error writing to writer");

    // Full image URI as a separate row with only SHA colorized
    let uri_parts: Vec<&str> = nightly.tag.name.split(sha).collect();
    writeln!(
        writer,
        "│  {} datadog/agent-dev:{}{}{}",
        "Image URI:".cyan(),
        uri_parts[0],
        sha.bright_blue(),
        uri_parts.get(1).unwrap_or(&"")
    )
    .expect("Error writing to writer");

    // SHA info with timestamp
    if let Some(sha_timestamp) = nightly.sha_timestamp {
        let formatted_date = sha_timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        writeln!(
            writer,
            "│  {} {}",
            "SHA Timestamp:".cyan(),
            formatted_date.yellow()
        )
        .expect("Error writing nightly to writer");
    }

    // GitHub URL
    writeln!(
        writer,
        "│  {} {}{}",
        "GitHub URL:".cyan(),
        "https://github.com/DataDog/datadog-agent/tree/".normal(),
        nightly.sha.bright_blue()
    )
    .expect("Error writing nightly to writer");

    // Additional tag info if requested
    if all_tags {
        print_tag(&mut writer, &nightly.tag, print_digest);
    }

    // Footer for each nightly
    writeln!(writer, "└─────────────────────────────────────").expect("Error writing to writer");
}

pub fn print_tag<W>(mut writer: W, tag: &Tag, print_digest: bool)
where
    W: std::io::Write,
{
    if print_digest {
        writeln!(
            writer,
            "│  {} {}",
            "Image Digest:".cyan(),
            tag.digest.bright_magenta()
        )
        .expect("Error writing tag to writer");
    }
}

/// Saves the given nightlies to a cache file
///
/// # Errors
/// - Errors if the cache file cannot be written to
/// - Errors if the nightlies cannot be serialized to json
pub fn save_db_to_cache(nightlies: &[Nightly]) -> Result<(), crate::NightlyError> {
    let file: &Path = CACHE_FILE.as_path();
    fs::write(file, serde_json::to_string_pretty(&nightlies)?)?;
    debug!("Updated nightlies saved to {file}", file = file.display());
    Ok(())
}

/// Loads nightlies from a cache file
///
/// # Errors
/// - Errors if the cache file cannot be read
/// - Errors if the nightlies cannot be deserialized from json
pub fn load_db_from_cache() -> Result<Vec<Nightly>, crate::NightlyError> {
    let file: &Path = CACHE_FILE.as_path();
    debug!(
        "Reading cached nightlies from {file}",
        file = file.display()
    );
    match fs::read_to_string(file) {
        Ok(file_content) => {
            let tags: Vec<Nightly> = serde_json::from_str(&file_content)?;
            Ok(tags)
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                // No cache file found, this is not a concerning error
            } else {
                warn!("Cache file reading error: {}", e);
            }
            Ok(Vec::new())
        }
    }
}

impl Nightly {
    /// Returns true if this nightly was built on a weekend (Saturday or Sunday in UTC)
    pub fn is_weekend_build(&self) -> bool {
        let weekday = self.estimated_last_pushed.weekday();
        weekday == Weekday::Sat || weekday == Weekday::Sun
    }
}
