use crate::NightlyError;
use chrono::{DateTime, Utc};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::{warn, info};

const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Tag {
    pub name: String,
    #[serde(rename = "tag_last_pushed")]
    pub last_pushed: DateTime<Utc>,
    pub digest: String,
    pub sha: Option<String>,
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


pub fn find_nightly_by_build_sha<'a, 'b>(
    nightlies: &'a [Nightly],
    build_sha: &'b str,
) -> Option<&'a Nightly>
where
    'b: 'a,
{
    info!("Searching for nightly image with sha: {}", build_sha);
    nightlies.iter().find(move |nightly| nightly.sha == build_sha)
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

#[must_use]
pub fn tags_to_nightlies(tags: &[Tag]) -> Vec<Nightly> {
    let mut nightlies: HashMap<String, Vec<Tag>> = HashMap::new();
    for tag in tags {
        let Some(sha) = &tag.sha else {
            continue
        };
        let entry = nightlies.entry(sha.clone()).or_insert_with(|| { vec![] });
        entry.push(tag.clone());
    }

    let mut nightlies = nightlies.into_iter().filter_map(|(sha, tags)| {
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
        if let (Some(py3), Some(py2), Some(py3_jmx), Some(py2_jmx), Some(jmx)) = (py3, py2, py3_jmx, py2_jmx, jmx) {
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
        } else {
            warn!("Missing tags for sha: {}", sha);
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

pub fn print<W>(mut writer: W, nightly: &Nightly, all_tags: bool, print_digest: bool) where W : std::io::Write {
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
                ",\tGitHub URL: https://github.com/DataDog/datadog-agent/tree/{sha}"
            ).expect("Error writing tag to writer");
        }
        writeln!(writer).expect("Error writing tag to writer");
    }
}
