use std::{path::PathBuf, process::exit, str::FromStr, sync::Arc};

use clap::Parser;
use config::ConfigProvider;
use log::LevelFilter;
use proxy::RaknetProxy;
use simple_logger::SimpleLogger;
use tokio::io::AsyncBufReadExt;

mod config;
mod health;
mod load_balancer;
mod metrics;
mod motd;
mod proxy;
mod raknet;

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
    let log_level = match args.verbose {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    SimpleLogger::new()
        .with_level(log_level)
        .with_colors(!args.no_color)
        .init()
        .unwrap();

    if args.raise_ulimit {
        match fdlimit::raise_fd_limit() {
            Ok(fdlimit::Outcome::LimitRaised { from, to }) => {
                log::info!("Raised ulimit from {} to {}", from, to);
            }
            Ok(fdlimit::Outcome::LimitNotRaised { limit }) => {
                log::info!("Ulimit already at {}", limit);
            }
            Err(err) => log::warn!("Failed to raise ulimit: {}", err),
        }
    }

    let config_file = args
        .config
        .as_ref()
        .map(PathBuf::clone)
        .unwrap_or_else(|| PathBuf::from_str("config.toml").unwrap());
    let config_provider = match config::read_config(config_file.clone()) {
        Ok(config) => config,
        Err(err) => {
            log::error!(
                "Could not read configuration file ({}): {}",
                config_file.to_string_lossy(),
                err
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
        log::debug!("Parsed configuration: {:#?}", config);
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
                log::info!("Console commands enabled");
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
                        log::info!("Shutdown requested...");
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
        log::error!("{}", err);
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
                log::error!("Error reading user input: {:?}", err);
                continue;
            }
        };
        let line = &buf[0..len].trim();
        match line.to_lowercase().as_str() {
            "reload" => config_provider.reload().await,
            "list" | "load" => {
                let overview = proxy.load_overview();
                log::info!(
                    "There are {} online players ({} active clients). Breakdown: {:?}",
                    overview.connected_count,
                    overview.client_count,
                    overview.per_server
                )
            }
            "metrics" | "stats" => {
                let snapshot = proxy.metrics().snapshot();
                log::info!("Metrics: {}", snapshot);
            }
            _ => log::warn!("Unknown command '{}'", line),
        }
    }
}
