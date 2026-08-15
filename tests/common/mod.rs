//! Shared helpers for integration tests.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use tempfile::TempDir;

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Serialize integration tests that share listen ports and database state.
pub fn integration_lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().expect("integration test lock")
}

/// Returns TEST_DATABASE_URL when set, otherwise None.
pub fn test_database_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL").ok()
}

pub fn skip_without_database(test_name: &str) -> Option<String> {
    test_database_url().or_else(|| {
        eprintln!("SKIP {}: TEST_DATABASE_URL not set", test_name);
        None
    })
}

pub fn available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local address").port()
}

pub struct TestConfig {
    pub dir: TempDir,
    pub config_path: PathBuf,
    pub credentials_dir: PathBuf,
}

impl TestConfig {
    pub fn valid_with_port(database_url: &str, port: u16) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let credentials_dir = dir.path().join("credentials");
        fs::create_dir_all(&credentials_dir).expect("credentials dir");

        fs::write(credentials_dir.join("database"), database_url).expect("write db cred");

        let config_path = dir.path().join("config.toml");
        let contents = format!(
            r#"
[server]
listen_address = "127.0.0.1:{port}"

[database]
url_credential = "database"
max_connections = 5

[concurrency]
receipt_extraction = 1
outbound_delivery = 4

[retention]
original_receipt_days = 7

[credentials]
directory = "{}"

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
"#,
            credentials_dir.display()
        );
        fs::write(&config_path, contents).expect("write config");

        Self {
            dir,
            config_path,
            credentials_dir,
        }
    }

    pub fn valid(database_url: &str) -> Self {
        Self::valid_with_port(database_url, 18080)
    }

    pub fn invalid_retention() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let credentials_dir = dir.path().join("credentials");
        fs::create_dir_all(&credentials_dir).expect("credentials dir");
        fs::write(credentials_dir.join("database"), "postgres://unused").expect("write db cred");

        let config_path = dir.path().join("config.toml");
        let contents = format!(
            r#"
[server]
listen_address = "127.0.0.1:18080"

[database]
url_credential = "database"
max_connections = 5

[concurrency]
receipt_extraction = 1
outbound_delivery = 4

[retention]
original_receipt_days = 31

[credentials]
directory = "{}"

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
"#,
            credentials_dir.display()
        );
        fs::write(&config_path, contents).expect("write config");

        Self {
            dir,
            config_path,
            credentials_dir,
        }
    }

    pub fn path(&self) -> &Path {
        self.config_path.as_path()
    }
}
