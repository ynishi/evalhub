//! Layered configuration with provenance.
//!
//! Configuration is assembled by `figment` from, in order of increasing
//! precedence:
//!
//! 1. built-in defaults (the `Default` impls below),
//! 2. a TOML file (`--config <path>` or `EVALHUB_CONFIG`; `evalhub.toml` in
//!    the working directory if neither is set and the file exists),
//! 3. environment variables prefixed `EVALHUB_` (nested keys joined with
//!    `__`, e.g. `EVALHUB_S3__ENDPOINT`),
//! 4. command-line flags.
//!
//! Every effective value remembers which layer set it, and `evalhub config
//! show --origin` prints the whole tree with that annotation. This exists
//! because "which setting is actually in effect" is the first question in
//! every deployment problem, and answering it by reading four files is a
//! waste.
//!
//! # Keys (v0)
//!
//! ```text
//! bind            = "127.0.0.1:8080"
//! database.url    = "postgres://…"            secret; optional until the record API lands
//! database.max_connections = 8
//! s3.endpoint     = "http://…"
//! s3.bucket       = "evalhub"
//! s3.access_key   =                            secret
//! s3.secret_key   =                            secret
//! s3.path_style   = true
//! s3.presign_ttl_secs = 900
//! auth.cursor_key =                            secret, HMAC key for cursors
//! auth.cookie_key =                            secret, private-cookie key for the UI
//! attachments.hash_verify_max_bytes = 268435456
//! jobs.gc_grace_secs = 86400
//! log.format      = "text" | "json"
//! ```
//!
//! Secrets are held in `secrecy::SecretString`, never printed by `Debug`,
//! and shown as `[redacted]` by `config show`; their *origin* is still
//! shown.
//!
//! `database.url` is optional only while the server has no endpoint that
//! needs a database. Once the record API exists, `serve` without a
//! database is a start-up error.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use secrecy::SecretString;
use serde::Deserialize;

/// The effective configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Socket address the HTTP server binds.
    pub bind: String,
    /// Postgres.
    pub database: Database,
    /// S3-compatible object storage.
    pub s3: S3,
    /// Keys the server signs with.
    pub auth: Auth,
    /// Attachment handling.
    pub attachments: Attachments,
    /// Background jobs.
    pub jobs: Jobs,
    /// Logging.
    pub log: Log,
}

/// `database.*`
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Database {
    /// Connection URL. Secret.
    pub url: Option<SecretString>,
    /// Pool size.
    pub max_connections: u32,
}

/// `s3.*`
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct S3 {
    /// Endpoint URL of the S3-compatible service.
    pub endpoint: Option<String>,
    /// Bucket that holds attachments.
    pub bucket: String,
    /// Access key. Secret.
    pub access_key: Option<SecretString>,
    /// Secret key. Secret.
    pub secret_key: Option<SecretString>,
    /// Use path-style addressing (required by MinIO, harmless elsewhere).
    pub path_style: bool,
    /// Lifetime of presigned URLs, in seconds.
    pub presign_ttl_secs: u64,
}

/// `auth.*`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// HMAC key for pagination cursors. Secret.
    pub cursor_key: Option<SecretString>,
    /// Key for the UI's private cookie. Secret.
    pub cookie_key: Option<SecretString>,
}

/// `attachments.*`
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Attachments {
    /// Objects up to this size are re-hashed by the hub on `complete`.
    pub hash_verify_max_bytes: u64,
}

/// `jobs.*`
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Jobs {
    /// How long an unreferenced object survives before GC, in seconds.
    pub gc_grace_secs: u64,
}

/// `log.*`
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Log {
    /// `text` or `json`.
    pub format: LogFormat,
}

/// Output format of the tracing subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable lines.
    Text,
    /// One JSON object per line.
    Json,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".into(),
            database: Database::default(),
            s3: S3::default(),
            auth: Auth::default(),
            attachments: Attachments::default(),
            jobs: Jobs::default(),
            log: Log::default(),
        }
    }
}

impl Default for Database {
    fn default() -> Self {
        Self {
            url: None,
            max_connections: 8,
        }
    }
}

impl Default for S3 {
    fn default() -> Self {
        Self {
            endpoint: None,
            bucket: "evalhub".into(),
            access_key: None,
            secret_key: None,
            path_style: true,
            presign_ttl_secs: 900,
        }
    }
}

impl Default for Attachments {
    fn default() -> Self {
        Self {
            hash_verify_max_bytes: 256 * 1024 * 1024,
        }
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            gc_grace_secs: 24 * 60 * 60,
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            format: LogFormat::Text,
        }
    }
}

/// Values given on the command line. Each overrides every other layer.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    /// `--bind`
    pub bind: Option<String>,
    /// `--database-url`
    pub database_url: Option<String>,
    /// `--s3-endpoint`
    pub s3_endpoint: Option<String>,
}

/// Why configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A config file was named explicitly and does not exist.
    #[error("config file not found: {0}")]
    FileNotFound(PathBuf),
    /// Parsing or merging failed.
    #[error("{0}")]
    Figment(Box<figment::Error>),
}

impl From<figment::Error> for ConfigError {
    fn from(e: figment::Error) -> Self {
        Self::Figment(Box::new(e))
    }
}

/// The loaded configuration together with the provenance of each value.
#[derive(Debug)]
pub struct Loaded {
    /// The effective values.
    pub config: Config,
    figment: Figment,
}

/// Environment variable naming the config file.
pub const CONFIG_PATH_ENV: &str = "EVALHUB_CONFIG";

/// Config file consulted when no path is given and it exists.
pub const DEFAULT_CONFIG_FILE: &str = "evalhub.toml";

impl Loaded {
    /// Assemble the configuration from defaults, file, environment and
    /// command-line overrides, in that order of precedence.
    ///
    /// `file` is the `--config` flag. If it is `None`, `EVALHUB_CONFIG` is
    /// consulted, then `evalhub.toml` in the working directory; an explicit
    /// path that does not exist is an error, an implicit one is skipped
    /// (figment's `Toml::file` skips silently, so existence is checked here).
    pub fn load(file: Option<&Path>, overrides: Overrides) -> Result<Self, ConfigError> {
        let mut figment = Figment::new();

        let explicit: Option<PathBuf> = file
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os(CONFIG_PATH_ENV).map(PathBuf::from));
        match explicit {
            Some(path) => {
                if !path.is_file() {
                    return Err(ConfigError::FileNotFound(path));
                }
                figment = figment.merge(Toml::file(path));
            }
            None => {
                if Path::new(DEFAULT_CONFIG_FILE).is_file() {
                    figment = figment.merge(Toml::file(DEFAULT_CONFIG_FILE));
                }
            }
        }

        figment = figment.merge(Env::prefixed("EVALHUB_").split("__").ignore(&["config"]));

        if let Some(v) = overrides.bind {
            figment = figment.merge(Serialized::default("bind", v));
        }
        if let Some(v) = overrides.database_url {
            figment = figment.merge(Serialized::default("database.url", v));
        }
        if let Some(v) = overrides.s3_endpoint {
            figment = figment.merge(Serialized::default("s3.endpoint", v));
        }

        let config: Config = figment.extract()?;
        Ok(Self { config, figment })
    }

    /// Where the value at `key` (dotted path) came from: the provider's
    /// name, or `default` when no layer set it.
    pub fn origin(&self, key: &str) -> String {
        self.figment
            .find_metadata(key)
            .map(|m| m.name.to_string())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Every key with its display value (secrets redacted) and origin, in
    /// a stable order. This is what `config show` prints.
    pub fn entries(&self) -> Vec<Entry> {
        let c = &self.config;
        let secret = |v: &Option<SecretString>| {
            if v.is_some() {
                "[redacted]".to_string()
            } else {
                "(unset)".to_string()
            }
        };
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "(unset)".to_string());
        let raw = [
            ("bind", c.bind.clone()),
            ("database.url", secret(&c.database.url)),
            (
                "database.max_connections",
                c.database.max_connections.to_string(),
            ),
            ("s3.endpoint", opt(&c.s3.endpoint)),
            ("s3.bucket", c.s3.bucket.clone()),
            ("s3.access_key", secret(&c.s3.access_key)),
            ("s3.secret_key", secret(&c.s3.secret_key)),
            ("s3.path_style", c.s3.path_style.to_string()),
            ("s3.presign_ttl_secs", c.s3.presign_ttl_secs.to_string()),
            ("auth.cursor_key", secret(&c.auth.cursor_key)),
            ("auth.cookie_key", secret(&c.auth.cookie_key)),
            (
                "attachments.hash_verify_max_bytes",
                c.attachments.hash_verify_max_bytes.to_string(),
            ),
            ("jobs.gc_grace_secs", c.jobs.gc_grace_secs.to_string()),
            (
                "log.format",
                match c.log.format {
                    LogFormat::Text => "text",
                    LogFormat::Json => "json",
                }
                .to_string(),
            ),
        ];
        raw.into_iter()
            .map(|(key, value)| Entry {
                origin: self.origin(key),
                key: key.to_string(),
                value,
            })
            .collect()
    }
}

/// One line of `config show`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Dotted key.
    pub key: String,
    /// Display value; secrets are `[redacted]`.
    pub value: String,
    /// Provider name or `default`.
    pub origin: String,
}
