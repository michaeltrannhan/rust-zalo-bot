//! Milestone 1 acceptance tests for runtime and operator public seams.

mod common;

use std::fs;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use reqwest::StatusCode;
use serde_json::Value;
use tempfile::TempDir;

fn binary_path() -> std::path::PathBuf {
    cargo_bin("zl-expense")
}

fn write_config_with_listen(
    database_url: &str,
    listen_address: &str,
    retention_days: u32,
) -> TestConfigPaths {
    let dir = TempDir::new().expect("tempdir");
    let credentials_dir = dir.path().join("credentials");
    fs::create_dir_all(&credentials_dir).expect("credentials dir");
    fs::write(credentials_dir.join("database"), database_url).expect("db cred");
    fs::write(credentials_dir.join("zalo-bot"), "token-secret-value").expect("zalo cred");
    fs::write(
        credentials_dir.join("webhook-secret"),
        "webhook-secret-value",
    )
    .expect("webhook cred");

    let config_path = dir.path().join("config.toml");
    let contents = format!(
        r#"
[server]
listen_address = "{listen_address}"

[database]
url_credential = "database"
max_connections = 5

[concurrency]
receipt_extraction = 1
outbound_delivery = 4

[retention]
original_receipt_days = {retention_days}

[credentials]
directory = "{}"

[storage]
backend = "memory"

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
"#,
        credentials_dir.display()
    );
    fs::write(&config_path, contents).expect("write config");

    TestConfigPaths {
        _dir: dir,
        config_path,
    }
}

struct TestConfigPaths {
    _dir: TempDir,
    config_path: std::path::PathBuf,
}

async fn poll_health_not_ok(url: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match client.get(url).timeout(Duration::from_secs(2)).send().await {
            Ok(response) if response.status() != StatusCode::OK => return true,
            Err(_) => return true,
            Ok(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

async fn poll_http_ok(url: &str, want_ok: bool, timeout: Duration) -> bool {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(response) = client.get(url).timeout(Duration::from_secs(2)).send().await {
            let is_ok = response.status() == StatusCode::OK;
            if is_ok == want_ok {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[test]
fn run_preserves_dependency_exit_code_on_database_failure() {
    let _guard = common::integration_lock();
    let cfg = write_config_with_listen(
        "postgres://postgres:postgres@127.0.0.1:1/unreachable",
        "127.0.0.1:18081",
        7,
    );

    Command::new(binary_path())
        .args([
            "--config",
            cfg.config_path.to_str().expect("path"),
            "run",
            "--roles",
            "ingress",
        ])
        .env_remove("TEST_DATABASE_URL")
        .env_remove("ZL_EXPENSE_DATABASE_URL")
        .assert()
        .failure()
        .code(4)
        .stdout(predicate::str::contains("dependency_error"))
        .stdout(predicate::str::contains("postgres://").not())
        .stdout(predicate::str::contains("token-secret-value").not());
}

#[test]
fn db_check_preserves_dependency_exit_code_on_database_failure() {
    let _guard = common::integration_lock();
    let cfg = write_config_with_listen(
        "postgres://postgres:postgres@127.0.0.1:1/unreachable",
        "127.0.0.1:18081",
        7,
    );

    Command::new(binary_path())
        .args([
            "--config",
            cfg.config_path.to_str().expect("path"),
            "db",
            "check",
        ])
        .env_remove("TEST_DATABASE_URL")
        .env_remove("ZL_EXPENSE_DATABASE_URL")
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("dependency_error"))
        .stderr(predicate::str::contains("postgres://").not());
}

#[test]
fn config_validate_invalid_listen_address_redacts_credential_secrets() {
    let _guard = common::integration_lock();
    let cfg = write_config_with_listen(
        "postgres://super-secret-host.example/dbname",
        "not-a-valid-socket-address",
        7,
    );

    Command::new(binary_path())
        .args([
            "--config",
            cfg.config_path.to_str().expect("path"),
            "config",
            "validate",
        ])
        .env_remove("TEST_DATABASE_URL")
        .env_remove("ZL_EXPENSE_DATABASE_URL")
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("config_error"))
        .stderr(predicate::str::contains(
            "server.listen_address must be a valid IP socket address",
        ))
        .stderr(predicate::str::contains("super-secret-host").not())
        .stderr(predicate::str::contains("postgres://").not())
        .stderr(predicate::str::contains("token-secret-value").not());
}

#[test]
fn config_show_attributes_listen_address_to_env_override() {
    let _guard = common::integration_lock();
    let url = common::test_database_url().unwrap_or_else(|| {
        "postgres://postgres:postgres@127.0.0.1:55439/zl_expense_test".to_string()
    });
    let cfg = common::TestConfig::valid(&url);
    let override_listen = "127.0.0.1:19123";

    let output = Command::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "config",
            "show",
        ])
        .env_remove("TEST_DATABASE_URL")
        .env_remove("ZL_EXPENSE_DATABASE_URL")
        .env("ZL_EXPENSE_LISTEN_ADDRESS", override_listen)
        .assert()
        .success()
        .stdout(predicate::str::contains("postgres://").not())
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("config show JSON");
    let listen = json
        .get("server.listen_address")
        .expect("server.listen_address entry");
    assert_eq!(
        listen.get("value").and_then(|v| v.as_str()),
        Some(override_listen)
    );
    assert_eq!(listen.get("source").and_then(|v| v.as_str()), Some("env"));

    let retention = json
        .get("retention.original_receipt_days")
        .expect("retention attribution");
    assert_eq!(
        retention.get("source").and_then(|v| v.as_str()),
        Some("file")
    );
}

#[test]
fn run_exits_runtime_error_on_critical_bind_failure() {
    let _guard = common::integration_lock();
    let url = common::test_database_url().unwrap_or_else(|| {
        "postgres://postgres:postgres@127.0.0.1:55439/zl_expense_test".to_string()
    });
    let port = common::available_port();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind port");
    let cfg = common::TestConfig::valid_with_port(&url, port);

    let status = StdCommand::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "db",
            "migrate",
        ])
        .env("TEST_DATABASE_URL", &url)
        .status()
        .expect("migrate");
    assert!(status.success(), "migrate failed");

    Command::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "run",
            "--roles",
            "ingress",
        ])
        .env("TEST_DATABASE_URL", &url)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("postgres://").not());

    drop(listener);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn health_live_and_ready_report_distinct_status_while_serving() {
    let _guard = common::integration_lock();
    let db_url = match common::skip_without_database(
        "health_live_and_ready_report_distinct_status_while_serving",
    ) {
        Some(url) => url,
        None => return,
    };

    let port = common::available_port();
    let cfg = common::TestConfig::valid_with_port(&db_url, port);
    let status = StdCommand::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "db",
            "migrate",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .status()
        .expect("migrate");
    assert!(status.success(), "migrate failed");

    let mut child = StdCommand::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "run",
            "--roles",
            "ingress",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .spawn()
        .expect("spawn");

    let ready_url = format!("http://127.0.0.1:{port}/health/ready");
    let live_url = format!("http://127.0.0.1:{port}/health/live");
    assert!(
        poll_http_ok(&ready_url, true, Duration::from_secs(10)).await,
        "ready endpoint did not become OK"
    );
    assert!(
        poll_http_ok(&live_url, true, Duration::from_secs(5)).await,
        "live endpoint did not become OK"
    );

    let client = reqwest::Client::new();
    let live_body = client
        .get(&live_url)
        .send()
        .await
        .expect("live")
        .text()
        .await
        .expect("live body");
    let ready_body = client
        .get(&ready_url)
        .send()
        .await
        .expect("ready")
        .text()
        .await
        .expect("ready body");
    assert!(live_body.contains("live"));
    assert!(ready_body.contains("ready"));

    child.kill().expect("kill");
    let _ = child.wait().expect("wait");
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sigterm_flips_readiness_before_exit_within_deadline() {
    let _guard = common::integration_lock();
    let db_url = match common::skip_without_database(
        "sigterm_flips_readiness_before_exit_within_deadline",
    ) {
        Some(url) => url,
        None => return,
    };

    let port = common::available_port();
    let cfg = common::TestConfig::valid_with_port(&db_url, port);
    let status = StdCommand::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "db",
            "migrate",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .status()
        .expect("migrate");
    assert!(status.success(), "migrate failed");

    let mut child = StdCommand::new(binary_path())
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "run",
            "--roles",
            "ingress",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .spawn()
        .expect("spawn");

    let ready_url = format!("http://127.0.0.1:{port}/health/ready");
    let live_url = format!("http://127.0.0.1:{port}/health/live");
    assert!(
        poll_http_ok(&ready_url, true, Duration::from_secs(10)).await,
        "server not ready before SIGTERM"
    );

    let pid = child.id();
    StdCommand::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("send SIGTERM");

    assert!(
        poll_health_not_ok(&ready_url, Duration::from_secs(5)).await,
        "ready endpoint did not flip false after SIGTERM"
    );
    assert!(
        poll_health_not_ok(&live_url, Duration::from_secs(5)).await,
        "live endpoint did not flip false after SIGTERM"
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    let exit_status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill after deadline");
            panic!("process did not exit within graceful shutdown deadline");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert!(exit_status.success(), "expected graceful exit code 0");
}
