use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use clap::{App, Arg, SubCommand};
use reqwest;
use serde_json::{json, Value};
use std::fs;

const URL: &str = "https://hub.docker.com/v2/repositories/datadog/agent-dev/tags";
const OUTPUT_FILE: &str = "agent_nightlies.json";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = App::new("Agent Nightly Images")
        .version("1.0")
        .author("Scott Opell <me@scottopell.com>")
        .about("Scrapes and lists the recent agent nightly images and a GH link for each.")
        .subcommand(SubCommand::with_name("update")
            .about("Updates the database with new nightlies"))
        .subcommand(SubCommand::with_name("query")
            .about("Queries the database for nightlies")
            .arg(Arg::with_name("recent")
                .short("r")
                .long("recent")
                .value_name("NUMBER")
                .help("Queries the most recent N nightlies")
                .takes_value(true))
            .arg(Arg::with_name("all-tags")
                .long("all-tags")
                .help("Include all tags, not just those ending in -py3"))
            .arg(Arg::with_name("from")
                .short("f")
                .long("from")
                .value_name("FROM_DATE")
                .help("Queries nightlies from this date (inclusive), format: YYYY-MM-DDTHH:MM:SS")
                .takes_value(true))
            .arg(Arg::with_name("to")
                .short("t")
                .long("to")
                .value_name("TO_DATE")
                .help("Queries nightlies to this date (inclusive), format: YYYY-MM-DDTHH:MM:SS")
                .takes_value(true)))
        .get_matches();

    match matches.subcommand() {
        ("update", Some(_)) => update_database().await?,
        ("query", Some(matches)) => {
            let mut tags = load_tags()?;

            // Check if there's data from the last 48 hours
            let mut is_data_recent = false;
            let now = Utc::now();
            for tag in &tags {
                let pushed_date =
                    DateTime::parse_from_rfc3339(tag["tag_last_pushed"].as_str().unwrap())?
                        .with_timezone(&Utc);
                if (now - pushed_date).num_hours() < 48 {
                    is_data_recent = true;
                    break;
                }
            }

            // Update database if data is stale
            if !is_data_recent {
                println!("Data from the last 48 hours not found. Updating database...");
                update_database().await?;
                tags = load_tags()?;
            }

            // Execute the query requested
            let all_tags = matches.is_present("all-tags");
            if let Some(n) = matches.value_of("recent") {
                let n = n.parse::<usize>().unwrap();
                query_recent(&tags, n, all_tags)?;
            } else if let Some(from) = matches.value_of("from") {
                let from_date = NaiveDateTime::parse_from_str(from, "%Y-%m-%dT%H:%M:%S").unwrap();
                let to_date = matches
                    .value_of("to")
                    .map(|to| NaiveDateTime::parse_from_str(to, "%Y-%m-%dT%H:%M:%S").unwrap())
                    .unwrap_or_else(|| Utc::now().naive_utc());
                query_range(&tags, from_date, to_date, all_tags)?;
            }
        }
        _ => {
            let tags = load_tags()?;
            query_recent(&tags, 10, false)?;
        }
    }

    Ok(())
}

async fn update_database() -> Result<(), Box<dyn std::error::Error>> {
    let mut tags = load_tags()?;
    let most_recent_date = tags
        .iter()
        .map(|tag| DateTime::parse_from_rfc3339(tag["tag_last_pushed"].as_str().unwrap()).unwrap())
        .max()
        .unwrap_or_else(|| (Utc::now() - Duration::weeks(52)).into());

    let mut url = format!("{}?page_size=100&name=nightly-main-", URL);

    loop {
        let response: Value = reqwest::get(&url).await?.json().await?;
        let results = response["results"].as_array().unwrap();

        for tag in results {
            let last_pushed =
                DateTime::parse_from_rfc3339(tag["tag_last_pushed"].as_str().unwrap()).unwrap();
            if last_pushed > most_recent_date {
                let new_tag = json!({
                    "name": tag["name"].as_str().unwrap(),
                    "tag_last_pushed": tag["tag_last_pushed"].as_str().unwrap()
                });
                tags.push(new_tag);
            }
        }

        if response["next"].is_null() {
            break;
        }
        url = response["next"].as_str().unwrap().to_string();
    }

    fs::write(OUTPUT_FILE, serde_json::to_string_pretty(&tags)?)?;
    println!("Updated tags saved to {}", OUTPUT_FILE);
    Ok(())
}

fn load_tags() -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let file_content = fs::read_to_string(OUTPUT_FILE).unwrap_or_else(|_| "[]".to_string());
    let tags: Vec<Value> = serde_json::from_str(&file_content)?;
    Ok(tags)
}

fn query_recent(
    tags: &[Value],
    n: usize,
    all_tags: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let filtered_tags: Vec<&Value> = tags
        .iter()
        .filter(|&tag| all_tags || tag["name"].as_str().unwrap().ends_with("-py3"))
        .collect();
    let mut recent_tags: Vec<&Value> = filtered_tags.iter().cloned().collect();
    recent_tags.sort_by(|a, b| {
        b["tag_last_pushed"]
            .as_str()
            .cmp(&a["tag_last_pushed"].as_str())
    });

    for tag in recent_tags.iter().take(n) {
        print_tag(tag, all_tags)?;
    }
    Ok(())
}

fn query_range(
    tags: &[Value],
    from_date: NaiveDateTime,
    to_date: NaiveDateTime,
    all_tags: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for tag in tags {
        let tag_date = DateTime::parse_from_rfc3339(tag["tag_last_pushed"].as_str().unwrap())
            .unwrap()
            .naive_utc();
        if tag_date >= from_date && tag_date <= to_date {
            print_tag(tag, all_tags)?;
        }
    }
    Ok(())
}

fn print_tag(tag: &Value, all_tags: bool) -> Result<(), Box<dyn std::error::Error>> {
    let name = tag["name"].as_str().unwrap();
    if all_tags || name.ends_with("-py3") {
        let last_pushed = tag["tag_last_pushed"].as_str().unwrap();
        print!("Name: {}, Last Pushed: {}", name, last_pushed);

        if let Some(sha) = name.split('-').nth(2) {
            if sha.len() == 8 {
                print!(
                    ", GitHub URL: https://github.com/DataDog/datadog-agent/tree/{}",
                    sha
                );
            }
        }
        println!();
    }
    Ok(())
}
