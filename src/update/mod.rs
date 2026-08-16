//! Signed update metadata, preflight, apply, and schema-gated rollback.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::config::ResolvedConfig;
use crate::db::MIGRATOR;
use crate::error::AppError;
use crate::operator::run_backup;

const PREVIOUS_BINARY: &str = "previous-binary";
const PREVIOUS_METADATA: &str = "previous.json";
const CURRENT_METADATA: &str = "current.json";
const BACKUP_NAME: &str = "pre-update.dump";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateMetadata {
    pub package_version: String,
    pub schema_version: i64,
    pub min_runtime_schema: i64,
    pub max_runtime_schema: i64,
    pub sha256: String,
    pub arch: String,
}

#[derive(Debug, Clone)]
pub struct UpdatePaths {
    pub artifact: PathBuf,
    pub metadata: PathBuf,
    pub signature: PathBuf,
    pub public_keys: Vec<PathBuf>,
    pub public_keys_directory: PathBuf,
    pub install_path: PathBuf,
    pub state_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    pub yes: bool,
    pub skip_backup: bool,
    pub skip_migrate: bool,
    pub skip_health: bool,
    pub health_url: Option<String>,
}

pub fn current_binary_schema_version() -> i64 {
    i64::try_from(MIGRATOR.migrations.len()).unwrap_or(0)
}

pub fn rollback_permitted(database_schema: i64, previous: &UpdateMetadata) -> bool {
    database_schema >= previous.min_runtime_schema && database_schema <= previous.max_runtime_schema
}

pub async fn run_preflight(pool: &PgPool, paths: &UpdatePaths) -> Result<UpdateMetadata, AppError> {
    let metadata = verify_artifact(paths)?;
    let db_schema = current_database_schema(pool).await?;
    check_schema_preflight(db_schema, &metadata)?;
    println!(
        "preflight ok package_version={} artifact_sha256={} db_schema={} target_schema={}",
        metadata.package_version, metadata.sha256, db_schema, metadata.schema_version
    );
    Ok(metadata)
}

pub async fn run_apply(
    pool: &PgPool,
    config: &ResolvedConfig,
    paths: &UpdatePaths,
    options: &ApplyOptions,
) -> Result<(), AppError> {
    if !options.yes {
        return Err(AppError::usage("update apply requires --yes"));
    }

    let metadata = verify_artifact(paths)?;
    let db_schema = current_database_schema(pool).await?;
    check_schema_preflight(db_schema, &metadata)?;

    fs::create_dir_all(&paths.state_dir)
        .map_err(|_| AppError::dependency("failed to create update state directory"))?;

    if !options.skip_backup {
        let backup_path = paths.state_dir.join(BACKUP_NAME);
        run_backup(config, &backup_path)?;
    }

    if paths.install_path.exists() {
        let previous_binary = paths.state_dir.join(PREVIOUS_BINARY);
        fs::copy(&paths.install_path, &previous_binary)
            .map_err(|_| AppError::dependency("failed to snapshot current binary"))?;
        if let Ok(current_meta) = fs::read(paths.state_dir.join(CURRENT_METADATA)) {
            fs::write(paths.state_dir.join(PREVIOUS_METADATA), current_meta)
                .map_err(|_| AppError::dependency("failed to snapshot current metadata"))?;
        } else {
            let snapshot = UpdateMetadata {
                package_version: env!("CARGO_PKG_VERSION").to_string(),
                schema_version: current_binary_schema_version(),
                min_runtime_schema: current_binary_schema_version(),
                max_runtime_schema: current_binary_schema_version(),
                sha256: sha256_file(&paths.install_path)?,
                arch: std::env::consts::ARCH.to_string(),
            };
            write_json(&paths.state_dir.join(PREVIOUS_METADATA), &snapshot)?;
        }
    }

    atomic_replace(&paths.artifact, &paths.install_path)?;
    write_json(&paths.state_dir.join(CURRENT_METADATA), &metadata)?;

    if !options.skip_migrate {
        crate::db::migrate(pool).await?;
    }

    if options.skip_health {
        println!(
            "update applied package_version={} install_path={}",
            metadata.package_version,
            paths.install_path.display()
        );
        return Ok(());
    }

    let health_url = options.health_url.clone().unwrap_or_else(|| {
        format!(
            "http://{}/health/ready",
            config.listen_address.replace("0.0.0.0", "127.0.0.1")
        )
    });
    if let Err(error) = verify_health(&health_url).await {
        let db_schema_after = current_database_schema(pool).await.unwrap_or(db_schema);
        if let Err(rollback_error) = restore_previous(paths, db_schema_after) {
            return Err(AppError::health(format!(
                "health check failed after update ({}); rollback blocked: {}",
                error.message, rollback_error.message
            )));
        }
        return Err(AppError::health(
            "health check failed after update; previous binary restored",
        ));
    }

    println!(
        "update applied package_version={} install_path={}",
        metadata.package_version,
        paths.install_path.display()
    );
    Ok(())
}

pub fn run_rollback(paths: &UpdatePaths, yes: bool, database_schema: i64) -> Result<(), AppError> {
    if !yes {
        return Err(AppError::usage("update rollback requires --yes"));
    }
    restore_previous(paths, database_schema)?;
    println!(
        "rollback restored previous binary to {}",
        paths.install_path.display()
    );
    Ok(())
}

pub fn verify_artifact(paths: &UpdatePaths) -> Result<UpdateMetadata, AppError> {
    let metadata_bytes = fs::read(&paths.metadata)
        .map_err(|_| AppError::preflight("update metadata file is unreadable"))?;
    let signature_bytes = fs::read(&paths.signature)
        .map_err(|_| AppError::preflight("update signature file is unreadable"))?;
    let artifact_bytes = fs::read(&paths.artifact)
        .map_err(|_| AppError::preflight("update artifact is unreadable"))?;

    let keys = load_public_keys(paths)?;
    if keys.is_empty() {
        return Err(AppError::preflight("no update public keys configured"));
    }

    let signature = parse_signature(&signature_bytes)?;
    if !keys
        .iter()
        .any(|key| key.verify(&metadata_bytes, &signature).is_ok())
    {
        return Err(AppError::preflight("update signature verification failed"));
    }

    let metadata: UpdateMetadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|_| AppError::preflight("update metadata is not valid JSON"))?;
    let digest = hex::encode(Sha256::digest(&artifact_bytes));
    if !digest.eq_ignore_ascii_case(metadata.sha256.trim()) {
        return Err(AppError::preflight(
            "artifact checksum does not match metadata",
        ));
    }
    if metadata.min_runtime_schema > metadata.max_runtime_schema
        || metadata.schema_version < metadata.min_runtime_schema
    {
        return Err(AppError::preflight(
            "update metadata schema compatibility range is invalid",
        ));
    }
    Ok(metadata)
}

fn check_schema_preflight(db_schema: i64, metadata: &UpdateMetadata) -> Result<(), AppError> {
    if db_schema < metadata.min_runtime_schema {
        return Err(AppError::preflight(format!(
            "database schema {db_schema} is older than binary min {}",
            metadata.min_runtime_schema
        )));
    }
    if db_schema > metadata.schema_version {
        return Err(AppError::preflight(format!(
            "database schema {db_schema} is newer than this package ({})",
            metadata.schema_version
        )));
    }
    Ok(())
}

fn restore_previous(paths: &UpdatePaths, database_schema: i64) -> Result<(), AppError> {
    let previous_binary = paths.state_dir.join(PREVIOUS_BINARY);
    if !previous_binary.exists() {
        return Err(AppError::conflict(
            "no previous binary is available; restore from backup",
        ));
    }
    let previous_meta_path = paths.state_dir.join(PREVIOUS_METADATA);
    let previous: UpdateMetadata = if previous_meta_path.exists() {
        serde_json::from_slice(
            &fs::read(&previous_meta_path)
                .map_err(|_| AppError::dependency("failed to read previous update metadata"))?,
        )
        .map_err(|_| AppError::dependency("previous update metadata is invalid"))?
    } else {
        return Err(AppError::conflict(
            "previous metadata missing; restore from backup",
        ));
    };

    if !rollback_permitted(database_schema, &previous) {
        return Err(AppError::conflict(format!(
            "rollback blocked: database schema {database_schema} is outside previous binary range {}-{}; restore from {}",
            previous.min_runtime_schema,
            previous.max_runtime_schema,
            paths.state_dir.join(BACKUP_NAME).display()
        )));
    }

    atomic_replace(&previous_binary, &paths.install_path)?;
    write_json(&paths.state_dir.join(CURRENT_METADATA), &previous)?;
    Ok(())
}

async fn verify_health(url: &str) -> Result<(), AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| AppError::dependency("health client unavailable"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::health("post-update health request failed"))?;
    if !response.status().is_success() {
        return Err(AppError::health(format!(
            "post-update health returned {}",
            response.status().as_u16()
        )));
    }
    Ok(())
}

async fn current_database_schema(pool: &PgPool) -> Result<i64, AppError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::BIGINT FROM _sqlx_migrations WHERE success = true",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::dependency("failed to read applied migration count"))
}

fn load_public_keys(paths: &UpdatePaths) -> Result<Vec<VerifyingKey>, AppError> {
    let mut files = paths.public_keys.clone();
    if paths.public_keys_directory.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&paths.public_keys_directory)
            .map_err(|_| AppError::preflight("update public keys directory is unreadable"))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        entries.sort();
        files.extend(entries);
    }

    let mut keys = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)
            .map_err(|_| AppError::preflight("update public key file is unreadable"))?;
        keys.push(parse_public_key(text.trim())?);
    }
    Ok(keys)
}

fn parse_public_key(text: &str) -> Result<VerifyingKey, AppError> {
    let bytes = decode_hex(text)?;
    let raw: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AppError::preflight("update public key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&raw).map_err(|_| AppError::preflight("update public key is invalid"))
}

fn parse_signature(bytes: &[u8]) -> Result<Signature, AppError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| AppError::preflight("update signature must be hex text"))?
        .trim();
    let decoded = decode_hex(text)?;
    let raw: [u8; 64] = decoded
        .try_into()
        .map_err(|_| AppError::preflight("update signature must be 64 bytes"))?;
    Ok(Signature::from_bytes(&raw))
}

fn decode_hex(text: &str) -> Result<Vec<u8>, AppError> {
    hex::decode(text.trim()).map_err(|_| AppError::preflight("value is not valid hex"))
}

fn sha256_file(path: &Path) -> Result<String, AppError> {
    let bytes =
        fs::read(path).map_err(|_| AppError::dependency("failed to hash installed binary"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn atomic_replace(src: &Path, dest: &Path) -> Result<(), AppError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| AppError::dependency("failed to create install directory"))?;
    }
    let tmp = dest.with_extension("tmp-update");
    fs::copy(src, &tmp).map_err(|_| AppError::dependency("failed to stage update binary"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
            .map_err(|_| AppError::permission("failed to set update binary mode"))?;
    }
    fs::rename(&tmp, dest).map_err(|_| AppError::dependency("failed to replace installed binary"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|_| AppError::internal("failed to serialize update metadata"))?;
    let mut file = fs::File::create(path)
        .map_err(|_| AppError::dependency("failed to write update metadata"))?;
    file.write_all(&body)
        .map_err(|_| AppError::dependency("failed to write update metadata"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_blocked_when_schema_newer_than_previous_max() {
        let previous = UpdateMetadata {
            package_version: "0.1.0".into(),
            schema_version: 10,
            min_runtime_schema: 10,
            max_runtime_schema: 10,
            sha256: "abc".into(),
            arch: "aarch64".into(),
        };
        assert!(rollback_permitted(10, &previous));
        assert!(!rollback_permitted(11, &previous));
    }
}
