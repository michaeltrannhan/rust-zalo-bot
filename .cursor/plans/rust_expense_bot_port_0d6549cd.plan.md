---
name: Production Rust expense bot
overview: Build a clean-slate, production-ready Rust implementation of the Zalo expense bot for privacy-conscious self-hosting. Ship webhook-first ingress with polling fallback, a redesigned PostgreSQL model, durable work execution, configurable Gemini profiles, low-resource runtime modes, an operator CLI, systemd integration, and signed amd64/arm64 packages.
todos:
  - id: decisions-and-contracts
    content: Record product decisions, domain vocabulary, public test seams, production SLOs, privacy defaults, and the target VM profile
    status: pending
  - id: walking-skeleton
    content: Ship a minimal installable Rust binary with config, credentials, PostgreSQL migration, health endpoints, journald logs, and one red-green tracer test
    status: pending
  - id: durable-work
    content: Implement transactional jobs/outbox, leases, heartbeats, retries, idempotency, dead-letter recovery, bounded concurrency, and failure-injection tests
    status: pending
  - id: conversation-slice
    content: Implement consent, allowlist, Vietnamese command parsing, pending actions, and deterministic replies through public behavior tests
    status: pending
  - id: receipt-slice
    content: Implement receipt ingestion, asset retention, extraction attempts, draft confirmation, expense recording, and duplicate protection
    status: pending
  - id: ingress-and-providers
    content: Implement secure webhook ingress, mutually exclusive polling fallback, Zalo/Gemini/object-store adapters, and loopback HTTP contract tests
    status: pending
  - id: notifications-insights
    content: Implement durable outbound delivery, schedules, deterministic insights, and optional aggregate-only LLM narratives
    status: pending
  - id: operator-experience
    content: Implement status, monitor, logs, diagnose, backup, job recovery, systemd install, and safe configuration/secret commands
    status: pending
  - id: performance-hardening
    content: Enforce resource budgets on target hardware, run load/soak/failure tests, harden security and privacy behavior, and tune bounded concurrency
    status: pending
  - id: package-and-update
    content: Publish signed amd64/arm64 Debian packages and portable bundles; verify clean install, upgrade, health check, and rollback
    status: pending
isProject: false
---

# Production Rust expense bot

## Product decision

This is a new Rust product, not a database-compatible rewrite of the Go program.

- Port the useful user-visible behavior from /Users/michaeltrannhan/Desktop/hobby/zl-expese-bot.
- Redesign the internals, PostgreSQL schema, process lifecycle, packaging, and operator experience.
- Do not share a database with Go and do not copy the Go migration ledger.
- Treat Go tests and behavior as discovery material, not as an immutable source of truth.
- Optimize for robustness, predictable memory use, low idle CPU, secure self-hosting, and simple operation.
- Make webhook ingress the production default. Keep polling as an explicit fallback.
- Ship a versioned package that needs no repository checkout or Rust/Go toolchain.

Deprovisioning or importing data from the Go deployment is a separate future project.

## Production definition

Production-ready means the first stable release must have all of the following:

1. Authenticated public HTTPS webhook ingress with replay and duplicate protection.
2. Durable, observable, recoverable background work.
3. Safe handling of retries and ambiguous external side effects.
4. A fresh, versioned PostgreSQL schema with tested migrations.
5. Bounded CPU, memory, database pools, queues, image sizes, and external request concurrency.
6. Structured journald logging, health/readiness checks, metrics, and redacted diagnostics.
7. A clean install and upgrade path on supported Debian/Ubuntu amd64 and arm64 hosts.
8. Backups, restore rehearsal, and Rust-to-Rust migration/rollback rules.
9. Explicit privacy defaults and documentation of data sent to Zalo, Gemini, and any S3-compatible store.
10. Security, load, soak, crash-recovery, and target-host package tests.

The first stable release does not claim high availability across regions or unlimited horizontal scale. It must run well on one small VM and allow ingress and worker roles to be split later.

## Supported deployment profiles

### Small VM, default

- One zl-expense process supervised by one systemd unit.
- The process runs ingress, worker, scheduler, and maintenance roles under one task supervisor.
- PostgreSQL runs from the versioned deployment bundle.
- Receipt objects use the host filesystem by default; MinIO/S3 is optional.
- Small database pool and bounded receipt/LLM concurrency minimize RAM.

This is the lowest-resource and simplest self-hosted profile.

### Split roles

- Ingress, workers, and scheduler run as separate process roles from the same binary.
- All durable coordination remains in PostgreSQL.
- More worker instances may be added only after jobs have proven lease, idempotency, and per-account serialization behavior.
- Multiple webhook replicas may run in webhook mode; inbound-event uniqueness absorbs duplicate delivery.
- Polling has one database-elected leader, and no poller may run while the configured ingress mode is webhook.
- Scheduler instances use the same leader/lease discipline so a schedule is emitted once logically.

The public behavior must be identical in both profiles.

## Runtime shape

    Public HTTPS
         |
    webhook ingress ----\
                         > normalized inbound event -> PostgreSQL transaction
    polling fallback ---/                              | domain transition
                                                        | durable job/outbox
                                                        v
                                               bounded worker execution
                                                        |
                                      Zalo / Gemini / object-store adapters

The default all-in-one process uses structured concurrency:

- A critical task failure cancels sibling tasks and exits non-zero so systemd can restart the process.
- Readiness becomes false before shutdown begins.
- Shutdown stops new claims, releases or expires leases safely, drains bounded in-flight work until a deadline, then exits.
- A worker that loses its lease or heartbeat cancels the attempt and may not commit completion.
- In-memory channels are bounded and never become the source of truth.

## Rust technical baseline

- Pin the stable Rust toolchain in rust-toolchain.toml and commit Cargo.lock.
- Use Tokio for asynchronous execution, Axum/Tower for HTTP, Clap for CLI parsing, SQLx for PostgreSQL, Serde for wire formats, and Tracing for structured logs.
- Use Reqwest with Rustls for outbound HTTP so packages do not depend on a host OpenSSL layout.
- Use compile-checked SQL with committed offline metadata where practical; dynamic operator queries must still be covered by PostgreSQL integration tests.
- Use a library with IANA timezone and daylight-saving support rather than fixed offsets.
- Keep provider wire types inside their adapters and map them immediately to domain types.
- Disable unused dependency features. Compare release binary size and idle memory before accepting large SDKs.
- Select the S3-compatible client only after the measured spike described under resource budgets.
- Treat unsafe Rust as prohibited unless a narrowly documented need, safety argument, and focused review justify it.

## Deep modules and seams

Organize around cohesive behavior rather than mirroring Go packages one-for-one.

### Ingress

Own provider request verification, request limits, replay protection, event normalization, idempotent acceptance, and fast acknowledgement. It exposes normalized inbound events rather than Zalo response shapes.

### Conversation

Own consent, allowlist decisions, commands, pending actions, Vietnamese replies, and conversation state transitions. It has no knowledge of HTTP or SQL.

### Receipt lifecycle

Own receipt submission, asset state, extraction attempts, deduplication, expense drafts, confirmation, rejection, expiry, and retention. Callers should not orchestrate its internal row-by-row workflow.

### Durable work

Own transactional enqueue, claim, lease, heartbeat, complete, retry, dead-letter, cancellation, and recovery. Job payloads are versioned. At-least-once execution plus idempotent effects is the explicit model; do not claim exactly-once delivery.

### Outbound delivery

Own notification intent, provider idempotency keys, delivery attempts, rate limits, and queued/sending/sent/failed/suppressed/ambiguous states. The domain transition and outbound intent are committed in one database transaction.

### User-data lifecycle

Own account export, deletion, retention, and per-account serialization. Deletion must prevent concurrent receipt, notification, and schedule work from recreating data.

### Insights

Own deterministic aggregates and optional LLM narratives. The deterministic result remains available if the LLM is disabled or fails.

### Provider adapters

Use narrow, operation-specific interfaces for external effects:

- receive or poll provider events
- download provider media
- send provider messages
- extract structured receipt data
- read/write/delete receipt objects
- time and randomness

Do not create one broad provider trait and do not mock internal Rust modules.

## Clean PostgreSQL model

Start with a new migration series and a new database. Do not copy migrations 0001 through 0012 from Go.

The initial model should include these concepts:

- accounts: consent, locale, timezone, lifecycle state, and retention preference
- provider_identities: provider-scoped identity linked to an account
- account_ai_preferences: allowed model profile for extraction and insights
- inbound_events: provider event ID, normalized kind, payload version, receive time, processing state
- conversation_states: pending action with explicit expiry and optimistic version
- receipt_submissions: lifecycle state and account ownership
- receipt_assets: object key, hash, MIME type, size, retention deadline, deletion state
- extraction_attempts: provider/model/profile, prompt version, latency, token usage, result/error class
- expense_drafts: normalized candidate fields and confidence
- expenses: confirmed immutable financial facts plus explicit corrections
- jobs: versioned payload, state, priority, run time, dedupe key, serialization key, attempts, lease owner, lease deadline
- job_attempts: attempt timing and classified outcome
- outbound_messages: intent, idempotency key, state, provider message ID, ambiguity metadata
- summary_schedules: account-local schedule and next run
- insight_snapshots: deterministic aggregate, optional narrative, source period, model metadata
- deletion_requests: progress and audit-safe completion metadata without preserving deleted content

Schema rules:

- Use timestamptz for instants and an IANA timezone identifier for account-local calculations.
- Store money as signed 64-bit minor units plus ISO currency.
- Generate stable IDs in the application; never expose sequential database IDs as provider idempotency keys.
- Put unique constraints on provider event IDs, receipt hashes where appropriate, job dedupe keys, and outbound idempotency keys.
- Insert domain changes and their jobs/outbound intents transactionally.
- Use keyset pagination for unbounded operator and history queries.
- Add indexes from measured query plans, not hypothetical partitioning.
- Make deletion and retention executable in bounded batches.
- Store a schema version in every JSON job payload so old jobs can be upgraded or rejected safely.
- Future Rust releases use expand-and-contract migrations. No Go compatibility is required, but Rust-to-Rust upgrades must remain safe.

Before implementation, write an entity and invariant note in CONTEXT.md and record the clean-slate decision in an ADR.

## Ingress: webhook first, polling fallback

Webhook is the production default:

- POST /webhooks/zalo accepts only the configured provider verification mechanism and deployment secret.
- Apply strict body-size, content-type, timeout, and request-rate limits.
- Persist and deduplicate the inbound event before acknowledging it.
- Return quickly; downloading media, Gemini calls, and sending replies happen through durable jobs.
- Bind the application listener to a private interface by default and terminate public TLS in an operator-managed reverse proxy or the optional bundled Caddy profile.
- Expose GET /health/live and GET /health/ready separately.

Polling is an operational fallback:

- It is disabled by default and never installed as the default systemd command.
- A durable ingress-control record selects webhook or polling mode.
- Webhook instances reject processing when that record selects polling.
- A poller requires polling mode plus an exclusive renewable leader lease.
- Starting a poller while webhook mode is selected fails closed with a useful diagnostic.
- Polling normalizes events through the same ingress interface and therefore exercises the same conversation behavior.
- Switching modes is an explicit, audited CLI operation that coordinates provider webhook registration, advances a mode generation, and supplies rollback instructions.

## Gemini and insight profiles

Do not hard-code one Gemini model.

Configuration supports named profiles such as receipt-fast, receipt-accurate, and insight:

- provider and model identifier
- credential reference
- timeout and retry policy
- maximum input/output size
- reasoning or thinking effort when the selected model supports it
- structured-output schema version
- per-account and per-day request/token budgets
- task assignment: extraction, categorization, or insight narrative

Secrets are provisioned by the operator through a prompt or stdin and stored in root-readable credential files. They are not accepted as positional command arguments, chat messages, database plaintext, or logs. Multiple credential references may be configured; an account preference selects an allowed profile, not a raw key.

Model capability validation must reject unsupported thinking settings during configuration rather than discovering them on a receipt job.

Insights are implemented in two layers:

1. Deterministic SQL/domain insights: totals, trends, category changes, recurring merchants, budget drift, and unusual spending.
2. Optional LLM narrative: a concise explanation generated only from aggregate structured data unless the operator explicitly enables more data sharing.

The narrative is asynchronous, cached by aggregate/version/profile, schema-validated, quota-limited, and labeled as machine-generated. It has no tools and cannot mutate expenses.

## Privacy and security defaults

Self-hosting reduces infrastructure exposure but does not remove third-party processing: Zalo carries messages and Gemini receives receipt data when enabled.

- Default original-receipt retention is 7 days.
- Operators may configure 1 through 30 days; 30 days is not silently forced.
- A user may delete an original earlier without deleting the confirmed expense.
- Account deletion blocks new work, cancels pending jobs, deletes self-hosted object data, revokes or deletes provider-side artifacts where the provider supports it, then removes domain data.
- Logs and metrics contain no message bodies, receipt text, object URLs, tokens, secrets, or raw user identifiers.
- Diagnostic bundles are redacted and preview their file list before creation.
- Telemetry is off by default. Local Prometheus-format metrics are opt-in or loopback-only.
- Media downloads require HTTPS, provider-host validation, DNS/IP checks, size limits, timeouts, and redirect limits.
- Provider and LLM errors are classified without logging sensitive response bodies.
- Backups include documented encryption and retention recommendations.
- Ship privacy-policy and operator-terms templates describing storage, retention, deletion, backups, Zalo, Gemini, and administrator responsibility.

## Configuration and CLI

Separate non-secret configuration from credentials:

- /etc/zl-expense/config.toml: root-owned, atomically written, non-secret settings
- /etc/zl-expense/credentials/: root-readable credential files
- /var/lib/zl-expense/: state, local receipt objects, update metadata
- journald: application logs

Environment overrides remain available for containers and CI, but the CLI validates the resolved configuration and reports where each non-secret value came from.

Planned command groups:

- init: create directories, example configuration, credentials, and a deployment profile
- config get/set/unset/validate/show: non-secret settings with atomic locking
- secret set/unset/list: prompt or stdin only; values are never echoed
- db check/migrate/backup/restore: advisory migration lock and explicit confirmation for restore
- ingress status/webhook/poll: inspect or change ingress mode safely
- run --roles: all-in-one default or selected roles
- worker: advanced split-role execution
- doctor: passive checks only
- doctor --active zalo|gemini|object-store: explicit external calls with side-effect/quota warning
- status --json/--watch: health, readiness, process resources, queue age/depth, leases, dead jobs, outbound states, last successful ingress, and dependency state
- jobs list/show/retry/cancel: bounded operator recovery with audit logs
- logs --follow/--since: delegate to journalctl when systemd is present
- diagnose: create a redacted support bundle
- host install/uninstall: install or remove units and deployment files
- update check/apply/rollback: signed artifact flow with preflight, backup, migration check, restart, and health verification

Do not report dead outbound messages; dead is a job state. Outbound delivery reports failed and ambiguous separately.

## Observability and systemd

Use structured tracing with stable fields:

- request ID, normalized event ID, job ID, attempt ID, account pseudonym, operation, outcome, duration, and error class
- no unbounded provider payloads or user-generated strings
- log levels adjustable at runtime if safely supported, otherwise through config plus restart

Expose loopback-only Prometheus-format metrics:

- process resident memory, CPU time, file descriptors, and task count
- webhook count, rejection reason, acknowledgement latency, and duplicate count
- queue depth, oldest age, claims, retries, lease losses, dead jobs, and execution latency
- receipt state transitions, extraction outcomes, and retention deletion outcomes
- outbound state transitions and ambiguous sends
- provider request latency, timeouts, rate limits, and circuit state
- database pool usage and query latency
- LLM requests, input/output tokens, configured-profile usage, and estimated spend when pricing metadata is available

Never use account IDs, merchant names, command text, model responses, or job IDs as metric labels.

The default systemd unit should use:

- Type=notify when the runtime can signal readiness
- Restart=on-failure with bounded restart delay
- WatchdogSec with runtime heartbeat
- StateDirectory and RuntimeDirectory
- a dedicated unprivileged account
- NoNewPrivileges and appropriate filesystem/device/kernel hardening
- configurable MemoryMax, TasksMax, and CPUQuota presets
- TimeoutStopSec aligned with the application drain deadline
- stdout/stderr connected to journald

The CLI reads process and cgroup statistics for status --watch. It delegates historical logs to journalctl instead of building another log store.

## Resource budgets

Use an explicit initial small-VM benchmark profile:

- 1 vCPU, 1 GiB RAM
- PostgreSQL on the same host
- all-in-one application process
- database pool maximum 5
- receipt extraction concurrency 1 by default
- outbound concurrency 4 by default
- maximum inbound body and image sizes configured and tested

Initial release gates, subject to confirmation on the target VM:

- all-in-one idle resident memory at or below 100 MiB after warm-up
- idle application CPU below 1 percent averaged over ten minutes
- readiness within 3 seconds when dependencies are healthy and no migration is pending
- webhook acknowledgement p95 below 100 ms at 25 requests/second against local PostgreSQL
- process resident memory below 256 MiB during the standard webhook burst test, excluding PostgreSQL and reverse proxy
- no monotonic memory growth in a one-hour mixed receipt/command soak
- graceful stop finishes within 30 seconds or records the jobs left for lease recovery
- queue age returns to baseline after the standard burst

Do not silently relax a failed budget. Record the target, artifact, workload, result, and reason for any adjustment.

Performance strategy:

- Stream request bodies and object downloads where possible.
- Bound all queues and external-call concurrency.
- Downscale images under an explicit decoded-pixel limit to prevent decompression bombs.
- Keep the default process role combined to avoid duplicated runtimes and connection pools.
- Run a measured dependency spike before selecting the S3-compatible client; compare correctness, binary size, compile cost, idle RSS, and request memory.
- Keep microbenchmarks informational on ordinary shared CI.
- Gate release resource budgets on stable native amd64 and arm64 runners or representative VMs.
- Preserve benchmark results as release artifacts so regressions are visible.

## TDD contract

No implementation test is written until the public seams below are confirmed. Tests observe behavior through these seams, not private functions or internal call counts.

Proposed seams:

1. Conversation seam: account context plus normalized user input produces a reply plan or domain command.
2. Ingress seam: an HTTP webhook or polling event produces an acknowledgement and an observable accepted/duplicate/rejected event outcome.
3. Receipt seam: receipt submission and public receipt commands expose pending, extracted, confirmed, rejected, failed, expired, or deleted behavior.
4. Durable-work seam: enqueue, claim, heartbeat, complete, fail, cancel, and recover are tested through the queue interface against real PostgreSQL.
5. Outbound seam: a notification intent reaches an observable delivery state through a loopback Zalo server.
6. Provider HTTP seams: real adapter serialization, authentication, timeouts, status mapping, tolerant response parsing, and redaction are tested against a loopback mock server.
7. Operator seam: the compiled CLI exposes exit status, redacted output, configuration effects, and generated deployment files.
8. Runtime seam: the packaged process exposes health, readiness, metrics, graceful shutdown, bounded resource use, and systemd restart behavior.

Red-green execution rules:

- Work one vertical behavior slice at a time.
- Write one failing test through a confirmed seam.
- Implement only enough behavior to make it pass.
- Repeat with the next behavior learned from the previous cycle.
- Refactor only during an explicit review stage after the red-green cycles.
- Use independent literals and fixtures as expected results; do not recompute expectations with production logic.
- Mock only external HTTP, time, randomness, and optional object storage.
- Prefer real PostgreSQL for persistence and durable-work tests.
- Do not mock internal conversation, receipt, job, or outbound modules.
- Do not use a blanket line-coverage percentage as the quality target.

Test layers:

- Small unit/property tests: Vietnamese parsing, money, dates, timezone/DST, normalization, state transitions, retry classification, and deterministic insights.
- HTTP contract tests: Zalo webhook/media/message shapes, Gemini structured responses, S3-compatible behavior, malformed JSON, timeouts, 429, 5xx, redirects, and oversized payloads.
- PostgreSQL integration tests: constraints, transactional enqueue/outbox, leases, stale claims, per-account serialization, migrations, retention, and deletion.
- Vertical workflow tests: webhook to reply; image to draft; confirmation to expense; summary to outbound delivery.
- Failure-injection tests: crash at each durable transition, lease loss, database disconnect, provider timeout, ambiguous send, shutdown during work, and duplicate replay.
- Security tests: signature/secret validation, SSRF, redaction, body limits, secret handling, allowlist, and authorization.
- Performance tests: parser benchmarks, webhook enqueue load, queue throughput, image-memory ceiling, idle resources, mixed soak, and startup/shutdown.
- Package tests: clean install, systemd start, journald output, reboot survival, update, migration, failed-health rollback, and uninstall on supported hosts.

External HTTP is not low-risk merely because the local implementation is small. Contract tests prevent expensive duplicate sends, schema drift, retry storms, leaked credentials, and silent parsing failures while keeping mocks at the correct external seam.

## Vertical implementation plan

### Milestone 0: decisions and measurable contracts

- Create CONTEXT.md with the domain vocabulary and invariants.
- Record an ADR for clean-slate Rust, all-in-one default with split roles, PostgreSQL durable work, webhook-first ingress, and deployment packaging.
- Confirm the eight public test seams.
- Confirm target VM/traffic profile and resource gates.
- Inventory the Go user-visible capabilities and explicitly keep, improve, or drop each one.
- Define stable error classes and exit codes.
- Threat-model webhook, media download, credentials, LLM data sharing, update, and diagnostics.

Exit: no unresolved product decision can change the initial schema or public seam.

### Milestone 1: installable walking skeleton

First tracer bullet:

- Red: a clean package install with valid config and PostgreSQL reports ready; invalid config reports a stable redacted error.
- Green: minimal Rust binary, CLI, config/credential loading, tracing, one fresh migration, database connection, live/ready endpoints, and supervised run loop.

Also produce an unsigned development Debian package and portable bundle immediately. Packaging is exercised throughout the project rather than deferred to the end.

Exit: a clean supported VM can install, migrate, start under systemd, emit journald logs, report resource statistics, and uninstall.

### Milestone 2: text-command slice

- Red-green consent, allowlist, start/help, manual expense entry, today/history queries, pending-action expiry, and deterministic Vietnamese replies.
- Accept a verified webhook event, persist its idempotency key, run conversation behavior, and transactionally enqueue the reply.
- Deliver to a loopback Zalo server through the real HTTP adapter.
- Add polling normalization through the same ingress seam after webhook behavior is green.

Exit: duplicate webhook and polling events produce one logical command and one idempotent reply.

### Milestone 3: durable work depth

- Build leases, heartbeats, retries, dead-letter handling, cancellation, serialization keys, bounded concurrency, and operator recovery only as demanded by failing workflow tests.
- Inject crashes before and after each transaction/external effect.
- Prove stale workers cannot complete or overwrite a newer attempt.
- Prove an ambiguous send is not automatically resent.

Exit: worker restart, lease loss, and database interruption cannot lose accepted work or create an uncontrolled duplicate effect.

### Milestone 4: receipt-to-expense slice

- Accept an image event and validate/download it through a loopback provider.
- Store a bounded asset, calculate a content hash, enqueue extraction, and expose receipt state.
- Use a deterministic fake extractor first, then confirm/reject/edit the draft.
- Record the expense and expose today/history behavior.
- Enforce retention and early original deletion.

Exit: image to confirmed expense works end-to-end with real PostgreSQL and no real external network.

### Milestone 5: real extraction and object storage

- Select the S3-compatible implementation using the resource spike.
- Add local filesystem and MinIO/S3 adapters with the same receipt behavior.
- Add configurable Gemini profiles and capability validation.
- Test all HTTP wire contracts with loopback servers before one opt-in live smoke test.
- Track prompt/model/schema versions and token usage without logging receipt content.

Exit: operator-selected model profiles extract the same validated domain shape, and failures remain retryable or terminal by explicit class.

### Milestone 6: notifications, schedules, deletion, and insights

- Add quotas, summaries, scheduled work, exports, and the full user-data deletion lifecycle.
- Implement deterministic insight snapshots.
- Add optional aggregate-only LLM narratives behind a feature/config kill switch.
- Ensure account deletion wins against in-flight receipt, schedule, and outbound work.

Exit: schedules are timezone/DST correct; deletion leaves no user content in active tables or object storage; deterministic insights remain usable with LLM disabled.

### Milestone 7: operator depth

- Complete status, monitor, jobs, logs, diagnose, backup, restore, ingress-mode, and active doctor commands.
- Add Prometheus metrics and systemd readiness/watchdog integration.
- Add optional Caddy and MinIO deployment profiles to the versioned bundle.
- Document DNS/TLS, firewall, backup, restore, privacy policy, provider credentials, and incident recovery.

Exit: an operator can identify a failed dependency/job, inspect redacted evidence, recover it, and verify health without SQL access.

### Milestone 8: performance and security hardening

- Measure amd64 and arm64 release artifacts on the target profile.
- Run webhook load, mixed workload soak, crash matrix, media abuse, dependency outage, and disk-full tests.
- Tune pools, allocators only if measured, concurrency, timeouts, image handling, and release profile.
- Validate systemd hardening and resource limits.
- Run dependency/license/security audits and produce an SBOM.

Exit: all resource, resilience, privacy, and security gates pass with archived evidence.

### Milestone 9: signed stable release and update

- Publish amd64 and arm64 Debian packages plus portable release bundles.
- Include binary, systemd units, config example, pinned compose file, optional proxy/object-store profiles, migrations, documentation, checksums, SBOM, and signatures.
- Test each artifact natively on every supported host/architecture.
- Implement signed update metadata, preflight, database backup, migration, atomic binary replacement, restart, and health verification.
- Roll back the binary automatically only when the schema compatibility declaration permits it; otherwise stop and guide restore.

Exit: a new host can install without a repository/toolchain, survive reboot, receive a tested update, and recover from a failed health check.

## CI and release evidence

Every pull request:

- formatting and lint checks with warnings denied
- dependency policy and secret scanning
- changed vertical-slice tests
- PostgreSQL integration tests
- loopback HTTP contract tests
- debug build of the Debian/package layout

Main/nightly:

- all features and minimum-supported Rust checks
- property and failure-injection suites
- migration from every supported Rust schema version
- sanitizer/interpreter checks where supported
- short mixed-workload soak
- informational benchmarks with previous-result comparison

Release candidate:

- native amd64 and arm64 package installation
- supported Debian/Ubuntu matrix
- systemd/journald/reboot test
- PostgreSQL backup, migration, restore, and permitted rollback test
- webhook TLS deployment smoke test
- polling fallback and ingress ownership test
- target-host load and one-hour soak
- resource-budget report
- SBOM, checksums, signatures, and provenance

Live provider tests are opt-in, quota-capped, use dedicated test identities, and are never required for ordinary pull requests.

## First stable release proof

Functional:

- Consent, allowlist, manual expense, receipt extraction, draft edit/confirm/reject, today/history, summaries, export, and deletion pass through public seams.
- Webhook is the installed default; polling can be enabled explicitly and cannot compete with webhook ownership.
- Duplicate inbound events and receipt content are absorbed.
- Named Gemini profiles select model, credential reference, and supported thinking effort.
- Deterministic insights work without Gemini; optional narratives use aggregates and obey quotas.

Durability:

- Every accepted slow operation is recoverable from PostgreSQL.
- Domain changes and their jobs/outbound intents are atomic.
- Lease loss prevents stale completion.
- Crash-point tests prove retry behavior.
- Ambiguous sends require reconciliation rather than blind resend.
- Account deletion cannot race with remaining account work.

Operations:

- Clean install requires no source checkout or language toolchain.
- Default all-in-one and advanced split-role deployments both pass the same workflow suite.
- Status and monitor report CPU, memory, readiness, queues, oldest job, dead jobs, and outbound ambiguity.
- Logs are available through journalctl with correlation fields and no sensitive content.
- Diagnose produces a redacted support bundle.
- Backup/restore and one stable-version update are rehearsed.

Performance and security:

- The confirmed small-VM budgets pass on both target architectures.
- A one-hour soak shows no unbounded memory, task, connection, or queue growth.
- Webhook authentication, replay protection, rate/body limits, SSRF protection, secret redaction, and systemd hardening pass.
- Retention deletes originals at the configured deadline and defaults to 7 days.

## Confirmed test seams and remaining decisions

The user confirmed all eight proposed public test seams on 2026-08-15. TDD implementation may begin through those seams.

The following defaults remain open to amendment but do not block the first walking-skeleton tracer:

1. Initial target profile: 1 vCPU, 1 GiB RAM, 25 webhook requests/second burst, and low receipt concurrency.
2. Gemini credentials are operator-managed named profiles. Per-user raw key submission over chat is intentionally excluded.
3. Supported hosts are Debian 12 and Ubuntu 22.04/24.04 on amd64 and arm64.

Resource gates may be calibrated during the walking-skeleton benchmark, but changes must be recorded rather than silently weakened.
