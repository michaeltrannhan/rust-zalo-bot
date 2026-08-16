//! Maintenance role integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::receipt::{
    FakeExtractor, InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle, ReceiptState,
};
use zl_expense::runtime::maintenance_tick;

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m6_maintenance_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .expect("create isolated schema");
    admin_pool.close().await;

    let search_path = schema;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
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

#[tokio::test]
async fn maintenance_tick_expires_stale_reviews() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let account_id = Uuid::new_v4();
    let submission_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state, consent_version, consented_at)
        VALUES ($1, 'active', 'consent-v1', NOW())
        "#,
    )
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed account");
    sqlx::query(
        r#"
        INSERT INTO receipt_submissions (
            id, account_id, inbound_event_id, lifecycle_state, review_expires_at
        )
        VALUES ($1, $2, NULL, 'review_required', NOW() - INTERVAL '1 hour')
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed submission");

    let receipt = ReceiptLifecycle::with_extractor(
        pool.clone(),
        InMemoryObjectStore::new(),
        std::sync::Arc::new(FakeExtractor),
        ReceiptConfig::default(),
    );
    maintenance_tick(&receipt).await;

    let state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("load state");
    assert_eq!(state, ReceiptState::Expired.as_str());
}
