use crate::NightlyError;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf}, collections::HashMap,
};
use tracing::{debug, warn, info};

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Tag {
    pub name: String,

    #[serde(rename = "tag_last_pushed")]
    pub last_pushed: DateTime<Utc>,

    pub digest: String,

    pub sha: Option<String>,
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

pub fn find_nightly_by_build_sha<'a, 'b>(
    nightlies: &'a [Nightly],
    build_sha: &'b str,
) -> Option<&'a Nightly>
where
    'b: 'a,
{
    info!("Searching for nightly image with sha: {}", build_sha);
    nightlies.iter().filter(move |nightly| nightly.sha == build_sha).next()
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

#[derive(Debug, PartialEq, Clone)]
pub struct Nightly {
    pub sha: String,
    pub estimated_last_pushed: DateTime<Utc>,
    // todo complement this with a lookup of the git commit sha and get the timestamp from that

    pub py3: Tag,
    pub py2: Tag,
    pub py3_jmx: Tag,
    pub py2_jmx: Tag,
    pub jmx: Tag,
}

pub fn tags_to_nightlies(tags: &[Tag]) -> Vec<Nightly> {
    let mut nightlies: HashMap<String, Vec<Tag>> = HashMap::new();
    for tag in tags {
        let Some(sha) = &tag.sha else {
            continue
        };
        let entry = nightlies.entry(sha.clone()).or_insert_with(|| { vec![] });
        entry.push(tag.clone());
    }

    let mut nightlies = nightlies.into_iter().map(|(sha, tags)| {
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
        match (py3, py2, py3_jmx, py2_jmx, jmx) {
            (Some(py3), Some(py2), Some(py3_jmx), Some(py2_jmx), Some(jmx)) => {
                let estimated_last_pushed = py3.last_pushed;
                Some(Nightly {
                    sha,
                    estimated_last_pushed,
                    py3,
                    py2,
                    py3_jmx,
                    py2_jmx,
                    jmx,
                })
            }
            _ => {
                warn!("Missing tags for sha: {}", sha);
                None
            }
        }
    })
    .filter_map(|n| n)
    .collect::<Vec<Nightly>>();

    nightlies.sort_by(|a, b| b.estimated_last_pushed.cmp(&a.estimated_last_pushed));

    nightlies
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
            .map(|mut tag: Tag| {
                if let Some(sha) = tag.name.split('-').nth(2) {
                    if sha.len() == 8 {
                        tag.sha = Some(sha.to_string());
                    }
                }
                tag
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

pub fn print_nightly<W>(mut writer: W, nightly: &Nightly, all_tags: bool, print_digest: bool) where W : std::io::Write {
    if all_tags {
        print_tag(&mut writer, &nightly.py3_jmx, true, print_digest);
        print_tag(&mut writer, &nightly.py2_jmx, true, print_digest);
        print_tag(&mut writer, &nightly.py3, true, print_digest);
        print_tag(&mut writer, &nightly.py2, true, print_digest);
    } else {
        print_tag(&mut writer, &nightly.py3_jmx, true, print_digest);
    }
}

pub fn print_tag<W>(mut writer: W, tag: &Tag, all_tags: bool, print_digest: bool) where W : std::io::Write {
    if all_tags || tag.name.ends_with("-py3") {
        let last_pushed = tag.last_pushed.to_rfc3339();
        write!(
            writer,
            "Tag: datadog/agent-dev:{},\tLast Pushed: {}",
            tag.name, last_pushed,
        ).expect("Error writing tag to writer");

        if print_digest {
            write!(writer, ",\tImage Digest: {}", tag.digest).expect("Error writing tag to writer");
        }

        if let Some(sha) = &tag.sha {
            write!(
                writer,
                ",\tGitHub URL: https://github.com/DataDog/datadog-agent/tree/{}",
                sha
            ).expect("Error writing tag to writer");
        }
        write!(writer, "\n").expect("Error writing tag to writer");
    }
}
