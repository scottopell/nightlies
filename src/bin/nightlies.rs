use std::fmt::Write;
use std::io::Write as IoWrite;

use chrono::{DateTime, Duration, Utc, NaiveDateTime, NaiveTime, NaiveDate};
use clap::Parser;
use nightlies::{
    nightly::{
        fetch_docker_registry_tags, print_tag,
        print_nightly,
        query_range, tags_to_nightlies, find_nightly_by_build_sha,
    },
    NightlyError, repo::get_first_nightly_containing_change,
};
use tabwriter::TabWriter;
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, NightlyError> {
    let mut err_str = String::new();
    match DateTime::parse_from_rfc3339(s) {
        Ok(datetime) => return Ok(datetime.into()),
        Err(e) => {
            err_str.write_fmt(format_args!("Error parsing date as RFC3339: {}", e))
                .unwrap();
        }
    }
    match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        Ok(date) => {
            let default_time = NaiveTime::from_hms_opt(0, 0, 0).expect("Invalid time");
            let datetime = NaiveDateTime::new(date, default_time);
            return Ok(datetime.and_utc().into());
        }
        Err(e) => {
            err_str.write_fmt(format_args!("\n Error parsing date as YYYY-MM-DD: {}", e))
                .unwrap();
        }
    }
    Err(NightlyError::DateParseError(err_str))
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

    /// If the given build_sha exists as a nightly, print the tag
    #[arg(long)]
    build_sha: Option<String>,

    /// Given a sha that exists in the 'main' branch of the datadog-agent repo, print
    /// the first nightly that contains that sha
    #[arg(long)]
    agent_sha: Option<String>,

    /// Number of pages to fetch from the docker registry API
    #[arg(long)]
    num_registry_pages: Option<usize>,

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

    // TODO the way this should work is that we query pages until we are able to
    // find the target_sha and/or find results from the 'from_date'
    // For now I've added in a cli option to specify number of pages
    // If you don't see the dates you're looking for, try increasing the number of pages
    let num_pages = args.num_registry_pages.unwrap_or(1);
    let tags = fetch_docker_registry_tags(num_pages).await?;
    let nightlies = tags_to_nightlies(&tags);

    let mut tw = TabWriter::new(vec![]);
    // If dates are specified, lets look at that range
    if let Some(from) = args.from_date {
        info!("Querying range: {} - {}", from, args.to_date.unwrap_or(Utc::now()));
        let tags = query_range(&tags, from, args.to_date);
        for t in tags {
            print_tag(&mut tw, t, args.all_tags, args.print_digest);
        }
    } else if let Some(build_sha) = args.build_sha {
        let nightly = find_nightly_by_build_sha(&nightlies, &build_sha);
        if let Some(nightly) = nightly {
            print_nightly(&mut tw, &nightly, args.all_tags, args.print_digest);
        } else {
            warn!("Could not find nightly for build sha: {}", build_sha)
        }
    } else if let Some(sha) = args.agent_sha {
        let nightly = get_first_nightly_containing_change(&nightlies, &sha)?;

        write!(&mut tw, "The first nightly containing the target sha is:\n").expect("Error writing to tabwriter");
        print_nightly(&mut tw, &nightly, args.all_tags, args.print_digest);
    } else {
        // default is to just display the most recent 7 days
        let tags = query_range(&tags, (Utc::now() - Duration::days(7)).into(), None);
        for t in tags {
            print_tag(&mut tw, t, args.all_tags, args.print_digest);
        }
    }

    let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
    print!("{}", written);

    Ok(())
}
