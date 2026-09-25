//! The `evalhub` binary. See the crate docs of `evalhub_server` for the
//! design; this file is only the command-line surface.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    /// Apply pending database migrations: the SQL schema, then the
    /// one-shot data migrations, each printed as it is applied.
    Migrate(DbArgs),
    /// Inspect the effective configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Manage users (bootstrap; there is no signup endpoint).
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
}

#[derive(Debug, Subcommand)]
enum UserCommand {
    /// Create a user, its personal namespace, and a first token. The
    /// token secret is printed once and never again.
    Create {
        #[command(flatten)]
        db: DbArgs,
        /// Login; also the user's personal namespace.
        login: String,
        /// Scope of the first token.
        #[arg(long, default_value = "write", value_parser = ["read", "write", "admin"])]
        scope: String,
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
        Command::User {
            command: UserCommand::Create { db, login, scope },
        } => {
            let loaded = load(
                cli.config.as_deref(),
                Overrides {
                    database_url: db.database_url,
                    ..Overrides::default()
                },
            )?;
            init_tracing(loaded.config.log.format);
            user_create(loaded, &login, &scope).await
        }
    }
}

async fn user_create(loaded: Loaded, login: &str, scope: &str) -> anyhow::Result<()> {
    let pool = require_db(&loaded).await?;
    let user_id = evalhub_store::auth::create_user(&pool, login)
        .await
        .context("creating user")?;
    let (secret, hash) = evalhub_server::auth::new_secret();
    let scope = evalhub_store::auth::Scope::parse(scope);
    let token_id =
        evalhub_store::auth::create_token(&pool, user_id, scope, &[login.to_string()], &hash)
            .await
            .context("creating token")?;
    info!(%login, %user_id, %token_id, "user created");
    println!("login:    {login}");
    println!("scope:    {}", scope.as_str());
    println!("token:    {secret}");
    println!("(the token is shown once; the hub keeps only its hash)");
    Ok(())
}

async fn require_db(loaded: &Loaded) -> anyhow::Result<evalhub_store::PgPool> {
    connect(loaded).await?.ok_or_else(|| {
        anyhow::anyhow!(
            "no database configured (set database.url, EVALHUB_DATABASE__URL, or --database-url)"
        )
    })
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

/// Build the object store when `s3.endpoint` and both credentials are
/// configured. Anything less leaves attachments unavailable rather than
/// half-configured, and says so once at start-up.
fn object_store(loaded: &Loaded) -> anyhow::Result<Option<Arc<evalhub_store::objects::Objects>>> {
    let s3 = &loaded.config.s3;
    let (Some(endpoint), Some(access_key), Some(secret_key)) =
        (&s3.endpoint, &s3.access_key, &s3.secret_key)
    else {
        warn!("no object storage configured; the attachment endpoints answer 503");
        return Ok(None);
    };
    let objects = evalhub_store::objects::Objects::new(evalhub_store::objects::ObjectConfig {
        endpoint: endpoint.clone(),
        public_endpoint: s3.public_endpoint.clone(),
        bucket: s3.bucket.clone(),
        access_key: access_key.expose_secret().to_string(),
        secret_key: secret_key.expose_secret().to_string(),
        region: s3.region.clone(),
        path_style: s3.path_style,
        allow_http: s3.allow_http,
        presign_ttl: Duration::from_secs(s3.presign_ttl_secs),
    })
    .context("building the object store client")?;
    info!(bucket = %s3.bucket, "object storage configured");
    Ok(Some(Arc::new(objects)))
}

async fn serve(loaded: Loaded) -> anyhow::Result<()> {
    // The record API needs a database; a server without one has nothing
    // to serve but its own health, so it refuses to start.
    let pool = require_db(&loaded).await?;
    let pending = evalhub_store::pool::pending_migrations(&pool)
        .await
        .context("checking migrations")?;
    // `pending` covers the SQL migrations and the one-shot data
    // migrations alike, so a database whose DDL is current but whose data
    // step has not run is refused here too.
    if !pending.is_empty() {
        let list: Vec<String> = pending.iter().map(ToString::to_string).collect();
        anyhow::bail!(
            "database has pending migrations [{}]; run `evalhub migrate` first",
            list.join(", ")
        );
    }
    info!("database connected, schema current");

    if loaded.config.auth.cookie_key.is_none() {
        warn!(
            "auth.cookie_key is unset; UI sessions are encrypted with a key \
             generated for this process and end when it restarts"
        );
    }

    let objects = object_store(&loaded)?;
    // The garbage collector sweeps every hour; the grace period is what
    // keeps it from collecting an upload that is ready but not yet
    // referenced by the record being written.
    let gc = objects.as_ref().map(|objects| {
        evalhub_server::jobs::spawn_attachment_gc(
            pool.clone(),
            objects.clone(),
            Duration::from_secs(loaded.config.jobs.gc_grace_secs),
            Duration::from_secs(60 * 60),
        )
    });

    // The query vocabulary: facet paths from the committed schema now,
    // extension paths as the index builder applies them.
    let path_tables = evalhub_server::state::PathTableHandle::from_schema();
    match evalhub_store::registry::ext_schemas_applied(&pool).await {
        Ok(entries) => {
            path_tables
                .rebuild(&evalhub_server::jobs::to_query_ext(&entries))
                .await;
        }
        Err(e) => warn!(error = %format!("{e:#}"), "loading extension schemas failed"),
    }
    // The index builder finishes extension schemas registered while the
    // server was down and republishes the tables; the badge sweep brings
    // registry-derived badges up to date on versions stored before their
    // entry existed.
    let index_build = evalhub_server::jobs::spawn_index_build(
        pool.clone(),
        path_tables.clone(),
        Duration::from_secs(30),
    );
    let badge_recompute =
        evalhub_server::jobs::spawn_badge_recompute(pool.clone(), Duration::from_secs(5 * 60));

    let bind = loaded.config.bind.clone();
    let app =
        evalhub_server::api::router(Arc::new(loaded.config), Some(pool), objects, path_tables);
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    info!(%bind, "evalhub listening");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving");
    // The sweep is idempotent, so cutting it at an await point costs
    // nothing; the next start picks up whatever was left.
    if let Some(gc) = gc {
        gc.abort();
    }
    index_build.abort();
    badge_recompute.abort();
    result?;
    info!("evalhub stopped");
    Ok(())
}

async fn migrate(loaded: Loaded) -> anyhow::Result<()> {
    let Some(pool) = connect(&loaded).await? else {
        anyhow::bail!(
            "no database configured (set database.url, EVALHUB_DATABASE__URL, or --database-url)"
        );
    };
    let applied = evalhub_store::pool::migrate(&pool)
        .await
        .context("applying migrations")?;
    // One line per data migration applied, on stdout, so the release
    // command's log says what the data step did; a second run prints that
    // there was nothing to do.
    if applied.is_empty() {
        println!("no data migration pending");
    }
    for a in &applied {
        println!("applied data migration {}: {}", a.name, a.summary);
    }
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
