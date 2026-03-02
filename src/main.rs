mod chunk;
mod config;
mod embed;
mod index;
mod search;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "rag", about = "Semantic code search over OpenData")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index the codebase into the vector database
    Index {
        /// Path to config file
        #[arg(short, long, default_value = "rag.yaml")]
        config: String,
        /// Dry run: chunk and estimate tokens without calling the embedding API
        #[arg(long)]
        dry_run: bool,
    },
    /// Search the indexed codebase
    Search {
        /// Path to config file
        #[arg(short, long, default_value = "rag.yaml")]
        config: String,
        /// Search query
        query: String,
        /// Number of results to return
        #[arg(short, long, default_value = "10")]
        k: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Index { config, dry_run } => {
            let cfg = config::RagConfig::load(&config)?;
            if dry_run {
                index::dry_run(&cfg)?;
            } else {
                index::run(&cfg).await?;
            }
        }
        Command::Search { config, query, k } => {
            let cfg = config::RagConfig::load(&config)?;
            search::run(&cfg, &query, k).await?;
        }
    }

    Ok(())
}
