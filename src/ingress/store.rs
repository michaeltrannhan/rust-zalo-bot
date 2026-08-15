//! PostgreSQL-backed ingress persistence and transactional processing.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::receipt::{
    AcceptSubmissionRequest, ConfirmRequest, EditDraftRequest, ReceiptLifecycle, RejectRequest,
};
use crate::work::{EnqueueRequest, WorkStore};

use super::effects::{IngressEffect, IngressEffectError};
use super::types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressObservation, IngressOutcome,
    IngressRequest, LifecycleState, PendingAction, ReceiptDraftSnapshot, RecentExpense,
    ReplyIntent,
};

/// Ingress persistence and transactional orchestration.
#[derive(Clone)]
pub struct IngressStore {
    pool: PgPool,
    receipt: Option<ReceiptLifecycle>,
}

/// Ingress store operational failure.
#[derive(Debug)]
pub struct IngressError {
    pub message: String,
}

impl IngressError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for IngressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for IngressError {}

impl IngressStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            receipt: None,
        }
    }

    pub fn with_receipt(pool: PgPool, receipt: ReceiptLifecycle) -> Self {
        Self {
            pool,
            receipt: Some(receipt),
        }
    }

    pub fn receipt(&self) -> Option<&ReceiptLifecycle> {
        self.receipt.as_ref()
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Process one normalized ingress event inside a single database transaction.
    pub async fn process<F>(
        &self,
        request: IngressRequest,
        observation: IngressObservation,
        decide: F,
    ) -> Result<IngressOutcome, IngressError>
    where
        F: FnOnce(DecisionContext) -> Result<DecisionOutput, IngressEffectError>,
    {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| IngressError::new("failed to begin transaction"))?;

        let mode: String =
            sqlx::query_scalar("SELECT mode FROM ingress_control WHERE id = 1 FOR SHARE")
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| IngressError::new("failed to read ingress mode"))?;

        let expected_mode = request.source.as_str();
        if mode != expected_mode {
            let outcome =
                insert_observed_event(&mut tx, &request, &observation, "rejected", None, None)
                    .await?;
            if matches!(outcome, IngressOutcome::Duplicate { .. }) {
                tx.commit()
                    .await
                    .map_err(|_| IngressError::new("failed to commit duplicate"))?;
                return Ok(outcome);
            }
            tx.commit()
                .await
                .map_err(|_| IngressError::new("failed to commit mode rejection"))?;
            return Ok(match outcome {
                IngressOutcome::Accepted { inbound_event_id } => {
                    IngressOutcome::ModeRejected { inbound_event_id }
                }
                other => other,
            });
        }

        let insert_outcome =
            insert_observed_event(&mut tx, &request, &observation, "accepted", None, None).await?;
        let inbound_event_id = match insert_outcome {
            IngressOutcome::Accepted { inbound_event_id } => inbound_event_id,
            IngressOutcome::Duplicate { inbound_event_id } => {
                tx.commit()
                    .await
                    .map_err(|_| IngressError::new("failed to commit duplicate"))?;
                return Ok(IngressOutcome::Duplicate { inbound_event_id });
            }
            IngressOutcome::ModeRejected { .. } => {
                return Err(IngressError::new(
                    "unexpected mode rejection on accepted path",
                ));
            }
        };

        let account_id = if request.sender_allowed {
            Some(
                resolve_or_create_account(&mut tx, &request)
                    .await
                    .map_err(|e| IngressError::new(e.to_string()))?,
            )
        } else {
            None
        };

        let context = if let Some(account_id) = account_id {
            load_decision_context(&mut tx, account_id, inbound_event_id, &request)
                .await
                .map_err(|e| IngressError::new(e.to_string()))?
        } else {
            DecisionContext {
                account_id: None,
                inbound_event_id: Some(inbound_event_id),
                lifecycle_state: None,
                consent_version: None,
                pending_action: None,
                confirmed_today_total_minor: 0,
                confirmed_today_count: 0,
                today_currency: "VND".to_string(),
                recent_expenses: Vec::new(),
                sender_allowed: false,
                user_text: request.user_text.clone(),
                timezone: "Asia/Ho_Chi_Minh".to_string(),
                original_receipt_retention_days: 7,
                next_expense_id: Uuid::new_v4(),
                next_submission_id: Uuid::new_v4(),
                next_ingest_job_id: Uuid::new_v4(),
                now: request.observed_at,
            }
        };

        let decision = decide(context).map_err(|e| IngressError::new(e.to_string()))?;

        if let Some(account_id) = account_id {
            apply_effects(
                &mut tx,
                account_id,
                inbound_event_id,
                self.receipt.as_ref(),
                &decision.effects,
            )
            .await
            .map_err(|e| IngressError::new(e.to_string()))?;
        } else if decision
            .effects
            .iter()
            .any(|effect| !matches!(effect, IngressEffect::ReadOnly))
        {
            return Err(IngressError::new(
                "domain effects require an allowed sender account",
            ));
        }

        if let Some(reply) = decision.reply {
            enqueue_reply(&mut tx, inbound_event_id, account_id, &request, &reply)
                .await
                .map_err(|e| IngressError::new(e.to_string()))?;
        }

        sqlx::query(
            "UPDATE inbound_events SET account_id = $2, processed_at = NOW() WHERE id = $1",
        )
        .bind(inbound_event_id)
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| IngressError::new("failed to finalize inbound event"))?;

        tx.commit()
            .await
            .map_err(|_| IngressError::new("failed to commit ingress transaction"))?;

        Ok(IngressOutcome::Accepted { inbound_event_id })
    }
}

async fn insert_observed_event(
    tx: &mut Transaction<'_, Postgres>,
    request: &IngressRequest,
    observation: &IngressObservation,
    processing_state: &str,
    account_id: Option<Uuid>,
    processed_at: Option<DateTime<Utc>>,
) -> Result<IngressOutcome, IngressError> {
    let event_id = Uuid::new_v4();
    let event_kind = observation.event_kind.as_str();
    let inserted: Option<Uuid> = if processing_state == "accepted" {
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_events (
                id,
                provider_event_id,
                provider_scope,
                kind,
                processing_state,
                ingress_source,
                account_id,
                processed_at,
                media_url,
                provider_chat_id
            )
            VALUES ($1, $2, $3, $4, 'accepted', $5, $6, $7, $8, $9)
            ON CONFLICT (provider_scope, provider_event_id) DO UPDATE
            SET processing_state = 'accepted',
                ingress_source = EXCLUDED.ingress_source,
                account_id = EXCLUDED.account_id,
                processed_at = EXCLUDED.processed_at,
                kind = EXCLUDED.kind,
                media_url = EXCLUDED.media_url,
                provider_chat_id = EXCLUDED.provider_chat_id
            WHERE inbound_events.processing_state = 'rejected'
            RETURNING id
            "#,
        )
        .bind(event_id)
        .bind(&request.provider_event_id)
        .bind(&request.provider_scope)
        .bind(event_kind)
        .bind(request.source.as_str())
        .bind(account_id)
        .bind(processed_at)
        .bind(&observation.media_url)
        .bind(&request.provider_chat_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| IngressError::new("failed to insert or promote inbound event"))?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_events (
                id,
                provider_event_id,
                provider_scope,
                kind,
                processing_state,
                ingress_source,
                account_id,
                processed_at,
                media_url,
                provider_chat_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (provider_scope, provider_event_id) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(event_id)
        .bind(&request.provider_event_id)
        .bind(&request.provider_scope)
        .bind(event_kind)
        .bind(processing_state)
        .bind(request.source.as_str())
        .bind(account_id)
        .bind(processed_at)
        .bind(&observation.media_url)
        .bind(&request.provider_chat_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| IngressError::new("failed to insert inbound event"))?
    };

    if let Some(id) = inserted {
        return Ok(IngressOutcome::Accepted {
            inbound_event_id: id,
        });
    }

    let existing: Uuid = sqlx::query_scalar(
        "SELECT id FROM inbound_events WHERE provider_scope = $1 AND provider_event_id = $2",
    )
    .bind(&request.provider_scope)
    .bind(&request.provider_event_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| IngressError::new("failed to load duplicate inbound event"))?;

    Ok(IngressOutcome::Duplicate {
        inbound_event_id: existing,
    })
}

async fn resolve_or_create_account(
    tx: &mut Transaction<'_, Postgres>,
    request: &IngressRequest,
) -> Result<Uuid, IngressEffectError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT account_id
        FROM provider_identities
        WHERE provider_scope = $1 AND provider_sender_id = $2
        "#,
    )
    .bind(&request.provider_scope)
    .bind(&request.provider_sender_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    if let Some(account_id) = existing {
        return Ok(account_id);
    }

    let account_id = Uuid::new_v4();
    let identity_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state)
        VALUES ($1, 'pending_consent')
        "#,
    )
    .bind(account_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    sqlx::query(
        r#"
        INSERT INTO provider_identities (id, account_id, provider_scope, provider_sender_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(identity_id)
    .bind(account_id)
    .bind(&request.provider_scope)
    .bind(&request.provider_sender_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    Ok(account_id)
}

async fn load_decision_context(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    inbound_event_id: Uuid,
    request: &IngressRequest,
) -> Result<DecisionContext, IngressEffectError> {
    let account_row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT lifecycle_state, consent_version FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;

    let (lifecycle_state, consent_version) = account_row.ok_or(IngressEffectError::NotFound)?;
    let lifecycle = lifecycle_state
        .parse::<LifecycleState>()
        .map_err(|_| IngressEffectError::InvalidTransition)?;

    type PendingActionRow = (Option<String>, Option<String>, Option<DateTime<Utc>>, i32);
    let pending_row: Option<PendingActionRow> = sqlx::query_as(
        r#"
        SELECT pending_action_type, pending_payload_ref, expires_at, version
        FROM conversation_states
        WHERE account_id = $1
        "#,
    )
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    let account_preferences: (String, i32) =
        sqlx::query_as("SELECT timezone, retention_preference_days FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;
    let (timezone, retention_days) = account_preferences;

    let today_total: Option<(i64, i64, String)> = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(amount_minor), 0)::BIGINT,
               COUNT(*)::BIGINT,
               COALESCE(MIN(currency), 'VND')
        FROM expenses
        WHERE account_id = $1
          AND state = 'confirmed'
          AND (occurred_at AT TIME ZONE $2)::date = ($3::timestamptz AT TIME ZONE $2)::date
        "#,
    )
    .bind(account_id)
    .bind(&timezone)
    .bind(request.observed_at)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    let (confirmed_today_total_minor, confirmed_today_count, today_currency) =
        today_total.unwrap_or((0, 0, "VND".to_string()));

    type RecentExpenseRow = (
        Uuid,
        i64,
        String,
        DateTime<Utc>,
        String,
        String,
        String,
        i32,
    );

    let recent_rows: Vec<RecentExpenseRow> = sqlx::query_as(
        r#"
        SELECT id, amount_minor, currency, occurred_at, description, source, state, version
        FROM expenses
        WHERE account_id = $1
        ORDER BY occurred_at DESC, created_at DESC
        LIMIT 10
        "#,
    )
    .bind(account_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    let recent_expenses: Vec<RecentExpense> = recent_rows
        .into_iter()
        .filter_map(
            |(id, amount_minor, currency, occurred_at, description, source, state, version)| {
                state
                    .parse::<ExpenseState>()
                    .ok()
                    .map(|expense_state| RecentExpense {
                        id,
                        amount_minor,
                        currency,
                        occurred_at,
                        description,
                        source,
                        state: expense_state,
                        version,
                    })
            },
        )
        .collect();

    let pending_action = match pending_row {
        Some((Some(action_type), payload_ref, Some(expires_at), version))
            if action_type == "manual_expense_confirmation" =>
        {
            let expense_id = payload_ref
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok());
            let expense = if let Some(expense_id) = expense_id {
                if let Some(expense) = recent_expenses
                    .iter()
                    .find(|expense| expense.id == expense_id)
                    .cloned()
                {
                    Some(expense)
                } else {
                    let row: Option<RecentExpenseRow> = sqlx::query_as(
                        r#"
                        SELECT id, amount_minor, currency, occurred_at, description, source, state, version
                        FROM expenses
                        WHERE account_id = $1 AND id = $2
                        "#,
                    )
                    .bind(account_id)
                    .bind(expense_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|_| IngressEffectError::InvalidTransition)?;
                    row.and_then(
                        |(
                            id,
                            amount_minor,
                            currency,
                            occurred_at,
                            description,
                            source,
                            state,
                            version,
                        )| {
                            state
                                .parse::<ExpenseState>()
                                .ok()
                                .map(|expense_state| RecentExpense {
                                    id,
                                    amount_minor,
                                    currency,
                                    occurred_at,
                                    description,
                                    source,
                                    state: expense_state,
                                    version,
                                })
                        },
                    )
                }
            } else {
                None
            };
            Some(PendingAction {
                action_type,
                payload_ref,
                expires_at,
                version,
                expense,
                receipt_draft: None,
            })
        }
        Some((Some(action_type), payload_ref, Some(expires_at), version))
            if action_type == "receipt_review" =>
        {
            let submission_id = payload_ref
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok());
            let receipt_draft = if let Some(submission_id) = submission_id {
                load_receipt_draft_snapshot(tx, account_id, submission_id).await?
            } else {
                None
            };
            Some(PendingAction {
                action_type,
                payload_ref,
                expires_at,
                version,
                expense: None,
                receipt_draft,
            })
        }
        _ => None,
    };

    Ok(DecisionContext {
        account_id: Some(account_id),
        inbound_event_id: Some(inbound_event_id),
        lifecycle_state: Some(lifecycle),
        consent_version,
        pending_action,
        confirmed_today_total_minor,
        confirmed_today_count: confirmed_today_count as u32,
        today_currency,
        recent_expenses,
        sender_allowed: true,
        user_text: request.user_text.clone(),
        timezone,
        original_receipt_retention_days: retention_days as u32,
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        now: request.observed_at,
    })
}

type ReceiptDraftRow = (
    Uuid,
    i64,
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    i32,
);

async fn load_receipt_draft_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    submission_id: Uuid,
) -> Result<Option<ReceiptDraftSnapshot>, IngressEffectError> {
    let row: Option<ReceiptDraftRow> = sqlx::query_as(
        r#"
        SELECT
            d.submission_id,
            d.amount_minor,
            d.currency,
            d.merchant,
            c.display_name_vi,
            d.transaction_type,
            d.occurred_at,
            d.version
        FROM expense_drafts d
        JOIN categories c ON c.key = d.category_key
        JOIN receipt_submissions s
          ON s.id = d.submission_id
         AND s.account_id = d.account_id
        WHERE d.submission_id = $1
          AND d.account_id = $2
          AND s.lifecycle_state = 'review_required'
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    Ok(row.map(
        |(
            submission_id,
            amount_minor,
            currency,
            merchant,
            category_display,
            transaction_type,
            occurred_at,
            version,
        )| ReceiptDraftSnapshot {
            submission_id,
            amount_minor,
            currency,
            merchant,
            category_display,
            transaction_type,
            occurred_at,
            version,
        },
    ))
}

async fn apply_effects(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    inbound_event_id: Uuid,
    receipt: Option<&ReceiptLifecycle>,
    effects: &[IngressEffect],
) -> Result<(), IngressEffectError> {
    for effect in effects {
        apply_effect(tx, account_id, inbound_event_id, receipt, effect).await?;
    }
    Ok(())
}

async fn apply_effect(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    inbound_event_id: Uuid,
    receipt: Option<&ReceiptLifecycle>,
    effect: &IngressEffect,
) -> Result<(), IngressEffectError> {
    match effect {
        IngressEffect::ReadOnly => {}
        IngressEffect::GrantConsent { consent_version } => {
            let updated = sqlx::query(
                r#"
                UPDATE accounts
                SET lifecycle_state = 'active',
                    consent_version = $2,
                    consented_at = NOW(),
                    updated_at = NOW()
                WHERE id = $1 AND lifecycle_state = 'pending_consent'
                "#,
            )
            .bind(account_id)
            .bind(consent_version)
            .execute(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;
            if updated.rows_affected() == 0 {
                return Err(IngressEffectError::NotFound);
            }
        }
        IngressEffect::CreateManualExpenseAwaitingConfirmation {
            expense_id,
            amount_minor,
            currency,
            description,
            occurred_at,
            optimistic_version,
            pending_expires_at,
            pending_action_type,
        } => {
            sqlx::query(
                r#"
                INSERT INTO expenses (
                    id, account_id, amount_minor, currency, occurred_at, description, source, state,
                    version
                )
                VALUES ($1, $2, $3, $4, $5, $6, 'manual', 'awaiting_confirmation', $7)
                "#,
            )
            .bind(expense_id)
            .bind(account_id)
            .bind(amount_minor)
            .bind(currency)
            .bind(occurred_at)
            .bind(description)
            .bind(optimistic_version)
            .execute(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;

            upsert_pending_action(
                tx,
                account_id,
                pending_action_type,
                Some(expense_id.to_string()),
                *pending_expires_at,
                None,
            )
            .await?;
        }
        IngressEffect::ConfirmExpense {
            expense_id,
            expected_version,
        } => {
            let updated = sqlx::query(
                r#"
                UPDATE expenses
                SET state = 'confirmed', version = version + 1, updated_at = NOW()
                WHERE id = $1 AND account_id = $2 AND version = $3 AND state = 'awaiting_confirmation'
                "#,
            )
            .bind(expense_id)
            .bind(account_id)
            .bind(expected_version)
            .execute(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;
            if updated.rows_affected() == 0 {
                return Err(IngressEffectError::VersionConflict);
            }
        }
        IngressEffect::RejectExpense {
            expense_id,
            expected_version,
        } => {
            let updated = sqlx::query(
                r#"
                UPDATE expenses
                SET state = 'rejected', version = version + 1, updated_at = NOW()
                WHERE id = $1 AND account_id = $2 AND version = $3 AND state = 'awaiting_confirmation'
                "#,
            )
            .bind(expense_id)
            .bind(account_id)
            .bind(expected_version)
            .execute(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;
            if updated.rows_affected() == 0 {
                return Err(IngressEffectError::VersionConflict);
            }
        }
        IngressEffect::SetPendingAction {
            action_type,
            payload_ref,
            expires_at,
            expected_version,
        } => {
            upsert_pending_action(
                tx,
                account_id,
                action_type,
                payload_ref.clone(),
                *expires_at,
                *expected_version,
            )
            .await?;
        }
        IngressEffect::ClearPendingAction { expected_version } => {
            let updated = sqlx::query(
                r#"
                UPDATE conversation_states
                SET pending_action_type = NULL,
                    pending_payload_ref = NULL,
                    expires_at = NULL,
                    version = version + 1,
                    updated_at = NOW()
                WHERE account_id = $1 AND version = $2
                "#,
            )
            .bind(account_id)
            .bind(expected_version)
            .execute(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;
            if updated.rows_affected() == 0 {
                return Err(IngressEffectError::VersionConflict);
            }
        }
        IngressEffect::AcceptReceiptSubmission {
            submission_id,
            ingest_job_id,
            inbound_event_id: effect_event_id,
        } => {
            let receipt = receipt.ok_or(IngressEffectError::InvalidTransition)?;
            if *effect_event_id != inbound_event_id {
                return Err(IngressEffectError::InvalidTransition);
            }
            receipt
                .accept_submission_in_transaction(
                    tx,
                    AcceptSubmissionRequest {
                        submission_id: *submission_id,
                        account_id,
                        inbound_event_id: Some(inbound_event_id),
                        ingest_job_id: *ingest_job_id,
                    },
                )
                .await
                .map_err(map_receipt_error)?;
        }
        IngressEffect::ConfirmReceipt {
            submission_id,
            expense_id,
            expected_draft_version,
        } => {
            let receipt = receipt.ok_or(IngressEffectError::InvalidTransition)?;
            receipt
                .confirm_in_transaction(
                    tx,
                    ConfirmRequest {
                        account_id,
                        submission_id: *submission_id,
                        expected_draft_version: *expected_draft_version,
                        expense_id: *expense_id,
                    },
                )
                .await
                .map_err(map_receipt_error)?;
        }
        IngressEffect::RejectReceipt {
            submission_id,
            expected_draft_version,
        } => {
            let receipt = receipt.ok_or(IngressEffectError::InvalidTransition)?;
            receipt
                .reject_in_transaction(
                    tx,
                    RejectRequest {
                        account_id,
                        submission_id: *submission_id,
                        expected_draft_version: *expected_draft_version,
                    },
                )
                .await
                .map_err(map_receipt_error)?;
        }
        IngressEffect::EditReceiptDraft {
            submission_id,
            expected_draft_version,
            amount_minor,
        } => {
            let receipt = receipt.ok_or(IngressEffectError::InvalidTransition)?;
            receipt
                .edit_draft_in_transaction(
                    tx,
                    EditDraftRequest {
                        account_id,
                        submission_id: *submission_id,
                        expected_version: *expected_draft_version,
                        amount_minor: Some(*amount_minor),
                        currency: None,
                        merchant: None,
                        category_key: None,
                        occurred_at: None,
                    },
                )
                .await
                .map_err(map_receipt_error)?;
        }
    }
    Ok(())
}

fn map_receipt_error(error: crate::receipt::ReceiptError) -> IngressEffectError {
    use crate::error::ErrorClass;
    match error.class {
        ErrorClass::Conflict => IngressEffectError::VersionConflict,
        ErrorClass::NotFound => IngressEffectError::NotFound,
        _ => IngressEffectError::InvalidTransition,
    }
}

async fn upsert_pending_action(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    action_type: &str,
    payload_ref: Option<String>,
    expires_at: DateTime<Utc>,
    expected_version: Option<i32>,
) -> Result<(), IngressEffectError> {
    if let Some(expected) = expected_version {
        let updated = sqlx::query(
            r#"
            UPDATE conversation_states
            SET pending_action_type = $2,
                pending_payload_ref = $3,
                expires_at = $4,
                version = version + 1,
                updated_at = NOW()
            WHERE account_id = $1 AND version = $5
            "#,
        )
        .bind(account_id)
        .bind(action_type)
        .bind(payload_ref)
        .bind(expires_at)
        .bind(expected)
        .execute(&mut **tx)
        .await
        .map_err(|_| IngressEffectError::InvalidTransition)?;
        if updated.rows_affected() == 0 {
            return Err(IngressEffectError::VersionConflict);
        }
        return Ok(());
    }

    let inserted = sqlx::query(
        r#"
        INSERT INTO conversation_states (
            account_id, pending_action_type, pending_payload_ref, expires_at, version
        )
        VALUES ($1, $2, $3, $4, 1)
        ON CONFLICT (account_id) DO NOTHING
        "#,
    )
    .bind(account_id)
    .bind(action_type)
    .bind(&payload_ref)
    .bind(expires_at)
    .execute(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    if inserted.rows_affected() == 1 {
        return Ok(());
    }

    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM conversation_states WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?;

    let updated = sqlx::query(
        r#"
        UPDATE conversation_states
        SET pending_action_type = $2,
            pending_payload_ref = $3,
            expires_at = $4,
            version = version + 1,
            updated_at = NOW()
        WHERE account_id = $1 AND version = $5
        "#,
    )
    .bind(account_id)
    .bind(action_type)
    .bind(&payload_ref)
    .bind(expires_at)
    .bind(current_version)
    .execute(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    if updated.rows_affected() == 0 {
        return Err(IngressEffectError::VersionConflict);
    }
    Ok(())
}

async fn enqueue_reply(
    tx: &mut Transaction<'_, Postgres>,
    inbound_event_id: Uuid,
    account_id: Option<Uuid>,
    request: &IngressRequest,
    reply: &ReplyIntent,
) -> Result<(), IngressEffectError> {
    let idempotency_key = format!(
        "reply:{}:{}",
        request.provider_scope, request.provider_event_id
    );
    let job_dedupe_key = format!("outbound.deliver:{idempotency_key}");
    let serialization_key = match account_id {
        Some(account_id) => format!("account:{account_id}"),
        None => format!(
            "provider_chat:{}:{}",
            request.provider_scope, request.provider_chat_id
        ),
    };

    let outbound_id = Uuid::new_v4();
    let inserted_outbound: Option<Uuid> = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_messages (
            id,
            account_id,
            inbound_event_id,
            idempotency_key,
            provider_scope,
            provider_target,
            body,
            state
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued')
        ON CONFLICT (idempotency_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(outbound_id)
    .bind(account_id)
    .bind(inbound_event_id)
    .bind(&idempotency_key)
    .bind(&request.provider_scope)
    .bind(&request.provider_chat_id)
    .bind(&reply.body)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;

    let outbound_id = if let Some(outbound_id) = inserted_outbound {
        outbound_id
    } else {
        sqlx::query_scalar("SELECT id FROM outbound_messages WHERE idempotency_key = $1")
            .bind(&idempotency_key)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| IngressEffectError::InvalidTransition)?
    };

    WorkStore::enqueue_in_transaction(
        tx,
        EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: "outbound.deliver".to_string(),
            payload: json!({
                "schema_version": 1,
                "outbound_id": outbound_id,
            }),
            dedupe_key: job_dedupe_key,
            serialization_key: Some(serialization_key),
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 10,
        },
    )
    .await
    .map_err(|_| IngressEffectError::InvalidTransition)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::types::IngressSource;

    #[test]
    fn ingress_source_roundtrip() {
        assert_eq!(IngressSource::Webhook.as_str(), "webhook");
        assert_eq!(
            "polling".parse::<IngressSource>().ok(),
            Some(IngressSource::Polling)
        );
    }
}
