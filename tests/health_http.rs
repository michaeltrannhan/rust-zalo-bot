//! Runtime seam: live and ready health endpoints.

mod common;

use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin;
use reqwest::StatusCode;
use sqlx::postgres::PgPoolOptions;
use tokio::time::sleep;
use uuid::Uuid;

fn binary_path() -> std::path::PathBuf {
    cargo_bin("zl-expense")
}

fn spawn_server(config_path: &str, db_url: &str) -> std::process::Child {
    StdCommand::new(binary_path())
        .args(["--config", config_path, "run", "--roles", "ingress"])
        .env("TEST_DATABASE_URL", db_url)
        .spawn()
        .expect("spawn")
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn health_live_always_reports_live_while_process_running() {
    let _guard = common::integration_lock();
    let db_url = match common::skip_without_database(
        "health_live_always_reports_live_while_process_running",
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

    let mut child = spawn_server(cfg.path().to_str().expect("path"), &db_url);

    sleep(Duration::from_secs(2)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://127.0.0.1:{port}/health/live"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("live request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("body");
    assert!(body.contains("live"));

    child.kill().expect("kill child");
    let _ = child.wait().expect("wait child");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn health_ready_true_when_db_migrated() {
    let _guard = common::integration_lock();
    let db_url = match common::skip_without_database("health_ready_true_when_db_migrated") {
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

    let mut child = spawn_server(cfg.path().to_str().expect("path"), &db_url);

    sleep(Duration::from_secs(2)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://127.0.0.1:{port}/health/ready"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("ready request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("body");
    assert!(body.contains("ready"));

    child.kill().expect("kill child");
    let _ = child.wait().expect("wait child");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn migrations_current_false_before_apply() {
    let _guard = common::integration_lock();
    let db_url = match common::skip_without_database("migrations_current_false_before_apply") {
        Some(url) => url,
        None => return,
    };

    let cfg = common::TestConfig::valid(&db_url);
    let resolved = zl_expense::config::load_config(Some(cfg.path())).expect("config");
    let admin_pool = zl_expense::db::create_pool(&resolved)
        .await
        .expect("admin pool");
    let schema = format!("m1_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .expect("create isolated schema");

    let search_path = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |connection, _metadata| {
            let statement = format!("SET search_path TO {search_path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&db_url)
        .await
        .expect("isolated pool");

    let pending = zl_expense::db::check_migrations_current(&pool)
        .await
        .expect("check");
    assert!(!pending);

    zl_expense::db::migrate(&pool)
        .await
        .expect("migrate isolated schema");
    assert!(
        zl_expense::db::check_migrations_current(&pool)
            .await
            .expect("check after migrate")
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await
        .expect("drop isolated schema");
}
