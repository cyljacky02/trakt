use std::{path::PathBuf, process::exit, str::FromStr, sync::Arc};

use clap::Parser;
use config::ConfigProvider;
use proxy::RaknetProxy;
use tracing_subscriber::{EnvFilter, fmt};
use tracing_subscriber::prelude::*;
use tokio::io::AsyncBufReadExt;

mod config;
mod health;
mod load_balancer;
mod metrics;
mod motd;
mod proxy;
mod raknet;
mod ratelimit;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Configuration file.
    #[arg(short, long, value_name = "FILE", default_value = "config.toml")]
    config: Option<PathBuf>,
    /// Verbose level.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Disable reading from standard input for commands.
    #[arg(long)]
    ignore_stdin: bool,
    /// Disable colors from output.
    #[arg(long)]
    no_color: bool,
    /// Raise the maximum number of open files allowed to avoid issues.
    ///
    /// Not enabled by default as it may not work in all environments.
    #[arg(long)]
    raise_ulimit: bool,
}

fn main() {
    let args = Args::parse();
    let default_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer()
            .with_target(true)
            .with_ansi(!args.no_color))
        .init();

    if args.raise_ulimit {
        match fdlimit::raise_fd_limit() {
            Ok(fdlimit::Outcome::LimitRaised { from, to }) => {
                tracing::info!(from, to, "Raised ulimit");
            }
            Ok(_) => tracing::info!("Ulimit unchanged"),
            Err(err) => tracing::warn!(%err, "Failed to raise ulimit"),
        }
    }

    let config_file = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from_str("config.toml").unwrap());
    let config_provider = match config::read_config(config_file.clone()) {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(
                config_file = %config_file.to_string_lossy(),
                %err,
                "Could not read configuration file"
            );
            return;
        }
    };
    run(config_provider, args);
}

#[tokio::main]
async fn run(config_provider: ConfigProvider, args: Args) {
    let bind_address = {
        let config = config_provider.read().await;
        tracing::debug!(?config, "Parsed configuration");
        config.bind_address.clone()
    };
    let config_provider = Arc::new(config_provider);
    let proxy = RaknetProxy::bind(bind_address, config_provider.clone())
        .await
        .unwrap();
    if !args.ignore_stdin {
        tokio::spawn({
            let proxy = proxy.clone();
            let config_provider = config_provider.clone();
            async move {
                tracing::info!("Console commands enabled");
                run_stdin_handler(proxy, config_provider).await;
            }
        });
    }
    tokio::spawn({
        let proxy = proxy.clone();
        let config_provider = config_provider.clone();
        async move {
            let mut shutdown_requests = 0;
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        shutdown_requests += 1;
                        if shutdown_requests >= 3 {
                            exit(1);
                        }
                        tracing::info!("Shutdown requested...");
                        proxy.cleanup().await;
                        exit(0);
                    }
                    _ = config_provider.wait_reload() => {
                        proxy.reload_config().await;
                    }
                }
            }
        }
    });
    if let Err(err) = proxy.clone().run().await {
        tracing::error!(%err, "Proxy run failed");
    }
    proxy.cleanup().await;
}

async fn run_stdin_handler(proxy: Arc<RaknetProxy>, config_provider: Arc<ConfigProvider>) {
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        let mut buf = String::new();
        let len = match reader.read_line(&mut buf).await {
            Ok(line) => line,
            Err(err) => {
                tracing::error!(?err, "Error reading user input");
                continue;
            }
        };
        let line = &buf[0..len].trim();
        match line.to_lowercase().as_str() {
            "reload" => config_provider.reload().await,
            "list" | "load" => {
                let overview = proxy.load_overview();
                tracing::info!(
                    connected_count = overview.connected_count,
                    client_count = overview.client_count,
                    ?overview.per_server,
                    "Load overview"
                )
            }
            "metrics" | "stats" => {
                let snapshot = proxy.metrics().snapshot();
                tracing::info!(%snapshot, "Metrics");
            }
            _ => tracing::warn!(command = %line, "Unknown command"),
        }
    }
}
