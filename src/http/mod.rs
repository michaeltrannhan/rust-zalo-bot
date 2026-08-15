//! HTTP server and health route handlers.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde_json::json;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

use crate::db::{check_connection, check_migrations_current};
use crate::health::ReadinessState;

/// Shared HTTP application state.
#[derive(Clone)]
pub struct AppState {
    pub readiness: Arc<ReadinessState>,
    pub pool: Option<PgPool>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(live_handler))
        .route("/health/ready", get(ready_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn live_handler(State(state): State<AppState>) -> Response {
    if state.readiness.is_live() {
        (StatusCode::OK, Json(json!({ "status": "live" }))).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_live" })),
        )
            .into_response()
    }
}

async fn ready_handler(State(state): State<AppState>) -> Response {
    if let Some(pool) = &state.pool {
        let db_ok = check_connection(pool).await.is_ok();
        state.readiness.set_db_reachable(db_ok);

        if db_ok {
            let migrations_ok = check_migrations_current(pool).await.unwrap_or(false);
            state.readiness.set_migrations_current(migrations_ok);
        } else {
            state.readiness.set_migrations_current(false);
        }
    }

    if state.readiness.is_ready() {
        (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not_ready" })),
        )
            .into_response()
    }
}
