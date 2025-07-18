use std::io::Write as IoWrite;

use chrono::{Datelike, Weekday};
use chrono::{Duration, Utc};
use clap::Parser;
use colored::*;
use nightlies::{
    nightly::{
        enrich_nightlies, fetch_docker_registry_tags, load_db_from_cache, print, save_db_to_cache,
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
    #[command(subcommand)]
    command: Option<Commands>,
    /// Show tag details including pushed date and digest
    #[arg(short, long, default_value_t = false)]
    all_tags: bool,

    /// Print the image digest for each tag
    #[arg(short, long, default_value_t = false)]
    print_digest: bool,

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

    /// Include weekend builds (Saturday/Sunday in UTC)
    #[arg(long, default_value_t = false)]
    include_weekends: bool,


    /// Number of days to look back for nightlies (default: 7)
    #[arg(short, long, default_value_t = 7)]
    days: i64,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Show differences between nightlies
    Diff {
        /// Base (older) nightly for comparison. Can be a tag name, SHA, or full image URI.
        /// Examples: "nightly-full-main-abcd1234-jmx", "abcd1234", "datadog/agent-dev:nightly-full-main-abcd1234-jmx"
        #[arg(long)]
        base: Option<String>,

        /// Comparison (newer) nightly for comparison. Can be a tag name, SHA, or full image URI.
        /// Examples: "nightly-full-main-efgh5678-jmx", "efgh5678", "datadog/agent-dev:nightly-full-main-efgh5678-jmx"
        #[arg(long)]
        comparison: Option<String>,

        /// Interactively select nightlies to diff
        #[arg(short, long, default_value_t = false)]
        interactive: bool,

        /// Include weekend builds (Saturday/Sunday in UTC)
        #[arg(long, default_value_t = false)]
        include_weekends: bool,
    },
}

/// Checks if a timestamp is on a weekend (Saturday or Sunday)
fn is_weekend(timestamp: &chrono::DateTime<chrono::Utc>) -> bool {
    let weekday = timestamp.weekday();
    weekday == Weekday::Sat || weekday == Weekday::Sun
}

/// Parse a nightly identifier from various formats
/// 
/// Handles:
/// - Tag names: "nightly-full-main-abcd1234-jmx"
/// - SHAs: "abcd1234" (8 characters)
/// - Full URIs: "datadog/agent-dev:nightly-full-main-abcd1234-jmx"
fn parse_nightly_identifier(input: &str) -> Option<String> {
    // Check if it's a full URI
    if input.starts_with("datadog/agent-dev:") {
        let tag_part = input.strip_prefix("datadog/agent-dev:")?;
        return extract_sha_from_tag(tag_part);
    }
    
    // Check if it's a full tag name
    if input.starts_with("nightly-full-main-") && input.ends_with("-jmx") {
        return extract_sha_from_tag(input);
    }
    
    // Check if it's a SHA (8 characters, alphanumeric)
    if input.len() == 8 && input.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(input.to_string());
    }
    
    None
}

/// Extract SHA from a tag name
fn extract_sha_from_tag(tag: &str) -> Option<String> {
    if tag.starts_with("nightly-full-main-") && tag.ends_with("-jmx") {
        if let Some(sha) = tag.split('-').nth(3) {
            if sha.len() == 8 {
                return Some(sha.to_string());
            }
        }
    }
    None
}

/// Find a nightly by SHA
fn find_nightly_by_sha<'a>(nightlies: &'a [nightlies::nightly::Nightly], sha: &str) -> Option<&'a nightlies::nightly::Nightly> {
    nightlies.iter().find(|n| n.sha == sha)
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

    // Handle subcommands
    if let Some(command) = &args.command {
        match command {
            Commands::Diff { base, comparison, interactive, include_weekends: _ } => {
                // Validate argument combinations for diff subcommand
                if base.is_some() && comparison.is_none() {
                    anyhow::bail!("--base requires --comparison to be specified");
                }
                if comparison.is_some() && base.is_none() {
                    anyhow::bail!("--comparison requires --base to be specified");
                }
                if (base.is_some() || comparison.is_some()) && *interactive {
                    anyhow::bail!("--base/--comparison cannot be used with --interactive");
                }
                
                // Execute diff command logic after loading nightlies
                // This will be handled later in the function
            }
        }
    }

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

    // Handle subcommands
    if let Some(command) = &args.command {
        match command {
            Commands::Diff { base, comparison, interactive, include_weekends } => {
                if *interactive {
                    let (older_sha, newer_sha) =
                        nightlies::interactive::select_nightlies_to_diff(&nightlies, !*include_weekends)?;
                    nightlies::diff::show_diff_between_shas(older_sha, newer_sha).await?;
                    return Ok(());
                }

                // Handle non-interactive diffing with --base and --comparison
                if let (Some(base_input), Some(comparison_input)) = (base, comparison) {
                    // Parse the base identifier
                    let base_sha = parse_nightly_identifier(base_input)
                        .ok_or_else(|| anyhow::anyhow!("Invalid base identifier: '{}'. Expected tag name, SHA, or full URI.", base_input))?;
                    
                    // Parse the comparison identifier  
                    let comparison_sha = parse_nightly_identifier(comparison_input)
                        .ok_or_else(|| anyhow::anyhow!("Invalid comparison identifier: '{}'. Expected tag name, SHA, or full URI.", comparison_input))?;
                    
                    // Find the nightlies
                    let base_nightly = find_nightly_by_sha(&nightlies, &base_sha)
                        .ok_or_else(|| anyhow::anyhow!("Base nightly not found for SHA: {}", base_sha))?;
                    
                    let comparison_nightly = find_nightly_by_sha(&nightlies, &comparison_sha)
                        .ok_or_else(|| anyhow::anyhow!("Comparison nightly not found for SHA: {}", comparison_sha))?;
                    
                    // Ensure proper ordering (older first)
                    let base_ts = base_nightly.sha_timestamp.unwrap_or(base_nightly.estimated_last_pushed);
                    let comparison_ts = comparison_nightly.sha_timestamp.unwrap_or(comparison_nightly.estimated_last_pushed);
                    
                    let (older_sha, newer_sha) = if base_ts > comparison_ts {
                        (comparison_sha, base_sha)
                    } else {
                        (base_sha, comparison_sha)
                    };
                    
                    nightlies::diff::show_diff_between_shas(older_sha, newer_sha).await?;
                    return Ok(());
                }

                // Default behavior: show diff between latest two nightlies
                nightlies::diff::show_diff_between_latest_two(&nightlies, *include_weekends).await?;
                return Ok(());
            }
        }
    }

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

    if let Some(sha) = args.agent_sha {
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
        // Only show the specified number of days
        let filtered_nightlies = nightlies_vec
            .into_iter()
            .filter(|n| {
                // Use SHA timestamp with fallback to estimated_last_pushed for time filtering
                let timestamp = n.sha_timestamp.unwrap_or(n.estimated_last_pushed);

                // For the weekend check, use ONLY the estimated_last_pushed (Docker push timestamp)
                let is_weekend_build = is_weekend(&n.estimated_last_pushed);

                timestamp > (Utc::now() - Duration::days(args.days))
                    && (args.include_weekends || !is_weekend_build)
            })
            .collect::<Vec<_>>();

        if !filtered_nightlies.is_empty() {
            writeln!(
                &mut tw,
                "{}",
                format!(
                    "Showing {} nightlies from the past {} day{}{}:",
                    filtered_nightlies.len(),
                    args.days,
                    if args.days == 1 { "" } else { "s" },
                    if !args.include_weekends {
                        " (excluding weekend builds by push date)"
                    } else {
                        ""
                    }
                )
                .cyan()
                .bold()
            )
            .expect("Error writing to tabwriter");
        } else {
            writeln!(
                &mut tw,
                "{}",
                if !args.include_weekends {
                    format!("No nightlies found for the past {} day{} (excluding weekend builds by push date).", 
                           args.days, if args.days == 1 { "" } else { "s" })
                        .yellow()
                } else {
                    format!("No nightlies found for the past {} day{}.", 
                           args.days, if args.days == 1 { "" } else { "s" })
                        .yellow()
                }
            )
            .expect("Error writing to tabwriter");
        }

        for n in filtered_nightlies {
            print(&mut tw, n, args.all_tags, args.print_digest);
        }
    }

    let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
    print!("{}", written);

    Ok(())
}
