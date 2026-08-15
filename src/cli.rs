//! CLI parsing and command dispatch.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{load_config, validate_config};
use crate::db::{check_connection, create_pool, migrate};
use crate::error::{AppError, ExitCode};
use crate::runtime::{RuntimeOptions, parse_roles, run};

/// zl-expense operator CLI.
#[derive(Debug, Parser)]
#[command(name = "zl-expense", version, about = "Zalo expense bot")]
pub struct Cli {
    /// Path to config.toml (non-secret settings).
    #[arg(long, global = true, env = "ZL_EXPENSE_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
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
    /// Run the supervised runtime.
    Run {
        /// Roles to run (default: all). Comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Validate resolved configuration.
    Validate,
    /// Show resolved configuration with source attribution (no secrets).
    Show,
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
    init_tracing();

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

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init()
        .ok();
}
