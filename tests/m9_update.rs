//! Milestone 9 signed update and schema-gated rollback tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::fs;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use ed25519_dalek::{Signer, SigningKey};
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zl_expense::update::{UpdateMetadata, rollback_permitted};

fn migrate_config_db(cfg: &common::TestConfig, url: &str) {
    let status = StdCommand::new(cargo_bin("zl-expense"))
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "db",
            "migrate",
        ])
        .env("TEST_DATABASE_URL", url)
        .status()
        .expect("migrate");
    assert!(status.success(), "migrate failed");
}

struct SignedBundle {
    dir: TempDir,
    artifact: std::path::PathBuf,
    metadata: std::path::PathBuf,
    signature: std::path::PathBuf,
    public_key: std::path::PathBuf,
}

fn signed_bundle(package_version: &str, schema_version: i64) -> SignedBundle {
    let dir = TempDir::new().expect("bundle dir");
    let artifact = dir.path().join("zl-expense");
    fs::write(&artifact, b"#!/bin/sh\necho updated\n").expect("artifact");

    let digest = hex::encode(Sha256::digest(fs::read(&artifact).expect("read artifact")));
    let metadata_body = serde_json::to_vec_pretty(&UpdateMetadata {
        package_version: package_version.to_string(),
        schema_version,
        min_runtime_schema: schema_version,
        max_runtime_schema: schema_version,
        sha256: digest,
        arch: std::env::consts::ARCH.to_string(),
    })
    .expect("metadata json");
    let metadata = dir.path().join("metadata.json");
    fs::write(&metadata, &metadata_body).expect("write metadata");

    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let signature = signing_key.sign(&metadata_body);
    let signature_path = dir.path().join("metadata.sig");
    fs::write(&signature_path, hex::encode(signature.to_bytes())).expect("write signature");

    let public_key = dir.path().join("release.pub");
    fs::write(
        &public_key,
        hex::encode(signing_key.verifying_key().to_bytes()),
    )
    .expect("write public key");

    SignedBundle {
        dir,
        artifact,
        metadata,
        signature: signature_path,
        public_key,
    }
}

#[test]
fn rollback_helper_blocks_incompatible_schema() {
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

#[tokio::test]
async fn update_preflight_rejects_bad_signature() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database("update_preflight_rejects_bad_signature") else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let bundle = signed_bundle("0.1.1", 10);
    fs::write(&bundle.signature, "00".repeat(64)).expect("tamper signature");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "update",
            "preflight",
            "--artifact",
            bundle.artifact.to_str().expect("artifact"),
            "--metadata",
            bundle.metadata.to_str().expect("metadata"),
            "--signature",
            bundle.signature.to_str().expect("signature"),
            "--public-key",
            bundle.public_key.to_str().expect("public key"),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .failure()
        .stderr(predicate::str::contains("preflight_failed"));
    let _ = bundle.dir;
}

#[tokio::test]
async fn update_apply_replaces_binary_and_compatible_rollback_restores() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database(
        "update_apply_replaces_binary_and_compatible_rollback_restores",
    ) else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let workspace = TempDir::new().expect("workspace");
    let install_path = workspace.path().join("zl-expense");
    let state_dir = workspace.path().join("state");
    fs::write(&install_path, b"original-binary\n").expect("seed binary");

    let schema = zl_expense::update::current_binary_schema_version();
    let bundle = signed_bundle("0.1.1", schema);

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "update",
            "apply",
            "--artifact",
            bundle.artifact.to_str().expect("artifact"),
            "--metadata",
            bundle.metadata.to_str().expect("metadata"),
            "--signature",
            bundle.signature.to_str().expect("signature"),
            "--public-key",
            bundle.public_key.to_str().expect("public key"),
            "--install-path",
            install_path.to_str().expect("install"),
            "--state-dir",
            state_dir.to_str().expect("state"),
            "--yes",
            "--skip-backup",
            "--skip-migrate",
            "--skip-health",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success();

    let installed = fs::read(&install_path).expect("installed");
    assert_eq!(installed, fs::read(&bundle.artifact).expect("artifact"));

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "update",
            "rollback",
            "--yes",
            "--install-path",
            install_path.to_str().expect("install"),
            "--state-dir",
            state_dir.to_str().expect("state"),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success();

    assert_eq!(
        fs::read(&install_path).expect("rolled back"),
        b"original-binary\n"
    );
    let _ = bundle.dir;
}

#[tokio::test]
async fn update_rollback_blocked_when_schema_incompatible() {
    let _guard = common::integration_lock();
    let Some(url) =
        common::skip_without_database("update_rollback_blocked_when_schema_incompatible")
    else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let workspace = TempDir::new().expect("workspace");
    let install_path = workspace.path().join("zl-expense");
    let state_dir = workspace.path().join("state");
    fs::create_dir_all(&state_dir).expect("state dir");
    fs::write(&install_path, b"new-binary\n").expect("current");
    fs::write(state_dir.join("previous-binary"), b"old-binary\n").expect("previous binary");
    fs::write(
        state_dir.join("previous.json"),
        serde_json::to_vec(&UpdateMetadata {
            package_version: "0.0.9".into(),
            schema_version: 1,
            min_runtime_schema: 1,
            max_runtime_schema: 1,
            sha256: "00".into(),
            arch: "aarch64".into(),
        })
        .expect("json"),
    )
    .expect("previous metadata");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "update",
            "rollback",
            "--yes",
            "--install-path",
            install_path.to_str().expect("install"),
            "--state-dir",
            state_dir.to_str().expect("state"),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .failure()
        .stderr(predicate::str::contains("conflict"));

    assert_eq!(fs::read(&install_path).expect("unchanged"), b"new-binary\n");
}
