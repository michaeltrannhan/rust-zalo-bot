//! Conservative Milestone 2 outbox delivery through the real provider adapter.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ErrorClass;
use crate::provider::ZaloHttpAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Sent,
    Failed,
    Ambiguous,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryResult {
    pub outbound_id: Uuid,
    pub state: DeliveryState,
}

#[derive(Debug, thiserror::Error)]
#[error("outbound persistence operation failed")]
pub struct OutboundStoreError;

/// Reserve and deliver at most one queued Zalo message.
///
/// Reservation commits before HTTP. An indeterminate provider result becomes
/// `ambiguous`; a crash after reservation leaves `sending`. Neither state is
/// selected again automatically, so M2 cannot blindly duplicate an effect.
pub async fn deliver_next(
    pool: &PgPool,
    adapter: &ZaloHttpAdapter,
) -> Result<Option<DeliveryResult>, OutboundStoreError> {
    let reserved: Option<(Uuid, String, String)> = sqlx::query_as(
        r#"
        WITH candidate AS (
            SELECT id
            FROM outbound_messages
            WHERE state = 'queued' AND provider_scope = 'zalo_bot'
            ORDER BY created_at, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE outbound_messages AS outbound
        SET state = 'sending',
            attempt_count = attempt_count + 1,
            updated_at = NOW()
        FROM candidate
        WHERE outbound.id = candidate.id
        RETURNING outbound.id, outbound.provider_target, outbound.body
        "#,
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| OutboundStoreError)?;

    let Some((outbound_id, provider_target, body)) = reserved else {
        return Ok(None);
    };

    match adapter.send_message(&provider_target, &body).await {
        Ok(sent) => {
            let provider_message_id =
                (!sent.provider_message_id.is_empty()).then_some(sent.provider_message_id);
            update_delivery(
                pool,
                outbound_id,
                DeliveryState::Sent,
                provider_message_id.as_deref(),
                None,
            )
            .await?;
            Ok(Some(DeliveryResult {
                outbound_id,
                state: DeliveryState::Sent,
            }))
        }
        Err(error) => {
            let state = if error.class == ErrorClass::ProviderAmbiguous {
                DeliveryState::Ambiguous
            } else {
                DeliveryState::Failed
            };
            update_delivery(pool, outbound_id, state, None, Some(error.class.as_str())).await?;
            Ok(Some(DeliveryResult { outbound_id, state }))
        }
    }
}

async fn update_delivery(
    pool: &PgPool,
    outbound_id: Uuid,
    state: DeliveryState,
    provider_message_id: Option<&str>,
    error_class: Option<&str>,
) -> Result<(), OutboundStoreError> {
    let updated = sqlx::query(
        r#"
        UPDATE outbound_messages
        SET state = $2,
            provider_message_id = $3,
            last_error_class = $4,
            updated_at = NOW()
        WHERE id = $1 AND state = 'sending'
        "#,
    )
    .bind(outbound_id)
    .bind(state.as_str())
    .bind(provider_message_id)
    .bind(error_class)
    .execute(pool)
    .await
    .map_err(|_| OutboundStoreError)?;
    if updated.rows_affected() != 1 {
        return Err(OutboundStoreError);
    }
    Ok(())
}
