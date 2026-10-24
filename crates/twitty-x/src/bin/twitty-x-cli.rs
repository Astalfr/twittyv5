//! Developer CLI for the X adapter: inspect the operation catalog, refresh
//! query ids, and capture raw responses for fixtures.
//!
//! Credentials come from `--session <file>` (`{"auth_token": "...", "ct0": "..."}`)
//! or the `TWITTY_AUTH_TOKEN` / `TWITTY_CT0` environment variables. They are
//! never printed.

#![expect(clippy::print_stdout, reason = "command-line output")]

use std::{path::PathBuf, sync::Arc};

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use serde_json::Value;
use twitty_x::{
    Session, XClient,
    catalog::Catalog,
    discovery::Discovery,
    transport::{ClientIds, RetryPolicy, Transport},
};
use url::Url;

#[derive(Parser)]
#[command(about = "Twitty's X adapter dev tool")]
struct Cli {
    /// JSON file with "auth_token" and "ct0". Defaults to the TWITTY_* env vars.
    #[arg(long, global = true)]
    session: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List known operations and where their query ids come from.
    Ops,
    /// Re-discover query ids from x.com bundles (logged in) and update the cache.
    RefreshIds,
    /// Fetch a timeline and save the raw JSON response.
    Capture {
        #[command(subcommand)]
        what: Capture,
    },
}

#[derive(Subcommand)]
enum Capture {
    /// The home timeline.
    Home {
        /// "Following" (chronological) instead of "For you".
        #[arg(long)]
        latest: bool,
        #[arg(long, default_value_t = 20)]
        count: u32,
        #[arg(long)]
        cursor: Option<String>,
        /// Use this query id instead of the catalog's.
        #[arg(long)]
        query_id: Option<String>,
        /// Where to write the response (gitignored `dumps/` by default).
        #[arg(long, default_value = "dumps/home.json")]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "twitty_x=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let catalog = Arc::new(Catalog::new(Catalog::default_cache_path()));

    match cli.command {
        Command::Ops => {
            for (name, spec, origin) in catalog.list() {
                println!(
                    "{name:<28} {:<24} {:?} {:?} ({})",
                    spec.query_id, spec.method, origin, spec.source
                );
            }
        }
        Command::RefreshIds => {
            let client = client(cli.session, catalog)?;
            let report = client.refresh_ids().await?;
            println!("scripts fetched by family: {:?}", report.scripts_by_family);
            println!("operations found by format: {:?}", report.ops_by_format);
            if !report.failed_pages.is_empty() {
                println!("pages that failed: {:?}", report.failed_pages);
            }
            for name in ["HomeTimeline", "HomeLatestTimeline", "TweetDetail", "SearchTimeline"] {
                if let Some((spec, origin)) = client.catalog().resolve(name) {
                    println!("{name:<20} {} ({origin:?})", spec.query_id);
                }
            }
        }
        Command::Capture { what: Capture::Home { latest, count, cursor, query_id, out } } => {
            let client = client(cli.session, catalog)?;
            if let Some(id) = query_id {
                let op = if latest { "HomeLatestTimeline" } else { "HomeTimeline" };
                client.catalog().set_override(op, &id);
            }
            let body = client.home_timeline(latest, count, cursor.as_deref()).await?;
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&out, serde_json::to_string_pretty(&body)?)
                .with_context(|| format!("writing {}", out.display()))?;
            let (instructions, entries) = summarize(&body);
            println!("saved {} ({} entries)", out.display(), entries);
            println!("instructions: {instructions:?}");
            if let Some(l) = client.transport().limits().get(if latest {
                "HomeLatestTimeline"
            } else {
                "HomeTimeline"
            }) {
                println!("rate limit: {}/{} until {}", l.remaining, l.limit, l.reset_at);
            }
        }
    }
    Ok(())
}

fn client(session_file: Option<PathBuf>, catalog: Arc<Catalog>) -> anyhow::Result<XClient> {
    let session = match session_file {
        Some(path) => {
            Session::from_file(&path).with_context(|| format!("reading {}", path.display()))?
        }
        None => Session::from_env()?,
    };
    let ids_path = directories::ProjectDirs::from("", "", "Twitty")
        .map(|d| d.cache_dir().join("client-ids.json"));
    let ids = ids_path.as_deref().map_or_else(ClientIds::random, ClientIds::load_or_create);
    let transport =
        Transport::new(session, &ids, Url::parse(Transport::API_BASE)?, RetryPolicy::default())?;
    Ok(XClient::new(transport, catalog, Some(Discovery::default_pages()?)))
}

/// Instruction types and the total number of timeline entries, wherever they are.
fn summarize(body: &Value) -> (Vec<String>, usize) {
    fn walk(v: &Value, types: &mut Vec<String>, entries: &mut usize) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Array(instructions)) = map.get("instructions") {
                    for i in instructions {
                        if let Some(t) = i.get("type").and_then(Value::as_str) {
                            types.push(t.to_owned());
                        }
                        if let Some(Value::Array(e)) = i.get("entries") {
                            *entries += e.len();
                        }
                    }
                }
                map.values().for_each(|v| walk(v, types, entries));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, types, entries)),
            _ => {}
        }
    }
    let (mut types, mut entries) = (Vec::new(), 0);
    walk(body, &mut types, &mut entries);
    (types, entries)
}
