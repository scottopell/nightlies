use std::io::Write as IoWrite;

use chrono::{Duration, Utc};
use clap::Parser;
use colored::*;
use nightlies::{
    nightly::{
        enrich_nightlies, fetch_docker_registry_tags, find_nightly_by_build_sha,
        load_db_from_cache, print, save_db_to_cache,
    },
    repo::{get_first_nightly_containing_change, start_git_fetch},
    NightlyError,
};
use tabwriter::TabWriter;
use tracing::{debug, info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Lists the most recent agent-dev nightly images and a GH link for each
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Show tag details including pushed date and digest
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
    /// EXPERIMENTAL - there are known bugs, use at your own risk
    #[arg(long)]
    agent_sha: Option<String>,

    /// Skip git fetch operations (faster but might miss recent updates)
    #[arg(long, default_value_t = false)]
    no_fetch: bool,

    /// Force git fetch operations even if recently performed
    #[arg(long, default_value_t = false)]
    force_fetch: bool,

    /// Number of pages to fetch from the docker registry API
    #[arg(long)]
    num_registry_pages: Option<usize>,

    /// Show only most recently published nightly in full URI format
    #[arg(long, default_value_t = false)]
    latest_only: bool,

    /// Show only the 2nd most recently published nightly in full URI format
    #[arg(long, default_value_t = false)]
    prev_latest_only: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
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
    // find the target_sha
    // For now I've added in a cli option to specify number of pages
    let num_pages = args.num_registry_pages.unwrap_or(1);
    let no_fetch = args.no_fetch;
    let force_fetch = args.force_fetch;

    // Start all three operations in parallel:
    // 1. Fetch tags from Docker registry
    // 2. Load nightlies from cache file
    // 3. Start the git fetch to refresh the git repository
    let fetch_start = std::time::Instant::now();
    debug!("Starting parallel operations at {:?}", chrono::Utc::now());

    let (live_tags, file_nightlies, _) = tokio::join!(
        tokio::spawn(async move {
            let task_start = std::time::Instant::now();
            debug!(
                "TASK START: fetch_docker_registry_tags at {:?}",
                chrono::Utc::now()
            );
            let tags = fetch_docker_registry_tags(num_pages).await?;
            let task_end = std::time::Instant::now();
            debug!(
                "TASK END: fetch_docker_registry_tags at {:?}, duration: {:?}",
                chrono::Utc::now(),
                task_end.duration_since(task_start)
            );
            Ok::<_, crate::NightlyError>(tags)
        }),
        tokio::spawn(async move {
            let task_start = std::time::Instant::now();
            debug!("TASK START: load_db_from_cache at {:?}", chrono::Utc::now());
            let nightlies = load_db_from_cache()?;
            let task_end = std::time::Instant::now();
            debug!(
                "TASK END: load_db_from_cache at {:?}, duration: {:?}",
                chrono::Utc::now(),
                task_end.duration_since(task_start)
            );
            Ok::<_, crate::NightlyError>(nightlies)
        }),
        // Don't spawn this in another task - run it directly within join!
        async move {
            let task_start = std::time::Instant::now();
            debug!("TASK START: start_git_fetch at {:?}", chrono::Utc::now());
            // Start the git fetch in the background
            let result = start_git_fetch(no_fetch, force_fetch).await;
            let task_end = std::time::Instant::now();
            debug!(
                "TASK END: start_git_fetch at {:?}, duration: {:?}",
                chrono::Utc::now(),
                task_end.duration_since(task_start)
            );
            if let Err(e) = result {
                warn!("Error starting git fetch: {}", e);
            }
            Ok::<_, crate::NightlyError>(())
        }
    );

    let fetch_end = std::time::Instant::now();
    debug!(
        "All parallel operations completed at {:?}, total duration: {:?}",
        chrono::Utc::now(),
        fetch_end.duration_since(fetch_start)
    );
    let live_tags = live_tags??;
    let mut nightlies = file_nightlies??;

    enrich_nightlies(&live_tags, &mut nightlies)?;

    let to_save = nightlies.clone();
    tokio::spawn(async move {
        match save_db_to_cache(&to_save) {
            Ok(_) => {}
            Err(e) => warn!("Error saving db: {}", e),
        }
    });

    let mut tw = TabWriter::new(vec![]);
    if args.latest_only {
        let latest = nightlies.iter().max_by_key(|n| n.sha_timestamp);
        if let Some(latest) = latest {
            // For latest-only, just show the plain tag name without formatting
            writeln!(&mut tw, "datadog/agent-dev:{}", latest.tag.name)
                .expect("Error writing to tabwriter");
        }
        let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
        print!("{}", written);
        return Ok(());
    }
    if args.prev_latest_only {
        // get the 2nd most recent by sha_timestamp
        let mut nightlies = nightlies.clone();
        nightlies.sort_by(|a, b| a.sha_timestamp.cmp(&b.sha_timestamp));
        let prev_latest = nightlies.get(nightlies.len() - 2);
        if let Some(prev_latest) = prev_latest {
            // For prev-latest-only, just show the plain tag name without formatting
            writeln!(&mut tw, "datadog/agent-dev:{}", prev_latest.tag.name)
                .expect("Error writing to tabwriter");
        }
        let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
        print!("{}", written);
        return Ok(());
    }

    if let Some(build_sha) = args.build_sha {
        let nightly = find_nightly_by_build_sha(&nightlies, &build_sha);
        if let Some(nightly) = nightly {
            print(&mut tw, nightly, args.all_tags, args.print_digest);
        } else {
            warn!("Could not find nightly for build sha: {}", build_sha)
        }
    } else if let Some(sha) = args.agent_sha {
        let nightly = get_first_nightly_containing_change(&nightlies, &sha)?;

        writeln!(
            &mut tw,
            "{}",
            "The first nightly containing the target sha is:"
                .yellow()
                .bold()
        )
        .expect("Error writing to tabwriter");
        print(&mut tw, &nightly, args.all_tags, args.print_digest);
    } else {
        // default is to just display the most recent 7 days
        let mut nightlies_vec: Vec<&nightlies::nightly::Nightly> = nightlies.iter().collect();
        nightlies_vec.sort_by(|a, b| {
            let a_time = a.sha_timestamp.unwrap_or(a.estimated_last_pushed);
            let b_time = b.sha_timestamp.unwrap_or(b.estimated_last_pushed);
            a_time.cmp(&b_time)
        });
        // Only show the last week by default
        let last_week = nightlies_vec
            .into_iter()
            .filter(|n| {
                let timestamp = n.sha_timestamp.unwrap_or(n.estimated_last_pushed);
                timestamp > (Utc::now() - Duration::days(7))
            })
            .collect::<Vec<_>>();

        if !last_week.is_empty() {
            writeln!(
                &mut tw,
                "{}",
                format!("Showing {} nightlies from the past week:", last_week.len())
                    .cyan()
                    .bold()
            )
            .expect("Error writing to tabwriter");
        } else {
            writeln!(
                &mut tw,
                "{}",
                "No nightlies found for the past week.".yellow()
            )
            .expect("Error writing to tabwriter");
        }

        for n in last_week {
            print(&mut tw, n, args.all_tags, args.print_digest);
        }
    }

    let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
    print!("{}", written);

    Ok(())
}
