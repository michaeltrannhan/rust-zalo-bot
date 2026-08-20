//! CLI parsing and command dispatch.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::config::{load_config, validate_config};
use crate::db::{check_connection, create_pool, migrate};
use crate::error::{AppError, ExitCode};
use crate::operator::{
    ActiveProbe, run_backup, run_diagnose, run_doctor, run_ingress_status, run_ingress_switch,
    run_jobs_cancel, run_jobs_list, run_jobs_retry, run_jobs_show, run_logs, run_restore,
    run_status,
};
use crate::runtime::{RuntimeOptions, parse_roles, run};
use crate::update::{ApplyOptions, UpdatePaths, run_apply, run_preflight, run_rollback};
use crate::work::WorkStore;

/// zl-expense operator CLI.
#[derive(Debug, Parser)]
#[command(name = "zl-expense", version, about = "Zalo expense bot")]
pub struct Cli {
    /// Path to config.toml (non-secret settings).
    #[arg(long, global = true, env = "ZL_EXPENSE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Log format: pretty (default) or json.
    #[arg(
        long,
        global = true,
        env = "ZL_EXPENSE_LOG_FORMAT",
        default_value = "pretty"
    )]
    pub log_format: LogFormat,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum, PartialEq, Eq)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Configuration commands.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Database commands.
    Db {
        #[command(subcommand)]
        command: DbCommands,
    },
    /// Print health and queue status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Inspect and recover durable jobs.
    Jobs {
        #[command(subcommand)]
        command: JobsCommands,
    },
    /// Passive and optional active dependency checks.
    Doctor {
        #[arg(long)]
        active: Option<DoctorActive>,
    },
    /// Ingress mode inspection and switching.
    Ingress {
        #[command(subcommand)]
        command: IngressCommands,
    },
    /// Create a PostgreSQL custom-format backup.
    Backup {
        #[arg(long)]
        output: PathBuf,
    },
    /// Restore a PostgreSQL backup (requires --yes).
    Restore {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Tail service logs via journald when available.
    Logs {
        #[arg(long, short)]
        follow: bool,
        #[arg(long)]
        since: Option<String>,
    },
    /// Write a redacted operator diagnostic bundle.
    Diagnose {
        #[arg(long)]
        output: PathBuf,
    },
    /// Signed update preflight, apply, and rollback.
    Update {
        #[command(subcommand)]
        command: UpdateCommands,
    },
    /// Run the supervised runtime.
    Run {
        /// Roles to run (default: all). Comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum DoctorActive {
    Zalo,
    Gemini,
    #[value(name = "object-store")]
    ObjectStore,
}

impl From<DoctorActive> for ActiveProbe {
    fn from(value: DoctorActive) -> Self {
        match value {
            DoctorActive::Zalo => ActiveProbe::Zalo,
            DoctorActive::Gemini => ActiveProbe::Gemini,
            DoctorActive::ObjectStore => ActiveProbe::ObjectStore,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum JobsCommands {
    /// List jobs (redacted summary only).
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Show one job and its attempts.
    Show { id: Uuid },
    /// Requeue a dead job.
    Retry { id: Uuid },
    /// Cancel a queued or leased job.
    Cancel { id: Uuid },
}

#[derive(Debug, Subcommand)]
pub enum IngressCommands {
    /// Print ingress mode and generation.
    Status,
    /// Switch to webhook ingress.
    Webhook,
    /// Switch to polling ingress.
    Poll,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Validate resolved configuration.
    Validate,
    /// Show resolved configuration with source attribution (no secrets).
    Show,
}

#[derive(Debug, Subcommand)]
pub enum UpdateCommands {
    /// Verify signature, checksum, and schema compatibility.
    Preflight {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        metadata: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        #[arg(long)]
        public_key: Vec<PathBuf>,
        #[arg(long)]
        install_path: Option<PathBuf>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Apply a signed update (requires --yes).
    Apply {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        metadata: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        #[arg(long)]
        public_key: Vec<PathBuf>,
        #[arg(long)]
        install_path: Option<PathBuf>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        skip_backup: bool,
        #[arg(long)]
        skip_migrate: bool,
        #[arg(long)]
        skip_health: bool,
        #[arg(long)]
        health_url: Option<String>,
    },
    /// Restore the previous binary when schema compatibility allows.
    Rollback {
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        install_path: Option<PathBuf>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DbCommands {
    /// Check database connectivity.
    Check,
    /// Apply pending migrations.
    Migrate,
}

/// Testable execution entrypoint for CLI commands.
pub async fn execute(cli: Cli) -> ExitCode {
    init_tracing(cli.log_format);

    match run_command(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err.to_json_line());
            err.exit_code()
        }
    }
}

async fn run_command(cli: Cli) -> Result<ExitCode, AppError> {
    let config_path = cli.config.as_deref();

    match cli.command {
        Commands::Config { command } => match command {
            ConfigCommands::Validate => validate_config(config_path)?,
            ConfigCommands::Show => {
                let resolved = load_config(config_path)?;
                println!("{}", resolved.show_json());
            }
        },
        Commands::Db { command } => {
            let resolved = load_config(config_path)?;
            let pool = create_pool(&resolved).await?;
            match command {
                DbCommands::Check => check_connection(&pool).await?,
                DbCommands::Migrate => migrate(&pool).await?,
            }
        }
        Commands::Status { json } => {
            let resolved = load_config(config_path)?;
            let pool = create_pool(&resolved).await?;
            run_status(&pool, json).await?;
        }
        Commands::Jobs { command } => {
            let resolved = load_config(config_path)?;
            let pool = create_pool(&resolved).await?;
            let store = WorkStore::new(pool);
            match command {
                JobsCommands::List { state, limit, json } => {
                    run_jobs_list(&store, state.as_deref(), limit, json).await?
                }
                JobsCommands::Show { id } => run_jobs_show(&store, id).await?,
                JobsCommands::Retry { id } => run_jobs_retry(&store, id).await?,
                JobsCommands::Cancel { id } => run_jobs_cancel(&store, id).await?,
            }
        }
        Commands::Doctor { active } => {
            run_doctor(config_path, active.map(ActiveProbe::from)).await?;
        }
        Commands::Ingress { command } => {
            let resolved = load_config(config_path)?;
            let pool = create_pool(&resolved).await?;
            match command {
                IngressCommands::Status => run_ingress_status(&pool).await?,
                IngressCommands::Webhook => run_ingress_switch(&pool, "webhook").await?,
                IngressCommands::Poll => run_ingress_switch(&pool, "poll").await?,
            }
        }
        Commands::Backup { output } => {
            let resolved = load_config(config_path)?;
            run_backup(&resolved, &output)?;
        }
        Commands::Restore { input, yes } => {
            let resolved = load_config(config_path)?;
            run_restore(&resolved, &input, yes)?;
        }
        Commands::Logs { follow, since } => {
            run_logs(follow, since.as_deref())?;
        }
        Commands::Diagnose { output } => {
            let resolved = load_config(config_path)?;
            run_diagnose(&resolved, config_path, &output).await?;
        }
        Commands::Update { command } => {
            let resolved = load_config(config_path)?;
            match command {
                UpdateCommands::Preflight {
                    artifact,
                    metadata,
                    signature,
                    public_key,
                    install_path,
                    state_dir,
                } => {
                    let pool = create_pool(&resolved).await?;
                    let paths = update_paths(
                        &resolved,
                        artifact,
                        metadata,
                        signature,
                        public_key,
                        install_path,
                        state_dir,
                    );
                    run_preflight(&pool, &paths).await?;
                }
                UpdateCommands::Apply {
                    artifact,
                    metadata,
                    signature,
                    public_key,
                    install_path,
                    state_dir,
                    yes,
                    skip_backup,
                    skip_migrate,
                    skip_health,
                    health_url,
                } => {
                    let pool = create_pool(&resolved).await?;
                    let paths = update_paths(
                        &resolved,
                        artifact,
                        metadata,
                        signature,
                        public_key,
                        install_path,
                        state_dir,
                    );
                    run_apply(
                        &pool,
                        &resolved,
                        &paths,
                        &ApplyOptions {
                            yes,
                            skip_backup,
                            skip_migrate,
                            skip_health,
                            health_url,
                        },
                    )
                    .await?;
                }
                UpdateCommands::Rollback {
                    yes,
                    install_path,
                    state_dir,
                } => {
                    let pool = create_pool(&resolved).await?;
                    let paths = update_paths(
                        &resolved,
                        PathBuf::new(),
                        PathBuf::new(),
                        PathBuf::new(),
                        Vec::new(),
                        install_path,
                        state_dir,
                    );
                    let schema = sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*)::BIGINT FROM _sqlx_migrations WHERE success = true",
                    )
                    .fetch_one(&pool)
                    .await
                    .map_err(|_| AppError::dependency("failed to read applied migration count"))?;
                    run_rollback(&paths, yes, schema)?;
                }
            }
        }
        Commands::Run { roles } => {
            let resolved = load_config(config_path)?;
            let parsed_roles = parse_roles(&roles)?;
            return Ok(run(
                resolved,
                RuntimeOptions {
                    roles: parsed_roles,
                },
            )
            .await);
        }
    }

    Ok(ExitCode::Success)
}

fn update_paths(
    config: &crate::config::ResolvedConfig,
    artifact: PathBuf,
    metadata: PathBuf,
    signature: PathBuf,
    public_key: Vec<PathBuf>,
    install_path: Option<PathBuf>,
    state_dir: Option<PathBuf>,
) -> UpdatePaths {
    UpdatePaths {
        artifact,
        metadata,
        signature,
        public_keys: public_key,
        public_keys_directory: config.update_public_keys_directory.clone(),
        install_path: install_path.unwrap_or_else(|| config.update_install_path.clone()),
        state_dir: state_dir.unwrap_or_else(|| config.update_state_directory.clone()),
    }
}

fn init_tracing(format: LogFormat) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .with_target(false)
                .try_init()
                .ok();
        }
        LogFormat::Pretty => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .without_time()
                .try_init()
                .ok();
        }
    }
}
