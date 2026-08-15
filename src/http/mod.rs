//! HTTP server and health route handlers.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router, body,
    routing::{get, post},
};
use serde_json::json;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

use crate::db::{check_connection, check_migrations_current};
use crate::health::ReadinessState;
use crate::ingress::{
    IngressOutcome, IngressRequest, IngressSource, IngressStore, process_image,
    process_text_command,
};
use crate::provider::{InboundEventKind, SECRET_HEADER, ZaloHttpAdapter};

/// Authenticated Zalo webhook application service.
pub struct WebhookService {
    adapter: Arc<ZaloHttpAdapter>,
    store: IngressStore,
    allowed_provider_sender_ids: std::collections::BTreeSet<String>,
    max_body_bytes: usize,
}

impl WebhookService {
    pub fn new(
        adapter: Arc<ZaloHttpAdapter>,
        store: IngressStore,
        allowed_provider_sender_ids: std::collections::BTreeSet<String>,
        max_body_bytes: usize,
    ) -> Self {
        Self {
            adapter,
            store,
            allowed_provider_sender_ids,
            max_body_bytes,
        }
    }
}

/// Shared HTTP application state.
#[derive(Clone)]
pub struct AppState {
    pub readiness: Arc<ReadinessState>,
    pub pool: Option<PgPool>,
    pub webhook: Option<Arc<WebhookService>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(live_handler))
        .route("/health/ready", get(ready_handler))
        .route("/webhooks/zalo", post(zalo_webhook_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn zalo_webhook_handler(
    State(state): State<AppState>,
    request: Request<body::Body>,
) -> Response {
    let Some(service) = state.webhook else {
        return status_json(StatusCode::NOT_FOUND, "not_configured");
    };

    let secret = request
        .headers()
        .get(SECRET_HEADER)
        .and_then(|value| value.to_str().ok());
    if service.adapter.verify_webhook_secret(secret).is_err() {
        return status_json(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let is_json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return status_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid_content_type");
    }

    let bytes = match body::to_bytes(request.into_body(), service.max_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => return status_json(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
    };
    let text_event = match service.adapter.parse_text_webhook(&bytes) {
        Ok(event) => event,
        Err(_) => return status_json(StatusCode::BAD_REQUEST, "invalid_payload"),
    };

    if matches!(text_event.kind, InboundEventKind::TextReceived) {
        let sender_allowed = service
            .allowed_provider_sender_ids
            .contains(&text_event.sender_id);
        let ingress = IngressRequest {
            source: IngressSource::Webhook,
            provider_scope: text_event.provider_scope,
            provider_event_id: text_event.event_id,
            provider_sender_id: text_event.sender_id,
            provider_chat_id: text_event.chat_id,
            sender_allowed,
            user_text: text_event.text,
            observed_at: text_event.received_at,
        };
        return match process_text_command(&service.store, ingress).await {
            Ok(IngressOutcome::Accepted { .. }) => status_json(StatusCode::OK, "accepted"),
            Ok(IngressOutcome::Duplicate { .. }) => status_json(StatusCode::OK, "duplicate"),
            Ok(IngressOutcome::ModeRejected { .. }) => {
                status_json(StatusCode::CONFLICT, "mode_rejected")
            }
            Err(_) => status_json(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
    }

    let image_event = match service.adapter.parse_image_webhook(&bytes) {
        Ok(event) => event,
        Err(_) => return status_json(StatusCode::BAD_REQUEST, "invalid_payload"),
    };
    if !matches!(image_event.kind, InboundEventKind::ImageReceived) {
        return status_json(StatusCode::OK, "unsupported");
    }

    let sender_allowed = service
        .allowed_provider_sender_ids
        .contains(&image_event.sender_id);
    let ingress = IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: image_event.provider_scope,
        provider_event_id: image_event.event_id,
        provider_sender_id: image_event.sender_id,
        provider_chat_id: image_event.chat_id,
        sender_allowed,
        user_text: image_event.caption,
        observed_at: image_event.received_at,
    };
    match process_image(&service.store, ingress, image_event.image_url).await {
        Ok(IngressOutcome::Accepted { .. }) => status_json(StatusCode::OK, "accepted"),
        Ok(IngressOutcome::Duplicate { .. }) => status_json(StatusCode::OK, "duplicate"),
        Ok(IngressOutcome::ModeRejected { .. }) => {
            status_json(StatusCode::CONFLICT, "mode_rejected")
        }
        Err(_) => status_json(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

fn status_json(status: StatusCode, value: &'static str) -> Response {
    (status, Json(json!({ "status": value }))).into_response()
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
