//! Shared helpers for integration tests.

#![allow(dead_code)]

use std::sync::MutexGuard;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgba};
use sqlx::PgPool;
use tempfile::TempDir;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::receipt::{
    AcceptSubmissionRequest, ConfirmRequest, InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle,
    ReceiptState, corpus_index_for, extract,
};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Serialize integration tests that share listen ports and database state.
pub fn integration_lock() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        fs::write(credentials_dir.join("zalo-bot"), "test-zalo-bot-token")
            .expect("write zalo token");
        fs::write(
            credentials_dir.join("webhook-secret"),
            "test-webhook-secret-value",
        )
        .expect("write webhook secret");

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

pub async fn receipt_fresh_pool() -> PgPool {
    let url = test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m4_receipt_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .expect("create isolated schema");
    admin_pool.close().await;

    let search_path = schema;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .after_connect(move |connection, _metadata| {
            let statement = format!("SET search_path TO {search_path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect test database");
    MIGRATOR.run(&pool).await.expect("run migrations");
    pool
}

pub async fn seed_active_account(pool: &PgPool) -> Uuid {
    seed_active_account_with_retention(pool, 7).await
}

pub async fn seed_active_account_with_retention(pool: &PgPool, retention_days: i32) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (
            id, lifecycle_state, consent_version, consented_at, retention_preference_days
        )
        VALUES ($1, 'active', 'v1', NOW(), $2)
        "#,
    )
    .bind(account_id)
    .bind(retention_days)
    .execute(pool)
    .await
    .expect("seed account");
    account_id
}

pub async fn seed_inbound_event(pool: &PgPool, account_id: Uuid) -> Uuid {
    let inbound_event_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO inbound_events (
            id, provider_event_id, provider_scope, kind, processing_state, account_id
        )
        VALUES ($1, $2, 'zalo', 'image', 'accepted', $3)
        "#,
    )
    .bind(inbound_event_id)
    .bind(inbound_event_id.to_string())
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed inbound event");
    inbound_event_id
}

pub fn receipt_lifecycle(pool: PgPool) -> ReceiptLifecycle {
    ReceiptLifecycle::new(
        pool,
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
        },
    )
}

pub fn png_bytes(seed: &[u8]) -> Vec<u8> {
    let width = 8_u32;
    let height = 8_u32;
    let mut image = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(width, height);
    for (index, pixel) in image.pixels_mut().enumerate() {
        let value = seed[index % seed.len()];
        *pixel = Rgba([value, value.wrapping_add(1), value.wrapping_add(2), 255]);
    }
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
        .expect("encode png");
    bytes
}

pub fn corpus_png(index: usize) -> Vec<u8> {
    let mut nonce = 0_u32;
    loop {
        let seed = format!("mock-png-{index}-{nonce}");
        let bytes = png_bytes(seed.as_bytes());
        if corpus_index_for(&bytes) == index {
            return bytes;
        }
        nonce += 1;
    }
}

pub async fn accept_and_ingest(
    lifecycle: &ReceiptLifecycle,
    account_id: Uuid,
    submission_id: Uuid,
    bytes: &[u8],
) {
    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: None,
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");
    lifecycle
        .ingest(
            account_id,
            submission_id,
            bytes,
            "image/png",
            Uuid::new_v4(),
        )
        .await
        .expect("ingest");
}

pub async fn drive_to_review(
    lifecycle: &ReceiptLifecycle,
    account_id: Uuid,
    submission_id: Uuid,
    bytes: &[u8],
) {
    accept_and_ingest(lifecycle, account_id, submission_id, bytes).await;
    lifecycle
        .extract(account_id, submission_id)
        .await
        .expect("extract");
}

pub async fn confirm_submission(
    lifecycle: &ReceiptLifecycle,
    account_id: Uuid,
    submission_id: Uuid,
) -> Uuid {
    let draft = lifecycle
        .get_draft(account_id, submission_id)
        .await
        .expect("draft");
    let expense_id = Uuid::new_v4();
    lifecycle
        .confirm(ConfirmRequest {
            account_id,
            submission_id,
            expected_draft_version: draft.version,
            expense_id,
        })
        .await
        .expect("confirm");
    expense_id
}

pub async fn assert_receipt_state(
    lifecycle: &ReceiptLifecycle,
    account_id: Uuid,
    submission_id: Uuid,
    expected: ReceiptState,
) {
    let view = lifecycle
        .get_state(account_id, submission_id)
        .await
        .expect("state");
    assert_eq!(view.state, expected);
}

pub fn expected_extraction(bytes: &[u8]) -> zl_expense::receipt::ExtractionResult {
    extract(bytes).expect("extract")
}
