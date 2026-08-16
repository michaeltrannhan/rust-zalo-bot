//! Milestone 7 operator CLI and metrics tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::fs;
use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use chrono::Utc;
use predicates::prelude::*;
use tempfile::TempDir;
use tokio::time::sleep;
use uuid::Uuid;
use zl_expense::config::load_config;
use zl_expense::db::create_pool;
use zl_expense::error::ErrorClass;
use zl_expense::work::{EnqueueRequest, JobState, WorkStore};

fn migrated_config(pool_url: &str, port: u16, metrics_enabled: bool) -> common::TestConfig {
    let cfg = common::TestConfig::valid_with_port(pool_url, port);
    if metrics_enabled {
        let contents = fs::read_to_string(cfg.path()).expect("read config");
        let contents = format!("{contents}\n[metrics]\nenabled = true\n");
        fs::write(cfg.path(), contents).expect("write config");
    }
    migrate_config_db(&cfg, pool_url);
    cfg
}

#[test]
fn example_config_metrics_disabled_by_default() {
    let example = fs::read_to_string("config/config.example.toml").expect("example config");
    assert!(example.contains("[metrics]"));
    assert!(example.contains("enabled = false"));
}

#[tokio::test]
async fn status_json_contains_job_counts_without_secrets() {
    let _guard = common::integration_lock();
    let Some(url) =
        common::skip_without_database("status_json_contains_job_counts_without_secrets")
    else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "status",
            "--json",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"jobs\""))
        .stdout(predicate::str::contains("postgres://").not());
}

#[tokio::test]
async fn jobs_list_never_includes_payload_keys() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database("jobs_list_never_includes_payload_keys") else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let resolved = load_config(Some(cfg.path())).expect("config");
    let pool = create_pool(&resolved).await.expect("pool");
    let store = WorkStore::new(pool);
    store
        .enqueue(EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: "test.echo".to_string(),
            payload: serde_json::json!({
                "schema_version": 1,
                "secret_payload": "do-not-print"
            }),
            dedupe_key: format!("dedupe-{}", Uuid::new_v4()),
            serialization_key: None,
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 3,
        })
        .await
        .expect("enqueue");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "jobs",
            "list",
            "--json",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success()
        .stdout(predicate::str::contains("secret_payload").not())
        .stdout(predicate::str::contains("\"payload\"").not())
        .stdout(predicate::str::contains("dedupe_key").not());
}

#[tokio::test]
async fn jobs_retry_recovers_dead_job() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database("jobs_retry_recovers_dead_job") else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let resolved = load_config(Some(cfg.path())).expect("config");
    let pool = create_pool(&resolved).await.expect("pool");
    let store = WorkStore::new(pool.clone());
    let job_id = Uuid::new_v4();
    store
        .enqueue(EnqueueRequest {
            id: job_id,
            job_type: "test.echo".to_string(),
            payload: serde_json::json!({"schema_version": 1}),
            dedupe_key: format!("dead-{}", Uuid::new_v4()),
            serialization_key: None,
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 1,
        })
        .await
        .expect("enqueue");

    sqlx::query("UPDATE jobs SET state = 'dead', completed_at = NOW() WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .expect("mark dead");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "jobs",
            "retry",
            &job_id.to_string(),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success();

    let summary = store.get_job_summary(job_id).await.expect("summary");
    assert_eq!(summary.state, JobState::Queued);
}

#[tokio::test]
async fn ingress_poll_then_webhook_advances_mode_generation() {
    let _guard = common::integration_lock();
    let Some(url) =
        common::skip_without_database("ingress_poll_then_webhook_advances_mode_generation")
    else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let resolved = load_config(Some(cfg.path())).expect("config");
    let pool = create_pool(&resolved).await.expect("pool");

    let generation_before: i32 =
        sqlx::query_scalar("SELECT mode_generation FROM ingress_control WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("generation");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "ingress",
            "poll",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success()
        .stdout(predicate::str::contains("mode: polling"));

    let generation_after_poll: i32 =
        sqlx::query_scalar("SELECT mode_generation FROM ingress_control WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("generation");
    assert_eq!(generation_after_poll, generation_before + 1);

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "ingress",
            "webhook",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success()
        .stdout(predicate::str::contains("mode: webhook"));

    let generation_after_webhook: i32 =
        sqlx::query_scalar("SELECT mode_generation FROM ingress_control WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("generation");
    assert_eq!(generation_after_webhook, generation_after_poll + 1);
}

#[test]
fn doctor_fails_closed_on_missing_db_without_secrets() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
[server]
listen_address = "127.0.0.1:8080"

[database]
url_credential = "missing-database"

[credentials]
directory = "/nonexistent/credentials"

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
"#,
    )
    .expect("write config");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args(["--config", config_path.to_str().expect("path"), "doctor"])
        .env_remove("TEST_DATABASE_URL")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("config_error")
                .or(predicate::str::contains(ErrorClass::Dependency.as_str())),
        )
        .stderr(predicate::str::contains("postgres://").not());
}

#[tokio::test]
async fn diagnose_bundle_preview_and_files_have_no_credential_values() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database(
        "diagnose_bundle_preview_and_files_have_no_credential_values",
    ) else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let output = TempDir::new().expect("output dir");

    let assert = Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "diagnose",
            "--output",
            output.path().to_str().expect("output"),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("status.json"));
    assert!(stdout.contains("jobs-dead.json"));
    assert!(stdout.contains("config-show.json"));
    assert!(stdout.contains("doctor.json"));

    for file in [
        "status.json",
        "jobs-dead.json",
        "config-show.json",
        "doctor.json",
    ] {
        let contents = fs::read_to_string(output.path().join(file)).expect("bundle file");
        assert!(!contents.contains(&url));
        assert!(!contents.contains("postgres://"));
        assert!(!contents.contains("test-zalo-bot-token"));
    }
}

#[tokio::test]
async fn metrics_absent_when_disabled_present_when_enabled() {
    let _guard = common::integration_lock();
    let Some(url) =
        common::skip_without_database("metrics_absent_when_disabled_present_when_enabled")
    else {
        return;
    };

    let port = common::available_port();
    let disabled_cfg = migrated_config(&url, port, false);

    let mut disabled_child = spawn_server(disabled_cfg.path().to_str().expect("path"), &url);
    sleep(Duration::from_secs(2)).await;
    let client = reqwest::Client::new();
    let disabled_response = client
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .expect("metrics disabled");
    assert_eq!(disabled_response.status(), reqwest::StatusCode::NOT_FOUND);
    disabled_child.kill().expect("kill");
    let _ = disabled_child.wait();

    let enabled_port = common::available_port();
    let enabled_cfg = migrated_config(&url, enabled_port, true);
    let mut enabled_child = spawn_server(enabled_cfg.path().to_str().expect("path"), &url);
    sleep(Duration::from_secs(2)).await;
    let enabled_response = client
        .get(format!("http://127.0.0.1:{enabled_port}/metrics"))
        .send()
        .await
        .expect("metrics enabled");
    assert_eq!(enabled_response.status(), reqwest::StatusCode::OK);
    let body = enabled_response.text().await.expect("metrics body");
    assert!(body.contains("jobs_queued"));
    let sample_uuid = Uuid::new_v4().to_string();
    assert!(!body.contains(&sample_uuid));
    assert!(!body.contains("account_id"));
    assert!(!body.contains("merchant"));
    enabled_child.kill().expect("kill");
    let _ = enabled_child.wait();
}

#[tokio::test]
async fn jobs_show_omits_payload_and_dedupe_fields() {
    let _guard = common::integration_lock();
    let Some(url) = common::skip_without_database("jobs_show_omits_payload_and_dedupe_fields")
    else {
        return;
    };

    let cfg = common::TestConfig::valid(&url);
    migrate_config_db(&cfg, &url);
    let resolved = load_config(Some(cfg.path())).expect("config");
    let pool = create_pool(&resolved).await.expect("pool");
    let store = WorkStore::new(pool);
    let job_id = Uuid::new_v4();
    store
        .enqueue(EnqueueRequest {
            id: job_id,
            job_type: "test.echo".to_string(),
            payload: serde_json::json!({
                "schema_version": 1,
                "secret_payload": "do-not-print"
            }),
            dedupe_key: format!("dedupe-{}", Uuid::new_v4()),
            serialization_key: Some("account:operator-test".to_string()),
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 3,
        })
        .await
        .expect("enqueue");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "jobs",
            "show",
            &job_id.to_string(),
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .success()
        .stdout(predicate::str::contains("secret_payload").not())
        .stdout(predicate::str::contains("\"payload\"").not())
        .stdout(predicate::str::contains("dedupe_key").not())
        .stdout(predicate::str::contains("serialization_key").not());
}

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

fn spawn_server(config_path: &str, db_url: &str) -> std::process::Child {
    StdCommand::new(cargo_bin("zl-expense"))
        .args(["--config", config_path, "run", "--roles", "ingress"])
        .env("TEST_DATABASE_URL", db_url)
        .spawn()
        .expect("spawn")
}
