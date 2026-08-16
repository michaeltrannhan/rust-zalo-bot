//! Passive and active operator health checks.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::config::{ResolvedConfig, validate_config};
use crate::db::{check_connection, check_migrations_current, create_pool};
use crate::error::{AppError, ErrorClass};
use crate::receipt::build_object_store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveProbe {
    Zalo,
    Gemini,
    ObjectStore,
}

#[derive(Debug, Serialize)]
struct DoctorWarning {
    error_class: &'static str,
    message: String,
}

pub async fn run_doctor(
    config_path: Option<&Path>,
    active: Option<ActiveProbe>,
) -> Result<(), AppError> {
    let mut hard_fail = false;
    let mut warnings: Vec<DoctorWarning> = Vec::new();

    let config = match crate::config::load_config(config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", error.to_json_line());
            return Err(error);
        }
    };

    if let Err(error) = validate_config(config_path) {
        hard_fail = true;
        eprintln!("{}", error.to_json_line());
    }

    let pool = match create_pool(&config).await {
        Ok(pool) => Some(pool),
        Err(error) => {
            hard_fail = true;
            eprintln!("{}", error.to_json_line());
            None
        }
    };

    if let Some(pool) = &pool {
        if check_connection(pool).await.is_err() {
            hard_fail = true;
            eprintln!(
                "{}",
                AppError::dependency("database connection failed").to_json_line()
            );
        } else if !check_migrations_current(pool).await.unwrap_or(false) {
            warnings.push(DoctorWarning {
                error_class: ErrorClass::Migration.as_str(),
                message: "database migrations are not current".to_string(),
            });
        }
    }

    if let Ok(entries) = fs::read_dir(&config.credentials_directory) {
        let mut names = Vec::new();
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort();
        println!("credential_files: {}", names.join(", "));
    } else {
        warnings.push(DoctorWarning {
            error_class: ErrorClass::Config.as_str(),
            message: "credentials directory is unreadable".to_string(),
        });
    }

    if !is_loopback_listen(&config.listen_address) {
        warnings.push(DoctorWarning {
            error_class: ErrorClass::HealthFailed.as_str(),
            message: "listen_address is not loopback".to_string(),
        });
    }

    if config.metrics_enabled && !is_loopback_listen(&config.listen_address) {
        warnings.push(DoctorWarning {
            error_class: ErrorClass::HealthFailed.as_str(),
            message: "metrics enabled with non-loopback listener".to_string(),
        });
    }

    println!(
        "kill_switches: extraction_enabled={} outbound_enabled={} insights_llm_enabled={}",
        config.extraction_enabled, config.outbound_enabled, config.insights_llm_enabled
    );

    for warning in &warnings {
        println!(
            "{}",
            serde_json::to_string(warning)
                .map_err(|_| AppError::internal("warning serialization failed"))?
        );
    }

    if let Some(probe) = active {
        run_active_probe(&config, probe).await?;
    }

    if hard_fail {
        return Err(AppError::dependency("doctor detected hard failures"));
    }
    Ok(())
}

async fn run_active_probe(config: &ResolvedConfig, probe: ActiveProbe) -> Result<(), AppError> {
    match probe {
        ActiveProbe::Zalo => {
            eprintln!(
                "warning: active probe makes an external HTTP call and may consume provider quota"
            );
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| AppError::dependency("http client unavailable"))?;
            let url = format!("{}/", config.zalo_api_base.trim_end_matches('/'));
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|_| AppError::dependency("zalo api probe failed"))?;
            println!("zalo_probe_status: {}", response.status().as_u16());
        }
        ActiveProbe::Gemini => {
            eprintln!(
                "warning: active probe may make an external HTTP call and consume provider quota"
            );
            if !is_loopback_url(&config.gemini_api_base) {
                return Err(AppError::usage(
                    "live provider tests are opt-in; gemini api_base must be loopback for doctor --active gemini",
                ));
            }
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| AppError::dependency("http client unavailable"))?;
            let url = format!("{}/", config.gemini_api_base.trim_end_matches('/'));
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|_| AppError::dependency("gemini api probe failed"))?;
            println!("gemini_probe_status: {}", response.status().as_u16());
        }
        ActiveProbe::ObjectStore => {
            eprintln!("warning: active probe writes and deletes a tiny object-store probe key");
            let store = build_object_store(config)?;
            const PROBE_KEY: &str = "operator-doctor/probe";
            store
                .put(PROBE_KEY, b"probe")
                .map_err(|_| AppError::dependency("object store probe put failed"))?;
            let present = store
                .get(PROBE_KEY)
                .map_err(|_| AppError::dependency("object store probe get failed"))?
                .is_some();
            store
                .delete(PROBE_KEY)
                .map_err(|_| AppError::dependency("object store probe delete failed"))?;
            println!(
                "object_store_probe: {}",
                if present { "ok" } else { "missing" }
            );
        }
    }
    Ok(())
}

fn is_loopback_listen(address: &str) -> bool {
    is_loopback_host(listen_host(address))
}

fn is_loopback_url(url: &str) -> bool {
    is_loopback_host(url_host(url))
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

fn listen_host(address: &str) -> &str {
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    address
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(address)
}

fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or(rest);
    listen_host(hostport)
}
