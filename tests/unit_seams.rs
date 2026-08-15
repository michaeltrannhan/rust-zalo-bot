//! Unit tests for configuration and runtime role parsing.

use std::sync::{Mutex, MutexGuard};

use zl_expense::config::{load_config, validate_config};
use zl_expense::runtime::{Role, parse_roles};

static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

fn env_test_lock() -> MutexGuard<'static, ()> {
    ENV_TEST_LOCK.lock().expect("env test lock")
}

#[test]
fn validate_rejects_retention_above_thirty() {
    let _guard = env_test_lock();
    unsafe {
        std::env::remove_var("ZL_EXPENSE_RETENTION_ORIGINAL_RECEIPT_DAYS");
        std::env::remove_var("TEST_DATABASE_URL");
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let cred_dir = dir.path().join("credentials");
    std::fs::create_dir_all(&cred_dir).expect("cred dir");
    std::fs::write(cred_dir.join("database"), "postgres://localhost/db").expect("cred");

    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[retention]
original_receipt_days = 31

[credentials]
directory = "{}"
"#,
            cred_dir.display()
        ),
    )
    .expect("config");

    let err = validate_config(Some(&config_path)).unwrap_err();
    assert_eq!(err.class.as_str(), "config_error");
    assert!(err.message.contains("1 and 30"));
}

#[test]
fn parse_roles_defaults_to_all() {
    let roles = parse_roles(&[]).expect("parse");
    assert_eq!(roles.len(), 4);
    assert!(roles.contains(&Role::Ingress));
}

#[test]
fn parse_roles_rejects_unknown() {
    let err = parse_roles(&["bogus".to_string()]).expect_err("error");
    assert_eq!(err.exit_code().as_i32(), 2);
}

#[test]
fn load_config_applies_env_override_with_attribution() {
    let _guard = env_test_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let cred_dir = dir.path().join("credentials");
    std::fs::create_dir_all(&cred_dir).expect("cred dir");
    std::fs::write(cred_dir.join("database"), "postgres://localhost/db").expect("cred");

    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[credentials]
directory = "{}"
"#,
            cred_dir.display()
        ),
    )
    .expect("config");

    unsafe {
        std::env::set_var("ZL_EXPENSE_RETENTION_ORIGINAL_RECEIPT_DAYS", "14");
    }
    let resolved = load_config(Some(&config_path)).expect("load");
    assert_eq!(resolved.original_receipt_days, 14);
    let attr = resolved
        .attribution
        .get("retention.original_receipt_days")
        .expect("attr");
    assert_eq!(attr.source, zl_expense::config::ConfigSource::Env);
    unsafe {
        std::env::remove_var("ZL_EXPENSE_RETENTION_ORIGINAL_RECEIPT_DAYS");
    }
}

#[test]
fn load_config_rejects_credential_path_traversal() {
    let _guard = env_test_lock();
    unsafe {
        std::env::remove_var("TEST_DATABASE_URL");
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[database]
url_credential = "../../etc/passwd"

[credentials]
directory = "/etc/zl-expense/credentials"
"#,
    )
    .expect("config");

    let err = load_config(Some(&config_path)).expect_err("unsafe reference must fail");
    assert_eq!(err.class.as_str(), "config_error");
    assert!(!err.message.contains("passwd"));
}

#[test]
fn omitted_file_values_keep_default_source_attribution() {
    let _guard = env_test_lock();
    unsafe {
        std::env::set_var("ZL_EXPENSE_DATABASE_URL", "postgres://localhost/test");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[retention]\noriginal_receipt_days = 14\n").expect("config");

    let resolved = load_config(Some(&config_path)).expect("load");
    assert_eq!(
        resolved
            .attribution
            .get("retention.original_receipt_days")
            .expect("retention attribution")
            .source,
        zl_expense::config::ConfigSource::File
    );
    assert_eq!(
        resolved
            .attribution
            .get("database.max_connections")
            .expect("database attribution")
            .source,
        zl_expense::config::ConfigSource::Default
    );
    unsafe {
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }
}

#[test]
fn invalid_toml_error_does_not_echo_values() {
    let _guard = env_test_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "unexpected_secret = \"do-not-echo-this\"\n").expect("config");

    let err = load_config(Some(&config_path)).expect_err("unknown value must fail");
    assert_eq!(err.message, "invalid config TOML");
    assert!(!err.message.contains("do-not-echo-this"));
}

#[test]
fn access_allowlist_defaults_to_deny_all() {
    let _guard = env_test_lock();
    unsafe {
        std::env::set_var("ZL_EXPENSE_DATABASE_URL", "postgres://localhost/test");
        std::env::remove_var("ZL_EXPENSE_ALLOWED_PROVIDER_SENDER_IDS");
    }

    let resolved = load_config(None).expect("load defaults");
    assert!(resolved.allowed_provider_sender_ids.is_empty());
    assert!(!resolved.is_provider_sender_allowed("family-member-1"));

    unsafe {
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }
}

#[test]
fn config_show_reports_allowlist_count_without_identifiers() {
    let _guard = env_test_lock();
    unsafe {
        std::env::set_var("ZL_EXPENSE_DATABASE_URL", "postgres://localhost/test");
        std::env::remove_var("ZL_EXPENSE_ALLOWED_PROVIDER_SENDER_IDS");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[access]
allowed_provider_sender_ids = ["private-sender-123", "private-sender-456"]
"#,
    )
    .expect("config");

    let resolved = load_config(Some(&config_path)).expect("load");
    assert!(resolved.is_provider_sender_allowed("private-sender-123"));
    assert!(!resolved.is_provider_sender_allowed("different-sender"));
    let shown = resolved.show_json();
    assert!(!shown.contains("private-sender-123"));
    assert!(!shown.contains("private-sender-456"));
    assert!(shown.contains("\"count\": 2"));

    unsafe {
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }
}

#[test]
fn credential_reader_returns_value_without_exposing_it_on_failure() {
    let _guard = env_test_lock();
    unsafe {
        std::env::remove_var("TEST_DATABASE_URL");
        std::env::remove_var("ZL_EXPENSE_DATABASE_URL");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let cred_dir = dir.path().join("credentials");
    std::fs::create_dir_all(&cred_dir).expect("cred dir");
    std::fs::write(cred_dir.join("database"), "postgres://localhost/test").expect("database");
    std::fs::write(cred_dir.join("zalo-bot"), "secret-bot-token\n").expect("token");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!("[credentials]\ndirectory = \"{}\"\n", cred_dir.display()),
    )
    .expect("config");

    let resolved = load_config(Some(&config_path)).expect("load");
    assert_eq!(
        resolved.read_zalo_bot_token().expect("token"),
        "secret-bot-token"
    );
    let err = resolved.read_webhook_secret().expect_err("missing secret");
    assert_eq!(err.message, "required credential is unavailable");
    assert!(
        !err.to_json_line()
            .contains(cred_dir.to_str().expect("path"))
    );
}
