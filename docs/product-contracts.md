# Product contracts (Milestone 0)

Measurable contracts for the clean-slate Rust implementation. Authoritative
product plan: `.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. Legacy
Go behavior is discovery material only (`zl-expese-bot`).

**Milestone 0 exit:** No unresolved product decision remains that would force
an initial-schema change or redefine a public test seam. Schema-changing
choices below are closed for the first stable release.

## Supported hosts

| Host | Architecture | Notes |
| --- | --- | --- |
| Debian 12 (Bookworm) | `amd64`, `arm64` | Primary self-host target |
| Ubuntu 22.04 LTS (Jammy) | `amd64`, `arm64` | Supported |
| Ubuntu 24.04 LTS (Noble) | `amd64`, `arm64` | Supported |

PostgreSQL ships in the versioned deployment bundle. Operators terminate public
TLS in a reverse proxy or the optional bundled Caddy profile. The application
listener binds to a private interface by default.

## Target VM, traffic, and resource gates

### Target profile (initial benchmark)

| Dimension | Target |
| --- | --- |
| vCPU | 1 |
| RAM | 1 GiB |
| PostgreSQL | Same host as application (bundle default) |
| Process layout | All-in-one: ingress, worker, scheduler, maintenance |
| Database pool max | 5 connections |
| Receipt extraction concurrency | 1 (default) |
| Outbound delivery concurrency | 4 (default) |
| Webhook burst | 25 requests/second against local PostgreSQL |

### Release gates (subject to calibration on target hardware)

Gates may be adjusted only by recording target, workload artifact, measured
result, and reason — never by silent relaxation.

| Gate | Threshold |
| --- | --- |
| Idle resident memory (app, post warm-up) | ≤ 100 MiB |
| Idle application CPU (10-minute average) | < 1% |
| Readiness when dependencies healthy, no pending migration | ≤ 3 s |
| Webhook acknowledgement p95 (25 rps burst) | < 100 ms |
| Resident memory during standard webhook burst | < 256 MiB (excludes PostgreSQL and reverse proxy) |
| One-hour mixed receipt/command soak | No monotonic memory growth |
| Graceful stop | ≤ 30 s, or jobs left for lease recovery are recorded |
| Queue age after burst | Returns to baseline |

## Eight public test seams

Confirmed 2026-08-15. Tests observe behavior only through these seams. Mock
external HTTP, time, randomness, and optional object storage — never internal
conversation, receipt, job, or outbound modules.

### 1. Conversation seam

**Interface:** `AccountContext` + normalized user input (text command, pending
action resolution, consent state) → `ConversationOutcome` (Vietnamese reply
plan and/or domain command).

**Observable outcomes:**

- Consent required, allowlist rejected, help, unsupported command
- Manual expense recorded or rejected with deterministic Vietnamese explanation
- Period summaries (`today`, `week`, `month`, last week/month) with SQL-backed totals
- Pending action armed, resolved, expired, or superseded
- Settings and schedule commands change stored preferences or return validation errors

### 2. Ingress seam

**Interface:** Verified webhook HTTP request or normalized polling event →
`IngressResult` (HTTP acknowledgement + persisted inbound event record).

**Observable outcomes:**

- `accepted` — new event persisted, fast acknowledgement
- `duplicate` — same provider event ID absorbed, no duplicate domain effect
- `rejected` — authentication, rate, body-size, content-type, or validation failure with stable reason class
- Polling and webhook paths produce identical normalized events and conversation behavior when ingress mode permits

### 3. Receipt seam

**Interface:** Receipt submission (image/media event or receipt lifecycle
command) + account context → observable receipt and draft state.

**Observable outcomes:**

- Lifecycle states: `pending`, `queued`, `stored`, `extracting`, `review_required`, `confirmed`, `rejected`, `failed_permanent`, `failed_transient`, `expired`, `deleted`
- Draft visible for confirm, edit fields, reject, or expiry
- Confirmed expense appears in today/history queries
- Content-hash duplicate absorbed without second expense
- Retention deadline applied; early original deletion without deleting confirmed expense

### 4. Durable-work seam

**Interface:** Versioned job queue against real PostgreSQL — enqueue, claim,
heartbeat, complete, fail, cancel, recover.

**Observable outcomes:**

- At-least-once execution with idempotent effects
- Lease owner, lease deadline, attempt count, and terminal `dead` state observable
- Stale worker cannot complete after lease loss
- Per-account serialization key prevents unsafe parallel mutations
- Operator list/show/retry/cancel affects observable job state

### 5. Outbound seam

**Interface:** Notification intent (reply, summary, card) → observable delivery
state via loopback Zalo HTTP adapter.

**Observable outcomes:**

- States: `queued`, `sending`, `sent`, `failed`, `suppressed`, `ambiguous`
- Duplicate idempotency key does not create duplicate provider send
- Ambiguous provider outcome remains `ambiguous` until reconciled — no blind resend
- Rate limits and kill switches suppress with observable `suppressed`

### 6. Provider HTTP seams

**Interface:** Narrow adapter operations against loopback mock servers —
Zalo (events, media download, send message), Gemini (structured extraction),
S3-compatible object store (put/get/delete).

**Observable outcomes:**

- Correct request serialization, authentication headers, timeouts, and status mapping
- Tolerant parsing of provider JSON; malformed responses classified without logging bodies
- Redirect, oversize, 429, and 5xx paths map to stable error classes
- Redacted logs contain error class and correlation IDs only

### 7. Operator seam

**Interface:** Compiled CLI subprocess — argv, env overrides, stdin prompts for
secrets, stdout/stderr, exit code, filesystem effects under `/etc/zl-expense`,
`/var/lib/zl-expense`, and generated systemd units.

**Observable outcomes:**

- Config get/set/validate/show with atomic writes and source attribution
- Secret set/list never echoes values
- Database check/migrate/backup/restore with advisory lock and restore confirmation
- Ingress mode inspect and audited switch with rollback instructions
- Status/monitor JSON: health, readiness, queues, leases, dead jobs, outbound ambiguity
- Diagnose bundle lists files before creation; contents redacted
- Host install/uninstall produces expected unit files and paths

### 8. Runtime seam

**Interface:** Packaged long-running process — HTTP health, readiness, optional
loopback metrics, graceful shutdown, cgroup-visible resource use, systemd
supervision.

**Observable outcomes:**

- `GET /health/live` vs `GET /health/ready` reflect liveness and dependency readiness separately
- Readiness false before shutdown; new claims stopped; leases released or expired; bounded drain
- Critical task failure cancels siblings and exits non-zero for systemd restart
- Watchdog heartbeat when configured
- Metrics expose queue depth, webhook duplicates, extraction outcomes without sensitive labels

## Stable redaction-safe error classes

Stable string identifiers for logs, metrics, CLI output, and job attempt
records. User-facing Vietnamese text is separate and may evolve; these classes
do not embed provider bodies, paths, tokens, or raw identifiers.

| Class | Meaning | Typical retry |
| --- | --- | --- |
| `validation` | Invalid input, schema, or state transition | No |
| `not_found` | Referenced entity absent | No |
| `conflict` | Optimistic version or state conflict | No |
| `forbidden` | Allowlist, suspension, or ingress mode denial | No |
| `consent_required` | Account has not consented | No |
| `quota_exceeded` | Per-user, per-day, or global budget exhausted | No |
| `unsupported` | Event or media type not handled | No |
| `duplicate` | Idempotent absorption (event, receipt hash, outbound key) | No |
| `auth` | Webhook or provider credential rejection | No |
| `rate_limited` | Local or provider throttle | Yes (backoff) |
| `timeout` | Operation deadline exceeded | Yes |
| `provider_error` | Provider 5xx or unclassified remote failure | Yes |
| `provider_ambiguous` | Send outcome unknown; reconciliation required | Manual |
| `transient` | Database or infrastructure flake | Yes |
| `kill_switch` | Operator-disabled feature path | No |
| `internal` | Unexpected defect; details logged server-side only | No |

Provider and LLM adapters map wire failures into these classes before logging.

## Stable CLI exit-code taxonomy

| Code | Name | When |
| --- | --- | --- |
| 0 | `success` | Command completed; checks passed |
| 1 | `runtime_error` | Unexpected failure (`internal` class or defect) |
| 2 | `usage_error` | Invalid argv, unknown subcommand, missing required flag |
| 3 | `config_error` | Resolved configuration invalid or incomplete |
| 4 | `dependency_error` | Database, object store, or required service unreachable |
| 5 | `migration_error` | Schema migration failed or pending migration blocks operation |
| 6 | `permission_error` | Insufficient filesystem or credential permissions |
| 7 | `conflict_error` | Operation refused due to ingress mode, lock, or unsafe state |
| 8 | `cancelled` | User declined confirmation or interrupt before completion |
| 10 | `preflight_failed` | Update/install preflight checks failed before mutation |
| 11 | `health_failed` | Post-change health or readiness verification failed |

Long-running `run` and `worker` roles use exit code 1 on critical task failure
so systemd `Restart=on-failure` applies. Doctor passive mode uses 0 when
checks pass, 4 for dependency failures, 3 for configuration failures.

## Initial schema decision inventory

Clean-slate PostgreSQL migration series. No Go migration compatibility. Money:
signed 64-bit minor units + ISO 4217 currency. Instants: `timestamptz`; account
local scheduling: IANA timezone identifier. Application-generated stable IDs;
never expose sequential database IDs as provider idempotency keys.

### System invariants

- At most one logical processing outcome exists per provider event identifier;
  duplicate ingress cannot create duplicate domain effects.
- A domain transition and every required job or outbound intent commit in one
  PostgreSQL transaction or none persists.
- Jobs execute at least once; every external or domain effect is idempotent or
  becomes an explicit terminal or ambiguous outcome.
- Only the current lease holder may complete or fail a job attempt. A stale
  worker that loses its lease cannot commit completion.
- Jobs with the same serialization key cannot execute concurrently when their
  effects could reorder account state.
- Webhook and polling ingress are mutually exclusive; polling additionally
  requires the current renewable leader lease.

### Phased table rollout

| Phase | Tables / concepts | Milestone |
| --- | --- | --- |
| P0 | `accounts`, `provider_identities`, `inbound_events`, ingress control record, schema metadata | M1 walking skeleton |
| P1 | `conversation_states`, `outbound_messages`, `jobs`, `job_attempts` | M2–M3 |
| P2 | `receipt_submissions`, `receipt_assets`, `extraction_attempts`, `expense_drafts`, `expenses`, categories seed | M4–M5 |
| P3 | `account_ai_preferences`, `summary_schedules`, `insight_snapshots`, usage budget counters, `deletion_requests` | M6 |
| P4 | Operator audit events, export artifact metadata (no chat delivery) | M6–M7 |

### Categories

- Seed deterministic system categories (Vietnamese display names, stable keys).
- User learns merchant→category rules from corrections; count-based confidence
  (Go reference: `internal/categorisation/categorisation.go`).
- Transaction types: expense, income, refund, transfer, adjustment.
- Default category key `khac` when no rule or extraction hint applies.

### Usage budgets

- Per-user daily receipt submission limit (configurable; Go default 20).
- Per-account and per-day LLM request/token budgets tied to named AI profile.
- Global monthly extraction page/token budget (operator-configured).
- Outbound message quotas aligned with provider limits.
- Counters stored in PostgreSQL; enforcement returns `quota_exceeded` without
  leaking counter internals in chat.

### Ingress control

- Durable record selects `webhook` (production default) or `polling` (explicit fallback).
- Mode generation incremented on audited switch; webhook instances reject
  processing when polling is selected; poller requires exclusive renewable leader lease.
- Mutually exclusive: no poller while webhook mode is active.
- Polling normalizes through the same ingress interface as webhook.

### Retention (7-day default)

- Default original-receipt retention: **7 days** (operator may configure 1–30 days).
- `receipt_assets.retention_deadline` drives bounded batch deletion; confirmed
  expenses and extraction metadata survive original purge.
- User may delete an original earlier without deleting the confirmed expense.
- Account deletion removes object data, export artifacts, and domain rows in
  bounded batches with per-account serialization.

### Duplicate strategy

- **Automatic absorption:** exact SHA-256 of normalized stored bytes per account.
  Unique constraint on `(account_id, content_sha256)` where asset not deleted.
  Duplicate submission → `duplicate` class, `failed_permanent` or absorbed
  state, Vietnamese “already processed” reply, no second expense.
- **Warning only (future):** perceptual hash and soft field matching (amount,
  currency, merchant, ±3-day window) may emit a non-blocking “possible
  duplicate” follow-up. Perceptual hashes are **not** used for automatic
  absorption in the first stable release. Go `PerceptualHash` field and
  `warnPossibleDuplicate` (`internal/receipt/processor.go`) are reference
  behavior for the warning path only.

### Named AI profiles

- Configuration defines named profiles (e.g. `receipt-fast`, `receipt-accurate`,
  `insight`) with: provider, model id, credential reference, timeout/retry,
  max input/output size, thinking effort when supported, structured-output schema
  version, task assignment (extraction, categorization, insight narrative).
- `account_ai_preferences` stores allowed profile per account; no raw API keys
  in chat, database plaintext, or logs.
- Capability validation at config load rejects unsupported thinking settings.
- Insights: deterministic SQL aggregates always available; optional LLM narrative
  from aggregate structured data only, cached, quota-limited, kill-switchable.

### Exports

- Operator-generated CSV/JSON artifacts; metadata row records generation time
  and secure delivery mechanism.
- Delivery only through configured secure mechanisms (local privileged path,
  optional S3 with operator credentials, future signed URL) — **never** chat
  filesystem paths (Go `/export` chat path delivery is explicitly dropped).

### Extraction backends (first stable release)

- In-process mock/deterministic extractor for tests and local runs.
- Gemini HTTP adapter behind named profiles.
- **AWS Textract:** not in first stable release; optional future adapter behind
  the same extraction seam (Go `internal/extraction/textract/register.go` is
  not ported for v1).

### Object storage

- Default: host filesystem under `/var/lib/zl-expense/`.
- Optional: MinIO/S3-compatible adapter behind the same receipt object seam.
- S3 client library choice deferred to measured resource spike (does not change
  table layout).

### Durable work

- `jobs`: versioned JSON payload, state, priority, `run_at`, dedupe key,
  serialization key, attempts, lease owner, lease deadline.
- Domain transition and job/outbound enqueue in one transaction.
- Terminal job state `dead`; outbound uses `failed` and `ambiguous` separately.

## Cross-reference

| Artifact | Owner milestone |
| --- | --- |
| Domain vocabulary and domain invariants | `CONTEXT.md` (M0 sibling) |
| Architecture ADR | ADR (M0 sibling) |
| Threat model | `docs/threat-model.md` (M0 sibling) |
| Legacy capability disposition | `docs/legacy-capabilities.md` |
