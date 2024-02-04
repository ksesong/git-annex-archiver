use clap::{Parser, Subcommand};
use commands::LogTarget;
use std::path::PathBuf;
use tokio::io::{self};

use crate::commands::maintain::maintain;
use crate::commands::sync::sync;

#[cfg(not(target_os = "linux"))]
use crate::daemon::run_daemon;

pub mod commands;
pub mod format;
pub mod types;
pub mod platform;

#[cfg(not(target_os = "linux"))]
pub mod daemon;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a daemon running archiving tasks on schedule
    Daemon,
    /// Sync a repository with its remote, including files
    Sync {
        #[arg(short, long, num_args = 1.., required = true)]
        repo_paths: Vec<String>,

        #[arg(long)]
        all: bool,
    },
    /// Run maintenance tasks, checking a repository integrity, including previous versions
    Maintain {
        #[arg(short, long, num_args = 1.., required = true)]
        repo_paths: Vec<String>,

        #[arg(short, long, required = true)]
        timeout: u64,
    },
}

async fn setup_daemon() {
    #[cfg(not(target_os = "linux"))]
    run_daemon().await;
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match args.command {
        Some(Commands::Daemon) => {
            setup_daemon().await;
        }
        Some(Commands::Sync { repo_paths, all }) => {
            sync(
                &repo_paths.into_iter().map(|x| PathBuf::from(&x)).collect(),
                all,
                &mut LogTarget::Stdout(&mut io::stdout()),
                |_| {}
            )
            .await
            .unwrap();
        }
        Some(Commands::Maintain {
            repo_paths,
            timeout,
        }) => {
            maintain(
                &repo_paths.into_iter().map(|x| PathBuf::from(&x)).collect(),
                timeout,
                (
                    &mut LogTarget::Stdout(&mut io::stdout()),
                    &mut LogTarget::Stdout(&mut io::stdout()),
                ),
                |_| {},
            )
            .await
            .unwrap();
        }
        None => {
            setup_daemon().await;
        }
    };
}
