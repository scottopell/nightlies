use chrono::{DateTime, Duration, Utc};
use clap::Parser;
use nightlies::{
    nightly::{
        fetch_docker_registry_tags, find_tags_by_sha, load_tags, merge_tags, print_tag,
        query_range, save_cached_tags,
    },
    NightlyError,
};
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, NightlyError> {
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
