use crate::NightlyError;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::{debug, warn};

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Tag {
    name: String,

    #[serde(rename = "tag_last_pushed")]
    last_pushed: DateTime<Utc>,

    digest: String,
}

pub fn merge_tags(tags_a: Vec<Tag>, tags_b: Vec<Tag>) -> Result<Vec<Tag>, crate::NightlyError> {
    let mut tags = tags_a;

    // Remove duplicates and ensure sorted by struct field last_pushed
    let mut tags_b: Vec<Tag> = tags_b.into_iter().filter(|t| !tags.contains(t)).collect();
    tags.append(&mut tags_b);
    tags.sort_by(|a, b| b.last_pushed.cmp(&a.last_pushed));

    Ok(tags)
}

const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";

static CACHE_FILE: Lazy<PathBuf> = Lazy::new(|| {
    // get a 'stable' temp dir that can be used to cache the results from previous runs
    let dir = std::env::temp_dir();
    PathBuf::from(dir).join("agent_nightlies.json")
});

pub fn find_tags_by_sha<'a, 'b>(
    tags: &'a [Tag],
    target_sha: &'b str,
) -> impl Iterator<Item = &'a Tag> + 'a
where
    'b: 'a,
{
    debug!("Searching for tag with sha: {}", target_sha);
    tags.iter().filter(move |t| t.name.contains(target_sha))
}

/// Fetches the first `num_pages` of results from the docker registry API
/// Page size is hardcoded to 100
pub async fn fetch_docker_registry_tags(num_pages: usize) -> Result<Vec<Tag>, NightlyError> {
    let mut url = format!("{}?page_size=100&name=nightly-main-", URL);

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
            .filter_map(|t| match serde_json::from_value(t.clone()) {
                Ok(tag) => Some(tag),
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

pub fn save_cached_tags(tags: &[Tag]) -> Result<(), crate::NightlyError> {
    let file: &Path = CACHE_FILE.as_path();
    fs::write(file, serde_json::to_string_pretty(&tags)?)?;
    debug!("Updated tags saved to {file}", file = file.display());
    Ok(())
}

pub fn load_tags() -> Result<Vec<Tag>, crate::NightlyError> {
    let file: &Path = CACHE_FILE.as_path();
    match fs::read_to_string(file) {
        Ok(file_content) => {
            let tags: Vec<Tag> = serde_json::from_str(&file_content)?;
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

pub fn query_range(
    tags: &[Tag],
    from_date: DateTime<Utc>,
    to_date: Option<DateTime<Utc>>,
) -> impl Iterator<Item = &Tag> + '_ {
    let r = tags.iter().filter(move |t| {
        if let Some(to_date) = to_date {
            t.last_pushed <= to_date && t.last_pushed >= from_date
        } else {
            t.last_pushed >= from_date
        }
    });

    r
}

pub fn print_tag(tag: &Tag, all_tags: bool, print_digest: bool) {
    if all_tags || tag.name.ends_with("-py3") {
        let last_pushed = tag.last_pushed.to_rfc3339();
        print!(
            "Tag: datadog/agent-dev:{}, Last Pushed: {}",
            tag.name, last_pushed,
        );

        if print_digest {
            print!(", Image Digest: {}", tag.digest);
        }

        if let Some(sha) = tag.name.split('-').nth(2) {
            if sha.len() == 8 {
                print!(
                    ", GitHub URL: https://github.com/DataDog/datadog-agent/tree/{}",
                    sha
                );
            }
        }
        println!();
    }
}
