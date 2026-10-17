use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use rootstock::{client, mcp, repo::Repo, server};

const DEFAULT_URL: &str = "http://127.0.0.1:7420";

#[derive(Parser)]
#[command(name = "rootstock", version, about = "Version control server for agent swarms, git underneath")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a data directory from the tip of a branch of an existing git repo.
    Init {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        from: PathBuf,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// Serve the HTTP API.
    Serve {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7420")]
        addr: String,
    },
    /// MCP server on stdio that forwards tool calls to a running rootstock.
    Mcp {
        #[arg(long, env = "ROOTSTOCK_URL", default_value = DEFAULT_URL)]
        url: String,
        /// Default agent name for open_change and revert.
        #[arg(long, env = "ROOTSTOCK_AGENT")]
        agent: Option<String>,
    },
    /// Call one operation, e.g. `rootstock call read '{"path":"README.md"}'`.
    Call {
        op: String,
        params: Option<String>,
        #[arg(long, env = "ROOTSTOCK_URL", default_value = DEFAULT_URL)]
        url: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init { data, from, branch } => {
            let root = Repo::init(&data, &from, &branch)?;
            println!("imported {branch} from {} as changeset {}", from.display(), &root[..12]);
        }
        Cmd::Serve { data, addr } => {
            let repo = Arc::new(Repo::open(&data)?);
            tokio::runtime::Runtime::new()?.block_on(server::serve(repo, &addr))?;
        }
        Cmd::Mcp { url, agent } => mcp::run(&url, agent)?,
        Cmd::Call { op, params, url } => {
            let params: Value = match params {
                Some(p) => serde_json::from_str(&p).context("params must be JSON")?,
                None => json!({}),
            };
            let v = client::call(&url, &op, params)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
    }
    Ok(())
}
