use chrono::{DateTime, Duration, Utc};
use clap::Parser;
use once_cell::sync::Lazy;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio::task::JoinError;
use tracing::{debug, info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";

static CACHE_FILE: Lazy<PathBuf> = Lazy::new(|| {
    // get a 'stable' temp dir that can be used to cache the results from previous runs
    let dir = std::env::temp_dir();
    PathBuf::from(dir).join("agent_nightlies.json")
});

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, crate::NightlyError> {
    let datetime = DateTime::parse_from_rfc3339(s)?;
    Ok(datetime.into())
}

/// Lists the most recent agent-dev nightly images and a GH link for each
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Include all tags, not just those ending in -py3
    #[arg(short, long, default_value_t = false)]
    all_tags: bool,

    /// Print the image digest for each tag
    #[arg(short, long, default_value_t = false)]
    print_digest: bool,

    /// If the given target_sha exists as a nightly, print the tag
    #[arg(long)]
    target_sha: Option<String>,

    /// Start date for query (inclusive), format: YYYY-MM-DDTHH:MM:SS
    #[arg(short, long, value_parser = parse_datetime)]
    from_date: Option<DateTime<Utc>>,

    /// End date for query (inclusive), format: YYYY-MM-DDTHH:MM:SS
    #[arg(short, long, value_parser = parse_datetime)]
    to_date: Option<DateTime<Utc>>,
}

fn merge_tags(tags_a: Vec<Tag>, tags_b: Vec<Tag>) -> Result<Vec<Tag>, crate::NightlyError> {
    let mut tags = tags_a;

    // Remove duplicates and ensure sorted by struct field last_pushed
    let mut tags_b: Vec<Tag> = tags_b.into_iter().filter(|t| !tags.contains(t)).collect();
    tags.append(&mut tags_b);
    tags.sort_by(|a, b| b.last_pushed.cmp(&a.last_pushed));

    Ok(tags)
}

#[derive(Error, Debug)]
pub enum NightlyError {
    #[error("Error while fetching tags from docker registry: {0}")]
    FetchError(#[from] reqwest::Error),

    #[error("Error while interacting with tag cache file: {0}")]
    FileError(#[from] std::io::Error),

    #[error("Json error: {0}")]
    JSONError(#[from] serde_json::Error),

    #[error("Join error: {0}")]
    JoinError(#[from] JoinError),

    #[error("Parse Error: {0}")]
    DateParseError(#[from] chrono::ParseError),

    #[error("Generic Error: {0}")]
    GenericError(String),
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
struct Tag {
    name: String,

    #[serde(rename = "tag_last_pushed")]
    last_pushed: DateTime<Utc>,

    digest: String,
}

#[tokio::main]
async fn main() -> Result<(), NightlyError> {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(env_filter)
        .init();

    info!("Hello, world!");
    let args = Args::parse();

    // Fetch tags from docker registry and load from cache file in parallel
    // This idea is sound, but not really that useful in practice.
    // But this project is just for fun anyway
    let (live_tags, file_tags) = tokio::join!(
        tokio::spawn(async move {
            let tags = fetch_docker_registry_tags(1).await?;
            Ok::<_, crate::NightlyError>(tags)
        }),
        tokio::spawn(async move {
            let tags = load_tags()?;
            Ok::<_, crate::NightlyError>(tags)
        })
    );
    let tags = merge_tags(live_tags??, file_tags??)?;

    let to_save = tags.clone();
    tokio::spawn(async move {
        match save_cached_tags(&to_save) {
            Ok(_) => {}
            Err(e) => warn!("Error saving tags: {}", e),
        }
    });

    // If dates are specified, lets look at that range
    if let Some(from) = args.from_date {
        let tags = query_range(&tags, from, args.to_date);
        for t in tags {
            print_tag(t, args.all_tags, args.print_digest);
        }
    } else if let Some(target_sha) = args.target_sha {
        let target_tags = find_tags_by_sha(&tags, &target_sha);
        for t in target_tags {
            print_tag(t, args.all_tags, args.print_digest);
        }
    } else {
        // default is to just display the most recent 7 days
        let tags = query_range(&tags, (Utc::now() - Duration::days(7)).into(), None);
        for t in tags {
            print_tag(t, args.all_tags, args.print_digest);
        }
    }

    Ok(())
}

fn find_tags_by_sha<'a, 'b>(
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
async fn fetch_docker_registry_tags(num_pages: usize) -> Result<Vec<Tag>, crate::NightlyError> {
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

fn save_cached_tags(tags: &[Tag]) -> Result<(), crate::NightlyError> {
    let file: &Path = CACHE_FILE.as_path();
    fs::write(file, serde_json::to_string_pretty(&tags)?)?;
    debug!("Updated tags saved to {file}", file = file.display());
    Ok(())
}

fn load_tags() -> Result<Vec<Tag>, crate::NightlyError> {
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

fn query_range(
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

fn print_tag(tag: &Tag, all_tags: bool, print_digest: bool) {
    if all_tags || tag.name.ends_with("-py3") {
        let last_pushed = tag.last_pushed.to_rfc3339();
        print!("Name: {}, Last Pushed: {}", tag.name, last_pushed,);

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
