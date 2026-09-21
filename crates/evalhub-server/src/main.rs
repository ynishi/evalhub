//! The `evalhub` binary. See the crate docs of `evalhub_server` for the
//! design; this file is only the command-line surface.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use secrecy::ExposeSecret;
use tokio::net::TcpListener;
use tracing::{info, warn};

use evalhub_server::config::{Loaded, LogFormat, Overrides};

/// evalhub — a hosting service for LLM evaluation results.
#[derive(Debug, Parser)]
#[command(name = "evalhub", version, about)]
struct Cli {
    /// Path to a TOML config file. Overrides EVALHUB_CONFIG.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve(ServeArgs),
    /// Apply pending database migrations.
    Migrate(DbArgs),
    /// Inspect the effective configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[command(flatten)]
    db: DbArgs,
    /// S3-compatible endpoint for attachments.
    #[arg(long, env = "EVALHUB_S3__ENDPOINT")]
    s3_endpoint: Option<String>,
    /// Address to bind (default 127.0.0.1:8080).
    #[arg(long, env = "EVALHUB_BIND")]
    bind: Option<String>,
}

#[derive(Debug, Args)]
struct DbArgs {
    /// Postgres connection URL.
    #[arg(long, env = "EVALHUB_DATABASE__URL")]
    database_url: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the effective configuration.
    Show {
        /// Annotate each value with the layer it came from.
        #[arg(long)]
        origin: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => {
            let loaded = load(
                cli.config.as_deref(),
                Overrides {
                    bind: args.bind,
                    database_url: args.db.database_url,
                    s3_endpoint: args.s3_endpoint,
                },
            )?;
            init_tracing(loaded.config.log.format);
            serve(loaded).await
        }
        Command::Migrate(args) => {
            let loaded = load(
                cli.config.as_deref(),
                Overrides {
                    database_url: args.database_url,
                    ..Overrides::default()
                },
            )?;
            init_tracing(loaded.config.log.format);
            migrate(loaded).await
        }
        Command::Config {
            command: ConfigCommand::Show { origin },
        } => {
            let loaded = load(cli.config.as_deref(), Overrides::default())?;
            show(&loaded, origin);
            Ok(())
        }
    }
}

fn load(file: Option<&std::path::Path>, overrides: Overrides) -> anyhow::Result<Loaded> {
    Loaded::load(file, overrides).context("loading configuration")
}

fn init_tracing(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    match format {
        LogFormat::Text => tracing_subscriber::fmt().with_env_filter(filter).init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init(),
    }
}

async fn connect(loaded: &Loaded) -> anyhow::Result<Option<evalhub_store::PgPool>> {
    let Some(url) = &loaded.config.database.url else {
        return Ok(None);
    };
    let pool =
        evalhub_store::pool::connect(url.expose_secret(), loaded.config.database.max_connections)
            .await
            .context("connecting to the database")?;
    Ok(Some(pool))
}

async fn serve(loaded: Loaded) -> anyhow::Result<()> {
    let db = connect(&loaded).await?;
    match &db {
        Some(pool) => {
            let pending = evalhub_store::pool::pending_migrations(pool)
                .await
                .context("checking migrations")?;
            if !pending.is_empty() {
                anyhow::bail!(
                    "database has pending migrations {pending:?}; run `evalhub migrate` first"
                );
            }
            info!("database connected, schema current");
        }
        None => warn!("no database configured; only meta endpoints are served"),
    }

    let bind = loaded.config.bind.clone();
    let app = evalhub_server::api::router(Arc::new(loaded.config), db);
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    info!(%bind, "evalhub listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;
    info!("evalhub stopped");
    Ok(())
}

async fn migrate(loaded: Loaded) -> anyhow::Result<()> {
    let Some(pool) = connect(&loaded).await? else {
        anyhow::bail!(
            "no database configured (set database.url, EVALHUB_DATABASE__URL, or --database-url)"
        );
    };
    evalhub_store::pool::migrate(&pool)
        .await
        .context("applying migrations")?;
    info!("migrations applied");
    Ok(())
}

fn show(loaded: &Loaded, origin: bool) {
    for e in loaded.entries() {
        if origin {
            println!("{:<36} = {:<24} # {}", e.key, e.value, e.origin);
        } else {
            println!("{:<36} = {}", e.key, e.value);
        }
    }
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        warn!(error = %e, "failed to listen for ctrl-c; running without graceful shutdown");
        std::future::pending::<()>().await;
    }
    info!("shutdown signal received");
}
