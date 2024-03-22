use crate::{repo::get_commit_timestamp, NightlyError};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use tracing::{debug, info, warn};

const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Tag {
    pub name: String,
    #[serde(rename = "tag_last_pushed")]
    pub last_pushed: DateTime<Utc>,
    pub digest: String,
}

impl Tag {
    fn get_sha(&self) -> Option<&str> {
        if let Some(sha) = self.name.split('-').nth(2) {
            if sha.len() == 8 {
                return Some(sha);
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Nightly {
    pub sha: String,
    pub estimated_last_pushed: DateTime<Utc>,
    pub sha_timestamp: DateTime<Utc>,

    pub py3: Option<Tag>,
    pub py2: Option<Tag>,
    pub py3_jmx: Option<Tag>,
    pub py2_jmx: Option<Tag>,
    pub jmx: Option<Tag>,
}

static CACHE_FILE: Lazy<PathBuf> = Lazy::new(|| {
    // get a 'stable' temp dir that can be used to cache the results from previous runs
    let dir = std::env::temp_dir();
    dir.join("agent_nightlies.json")
});

pub fn find_nightly_by_build_sha<'a, 'b>(
    nightlies: &'a [Nightly],
    build_sha: &'b str,
) -> Option<&'a Nightly>
where
    'b: 'a,
{
    info!("Searching for nightly image with sha: {}", build_sha);
    nightlies
        .iter()
        .find(move |nightly| nightly.sha == build_sha)
}

pub fn find_tags_by_build_sha<'a, 'b>(
    tags: &'a [Tag],
    build_sha: &'b str,
) -> impl Iterator<Item = &'a Tag> + 'a
where
    'b: 'a,
{
    info!("Searching for tag with build sha: {}", build_sha);
    tags.iter().filter(move |t| t.name.contains(build_sha))
}

/// Given a list of tags, find any tags that represent nightlies
/// not already tracked in 'nightlies' and add them to 'nightlies'
///
/// # Errors
/// - Errors if any of the tags cannot be parsed into a nightly
/// - Errors if any of the tags are missing a sha
/// - Errors if any of the tags are missing a timestamp
pub fn enrich_nightlies(tags: &[Tag], nightlies: &mut Vec<Nightly>) -> Result<(), NightlyError> {
    let initial_nightlies_len = nightlies.len();
    let mut nightlies_from_tags: HashMap<String, Vec<Tag>> = HashMap::new();
    for tag in tags {
        let Some(sha) = tag.get_sha() else {
            continue;
        };
        let entry = nightlies_from_tags
            .entry(sha.to_string())
            .or_insert_with(|| vec![]);
        entry.push(tag.clone());
    }

    for (nightly_sha, tags_for_sha) in &nightlies_from_tags {
        if !nightlies.iter_mut().any(|n| n.sha == *nightly_sha) {
            let new_nightly = sha_and_tags_to_nightly(nightly_sha, tags_for_sha)?;
            nightlies.push(new_nightly);
        }
    }

    debug!(
        "Added {} new nightlies from tags",
        nightlies.len() - initial_nightlies_len
    );

    Ok(())
}

fn sha_and_tags_to_nightly(sha: &str, tags: &[Tag]) -> Result<Nightly, NightlyError> {
    let mut py3 = None;
    let mut py2 = None;
    let mut py3_jmx = None;
    let mut py2_jmx = None;
    let mut jmx = None;
    for tag in tags {
        if tag.name.ends_with("-py3") {
            py3 = Some(tag);
        } else if tag.name.ends_with("-py2") {
            py2 = Some(tag);
        } else if tag.name.ends_with("-py3-jmx") {
            py3_jmx = Some(tag);
        } else if tag.name.ends_with("-py2-jmx") {
            py2_jmx = Some(tag);
        } else if tag.name.ends_with("-jmx") {
            jmx = Some(tag);
        }
    }
    let first_some = py3.or(py2).or(py3_jmx).or(py2_jmx).or(jmx);
    if let Some(tag) = first_some {
        let estimated_last_pushed = tag.last_pushed;

        let sha_timestamp = match get_commit_timestamp(sha) {
            Ok(timestamp) => timestamp,
            Err(e) => {
                warn!(
                    "Error getting commit timestamp for nightly sha: {}, skipping nightly...",
                    e
                );
                return Err(e);
            }
        };
        Ok(Nightly {
            sha: sha.to_string(),
            estimated_last_pushed,
            sha_timestamp,
            py3: py3.cloned(),
            py2: py2.cloned(),
            py3_jmx: py3_jmx.cloned(),
            py2_jmx: py2_jmx.cloned(),
            jmx: jmx.cloned(),
        })
    } else {
        Err(NightlyError::GenericError(format!(
            "Missing tags for sha: {sha}"
        )))
    }
}

#[must_use]
pub fn tags_to_nightlies(tags: &[Tag]) -> Vec<Nightly> {
    let mut nightlies: HashMap<String, Vec<Tag>> = HashMap::new();
    for tag in tags {
        let Some(sha) = tag.get_sha() else {
            continue;
        };
        let entry = nightlies.entry(sha.to_string()).or_insert_with(|| vec![]);
        entry.push(tag.clone());
    }

    let mut nightlies = nightlies
        .into_iter()
        .filter_map(|(sha, tags)| match sha_and_tags_to_nightly(&sha, &tags) {
            Ok(nightly) => Some(nightly),
            Err(e) => {
                warn!("Error parsing nightly: {}", e);
                None
            }
        })
        .collect::<Vec<Nightly>>();

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
    let mut url = format!("{URL}?page_size=100&name=nightly-main-");

    let mut tags: Vec<Tag> = Vec::new();
    let mut num_pages_fetched = 0;
    loop {
        if num_pages_fetched >= num_pages {
            break;
        }

        let response: Value = reqwest::get(&url).await?.json().await?;
        let results = response["results"].as_array().unwrap();
        let mut tag_results: Vec<Tag> = results
            .iter()
            .filter_map(|t| match serde_json::from_value::<Tag>(t.clone()) {
                Ok(tag) => {
                    if let Some(sha) = tag.name.split('-').nth(2) {
                        // Skip the 'main' tag that has no sha
                        // This floats around and isn't useful to us
                        if sha.is_empty() {
                            return None;
                        }
                    }

                    Some(tag)
                }
                Err(e) => {
                    warn!("Error parsing tag: {}", e);
                    None
                }
            })
            .collect::<Vec<_>>();
        tags.append(&mut tag_results);

        if response["next"].is_null() {
            break;
        }
        url = response["next"].as_str().unwrap().to_string();
        num_pages_fetched += 1;
    }

    Ok(tags)
}

pub fn query_range(
    nightlies: &[Nightly],
    from_date: DateTime<Utc>,
    to_date: Option<DateTime<Utc>>,
) -> impl Iterator<Item = &Nightly> + '_ {
    let r = nightlies.iter().filter(move |n| {
        if let Some(to_date) = to_date {
            n.sha_timestamp <= to_date && n.sha_timestamp >= from_date
        } else {
            n.sha_timestamp >= from_date
        }
    });

    r
}

/// Print the given nightly and optionally all tags
///
/// # Panics:
/// - If the writer encounters an error
/// - If the nightly is missing a valid image
pub fn print<W>(mut writer: W, nightly: &Nightly, all_tags: bool, print_digest: bool)
where
    W: std::io::Write,
{
    let first_valid_image = nightly
        .py3
        .as_ref()
        .or(nightly.py2.as_ref())
        .or(nightly.py3_jmx.as_ref())
        .or(nightly.py2_jmx.as_ref())
        .or(nightly.jmx.as_ref())
        .unwrap();
    writeln!(writer, "Nightly: datadog/agent-dev:{},\tSHA Timestamp: {}\tGitHub URL: https://github.com/DataDog/datadog-agent/tree/{}",
        first_valid_image.name,
        nightly.sha_timestamp.to_rfc3339(),
        nightly.sha,
    ).expect("Error writing nightly to writer");

    if all_tags {
        if let Some(tag) = &nightly.jmx {
            print_tag(&mut writer, tag, all_tags, print_digest);
        }
        if let Some(tag) = &nightly.py3_jmx {
            print_tag(&mut writer, tag, all_tags, print_digest);
        }
        if let Some(tag) = &nightly.py2_jmx {
            print_tag(&mut writer, tag, all_tags, print_digest);
        }
        if let Some(tag) = &nightly.py3 {
            print_tag(&mut writer, tag, all_tags, print_digest);
        }
        if let Some(tag) = &nightly.py2 {
            print_tag(&mut writer, tag, all_tags, print_digest);
        }
    }
}

pub fn print_tag<W>(mut writer: W, tag: &Tag, all_tags: bool, print_digest: bool)
where
    W: std::io::Write,
{
    if all_tags || tag.name.ends_with("-py3") {
        let last_pushed = tag.last_pushed.to_rfc3339();
        write!(
            writer,
            "Tag: datadog/agent-dev:{},\tLast Pushed: {}",
            tag.name, last_pushed,
        )
        .expect("Error writing tag to writer");

        if print_digest {
            write!(writer, ",\tImage Digest: {}", tag.digest).expect("Error writing tag to writer");
        }

        writeln!(writer).expect("Error writing tag to writer");
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
