//! Operator seam: invalid configuration reports stable redacted config_error.

mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn config_validate_invalid_retention_exits_with_config_error() {
    let cfg = common::TestConfig::invalid_retention();

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
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
            "retention.original_receipt_days must be between 1 and 30",
        ))
        .stderr(predicate::str::contains("postgres://").not());
}

#[test]
fn config_show_never_prints_database_url() {
    let url =
        common::test_database_url().unwrap_or_else(|| "postgres://secret-host/db".to_string());
    let cfg = common::TestConfig::valid(&url);

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "config",
            "show",
        ])
        .env_remove("TEST_DATABASE_URL")
        .env_remove("ZL_EXPENSE_DATABASE_URL")
        .assert()
        .success()
        .stdout(predicate::str::contains("database.url_credential"))
        .stdout(predicate::str::contains("secret-host").not())
        .stdout(predicate::str::contains("postgres://").not());
}
