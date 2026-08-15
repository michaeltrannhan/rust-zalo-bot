# ZL expense bot

A privacy-conscious self-hosted assistant that records personal expenses from Zalo chat, extracts receipt data when offered, and returns summaries and insights on demand. This glossary names the domain concepts the product reasons about; it does not prescribe implementation.

## People and identity

**Account**:
The person who consents to use the bot, owns expenses, and may delete their data. Lifecycle states include pending consent, active, suspended, deleting, and deleted.
_Avoid_: User, customer, subscriber

**Provider identity**:
The stable provider-scoped subject that links inbound chat traffic to one account. Identity is the provider sender identifier within a provider scope such as a bot instance, never a display name.
_Avoid_: User identity, sender, chat user

## Ingress and conversation

**Inbound event**:
A normalized, idempotent record of one provider notification after verification and deduplication. It anchors replay protection and records whether the event was accepted, duplicate, or rejected.
_Avoid_: Provider message, webhook payload, raw event

**Conversation state**:
The account's current interactive posture: which pending action interprets the next user text, when that action expires, and the optimistic version that rejects stale replies.
_Avoid_: Pending action, chat state, session

## Receipts and expenses

**Receipt submission**:
The account-owned lifecycle of one receipt offer from chat through confirmation, rejection, failure, expiry, or deletion. It is the unit of duplicate protection for receipt content.
_Avoid_: Receipt document, receipt record, upload

**Receipt asset**:
The stored original bytes of a receipt submission, including content hash, media metadata, retention deadline, and deletion state. An asset may be deleted while the confirmed expense remains.
_Avoid_: Receipt file, receipt image, receipt document

**Extraction attempt**:
One structured-data inference run against a receipt asset, recording provider profile, prompt or schema version, latency, token usage, and a classified outcome. Multiple attempts may exist for one submission.
_Avoid_: OCR run, extraction job, parse

**Expense draft**:
Normalized candidate financial fields and confidence for a receipt submission or manual entry awaiting account confirmation. It is mutable until confirmed, rejected, or expired.
_Avoid_: Draft transaction, pending expense, candidate

**Expense**:
A confirmed immutable financial fact in minor units and ISO currency, with provenance from receipt or manual entry. Corrections amend facts explicitly rather than silently rewriting history.
_Avoid_: Transaction, confirmed expense, ledger entry

**Correction**:
A first-class account fix to a predicted or confirmed field, recorded for audit and preference learning. Corrections do not erase the prior value.
_Avoid_: Edit, amendment, override

## Durable work

**Job**:
A versioned unit of deferred work with priority, scheduled run time, deduplication key, and optional serialization key that constrains concurrent execution for one account or resource.
_Avoid_: Task, queue item, work item

**Job attempt**:
One execution episode of a job, with start and end timing and a classified outcome. A lost lease or heartbeat invalidates the attempt without committing completion.
_Avoid_: Run, execution, try

**Lease**:
A time-bounded claim on a job attempt that grants exclusive right to complete or fail it. Completion after lease expiry or by a different worker is invalid.
_Avoid_: Claim token, visibility timeout, lock

**Dead job**:
A job that exhausted retries and requires operator review before any requeue. Dead is a job state, not an outbound delivery state.
_Avoid_: Failed job, poison message, DLQ item

## Outbound delivery

**Outbound message**:
A durable intent to send one provider chat message, with idempotency key, delivery state, and optional provider message identifier when known.
_Avoid_: Outbound record, notification, outbox row

**Ambiguous delivery**:
An outbound attempt whose provider outcome is unknown—timeout, malformed success, or other indeterminate response—so automatic resend is forbidden until reconciled.
_Avoid_: Unknown send, partial failure, stuck sending

## Schedules, insights, and deletion

**Schedule**:
An account-local preference for when recurring insight snapshots should be delivered, expressed in the account timezone with a computed next run instant.
_Avoid_: Summary schedule, cron, reminder

**Insight snapshot**:
A point-in-time aggregate for a bounded period—totals, trends, category shifts, recurring merchants, budget drift—with optional machine-generated narrative derived only from aggregate structured data.
_Avoid_: Insight, summary, report

**Deletion request**:
The audited progress of removing an account's domain data, pending jobs, object-store originals, and provider-side artifacts where supported. Active deletion blocks new receipt, schedule, and outbound work for that account.
_Avoid_: Delete saga, account wipe, purge

## Domain invariants

**Ambiguous send discipline**:
Ambiguous delivery never triggers an automatic resend; reconciliation or operator action is required.

**Deletion wins**:
An account in active deletion cannot recreate user content through receipts, schedules, or outbound work.

**Retention without erasing facts**:
Original receipt assets expire and may be deleted early; confirmed expenses and explicit corrections remain unless the account is deleted.

**Deterministic insight floor**:
Insight snapshots remain usable from deterministic aggregates when optional narrative generation is disabled or fails.
