//! `speakeasy` CLI. Every command prints one JSON document per line on stdout.
//! Errors are printed as JSON on stderr. Exit codes: 0 success, 1 error,
//! 2 usage, 3 verification/rejection (bad signature, rollback, equivocation),
//! 4 unavailable (no peer reachable or no peer has the feed).

use std::{io::Read, net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use speakeasy::{
    manifest::Limits,
    net::{NetConfig, NetworkMode, Node, open_store},
    ops::{self, FailureKind},
    repo::Repo,
};

#[derive(Parser)]
#[command(
    name = "speakeasy",
    version,
    about = "Publish locally. Share by invitation."
)]
struct Cli {
    /// Runtime root (keys, blob store, feeds). Never place it inside a git checkout.
    #[arg(long, env = "SPEAKEASY_ROOT", global = true)]
    root: Option<PathBuf>,
    /// Network mode: `n0` uses n0 relays and pkarr/DNS lookup; `local` uses neither.
    #[arg(long, value_enum, default_value = "n0", global = true)]
    network: NetworkMode,
    /// UDP socket address to bind (default: any interface, random port).
    #[arg(long, global = true)]
    bind: Option<SocketAddr>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the runtime root and node key; `--publisher` also creates a publisher signing key.
    Init {
        #[arg(long)]
        publisher: bool,
    },
    /// Publish the contents of a directory as the next version of a feed.
    Publish {
        #[arg(long)]
        feed: String,
        #[arg(long)]
        from: PathBuf,
        #[arg(long, default_value = "application/octet-stream")]
        content_type: String,
        /// Producer-defined schema/compatibility identifier (opaque).
        #[arg(long, default_value = "")]
        schema: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Producer-defined metadata, KEY=VALUE (repeatable).
        #[arg(long = "attr", value_parser = parse_kv)]
        attributes: Vec<(String, String)>,
    },
    /// Print a shareable descriptor (publisher key, feed name, peer hints).
    Descriptor {
        feed: String,
        /// Peer hint to include (endpoint ticket or id; repeatable).
        #[arg(long)]
        peer: Vec<String>,
    },
    /// Subscribe using a descriptor file (`-` for stdin).
    Subscribe {
        descriptor: PathBuf,
        #[arg(long)]
        peer: Vec<String>,
    },
    /// Fetch, verify and install the latest version of a feed from peers.
    Sync {
        feed: String,
        /// Peers to try instead of the stored hints (repeatable).
        #[arg(long)]
        peer: Vec<String>,
        /// Per-peer timeout in seconds.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Serve installed feeds to peers until interrupted.
    Serve {
        /// Also sync all subscribed feeds every N seconds.
        #[arg(long)]
        sync_every: Option<u64>,
    },
    /// List feeds in this root.
    List,
    /// Show installed version and sync status.
    Status { feed: Option<String> },
    /// Show a stored signed manifest (default: installed version).
    Inspect {
        feed: String,
        #[arg(long)]
        sequence: Option<u64>,
    },
    /// Print verified local paths for the installed version.
    Export { feed: String },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got {s:?}"))
}

fn print<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn default_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME not set; pass --root")?;
    Ok(PathBuf::from(home).join(".speakeasy"))
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let kind = ops::failure_kind(&err);
            let body = serde_json::json!({
                "error": format!("{err:#}"),
                "kind": kind,
            });
            eprintln!("{body}");
            match kind {
                Some(FailureKind::Verification) => ExitCode::from(3),
                Some(FailureKind::Unavailable) => ExitCode::from(4),
                Some(FailureKind::Local) | None => ExitCode::from(1),
            }
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let root = match cli.root {
        Some(r) => r,
        None => default_root()?,
    };
    let net = NetConfig {
        mode: cli.network,
        bind: cli.bind,
    };
    let limits = Limits::default();
    match cli.command {
        Command::Init { publisher } => {
            let repo = Repo::init(&root, publisher)?;
            let node = iroh::SecretKey::from_bytes(&repo.node_secret()?).public();
            let publisher = repo
                .publisher_key()?
                .map(|k| hex::encode(k.verifying_key().to_bytes()));
            print(&serde_json::json!({
                "root": repo.root(),
                "endpoint_id": node.to_string(),
                "publisher": publisher,
            }))
        }
        Command::Publish {
            feed,
            from,
            content_type,
            schema,
            description,
            attributes,
        } => {
            let repo = Repo::open(&root)?;
            let (store, _lock) = open_store(&repo).await?;
            let res = ops::publish(
                &repo,
                &store,
                ops::PublishOptions {
                    feed,
                    from,
                    content_type,
                    schema,
                    description,
                    attributes,
                },
                &limits,
            )
            .await;
            store.shutdown().await.ok();
            print(&res?)
        }
        Command::Descriptor { feed, peer } => {
            let repo = Repo::open(&root)?;
            print(&ops::descriptor(&repo.resolve_feed(&feed)?, &peer)?)
        }
        Command::Subscribe { descriptor, peer } => {
            let repo = Repo::open(&root)?;
            let text = if descriptor.as_os_str() == "-" {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(&descriptor)?
            };
            let desc: ops::Descriptor = serde_json::from_str(&text).context("parse descriptor")?;
            let feed = ops::subscribe(&repo, &desc, &peer)?;
            print(&ops::status(&feed)?)
        }
        Command::Sync {
            feed,
            peer,
            timeout,
        } => {
            let repo = Repo::open(&root)?;
            let feed = repo.resolve_feed(&feed)?;
            let node = Node::open(repo, &net).await?;
            let res =
                ops::sync_feed(&node, &feed, &peer, &limits, Duration::from_secs(timeout)).await;
            node.shutdown().await.ok();
            print(&res?)
        }
        Command::Serve { sync_every } => {
            let repo = Repo::open(&root)?;
            let node = Node::open(repo.clone(), &net).await?;
            for feed in repo.feeds()? {
                feed.recover(&limits)?;
            }
            let ticket = node.ticket().await;
            print(&serde_json::json!({
                "serving": true,
                "endpoint_id": node.endpoint().id().to_string(),
                "ticket": ticket.to_string(),
                "feeds": repo.feeds()?.iter().map(|f| f.id.to_hex()).collect::<Vec<_>>(),
            }))?;
            let res = serve_loop(&node, &repo, sync_every, &limits).await;
            node.shutdown().await.ok();
            res
        }
        Command::List => {
            let repo = Repo::open(&root)?;
            let feeds = repo
                .feeds()?
                .iter()
                .map(ops::status)
                .collect::<Result<Vec<_>>>()?;
            print(&feeds)
        }
        Command::Status { feed } => {
            let repo = Repo::open(&root)?;
            match feed {
                Some(f) => print(&ops::status(&repo.resolve_feed(&f)?)?),
                None => print(
                    &repo
                        .feeds()?
                        .iter()
                        .map(ops::status)
                        .collect::<Result<Vec<_>>>()?,
                ),
            }
        }
        Command::Inspect { feed, sequence } => {
            let repo = Repo::open(&root)?;
            print(&ops::inspect(
                &repo.resolve_feed(&feed)?,
                sequence,
                &limits,
            )?)
        }
        Command::Export { feed } => {
            let repo = Repo::open(&root)?;
            print(&ops::export(&repo.resolve_feed(&feed)?)?)
        }
    }
}

async fn serve_loop(
    node: &Node,
    repo: &Repo,
    sync_every: Option<u64>,
    limits: &Limits,
) -> Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let period = Duration::from_secs(sync_every.unwrap_or(u64::MAX / 4).max(1));
    let mut tick = tokio::time::interval(period);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = term.recv() => return Ok(()),
            _ = tick.tick(), if sync_every.is_some() => {
                for feed in repo.feeds()? {
                    if feed.subscription()?.role != speakeasy::repo::Role::Subscriber {
                        continue;
                    }
                    let res = ops::sync_feed(node, &feed, &[], limits, Duration::from_secs(300)).await;
                    let line = match res {
                        Ok(r) => serde_json::to_value(r)?,
                        Err(e) => serde_json::json!({
                            "feed_id": feed.id.to_hex(),
                            "error": format!("{e:#}"),
                            "kind": ops::failure_kind(&e),
                        }),
                    };
                    println!("{line}");
                }
            }
        }
    }
}
