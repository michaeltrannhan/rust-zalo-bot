# Rust port implementation progress

This checklist is the execution ledger for
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. A milestone is complete
only when its exit proof is recorded here and the repository-level checks pass.

Current paused-work handoff: `docs/engineering-handoff.md`.

## Milestones

- [x] M0 — Decisions and measurable contracts
  - [x] Domain vocabulary and invariants resolved
  - [x] Clean-slate architecture ADR accepted
  - [x] Eight public test seams recorded
  - [x] Target profile, resource gates, error classes, and exit codes recorded
  - [x] Legacy capabilities have keep/improve/drop dispositions
  - [x] Threat model covers webhook, media, credentials, LLM, updates, diagnostics
- [x] M1 — Installable walking skeleton
  - [x] Rust 1.94 workspace, CLI, typed config, PostgreSQL migrations, and exit taxonomy
  - [x] Supervised roles, liveness/readiness endpoints, and graceful SIGTERM shutdown
  - [x] Loopback-only development compose defaults and hardened systemd service
  - [x] Debian package and portable tarball preserve operator state on removal
  - [x] Real arm64 package installed, started, health-checked, stopped, removed, and purged on Debian 12 systemd
- [x] M2 — Text-command slice
  - [x] Consent, fail-closed allowlist, help, privacy, manual entry, today, and recent-history behavior
  - [x] Pending confirmation expires deterministically and resolves its stable draft reference
  - [x] Verified, bounded webhook ingress persists before acknowledgement and rejects the inactive mode
  - [x] Webhook and polling normalization share one transactional idempotency boundary
  - [x] Replies enqueue transactionally and deliver through the real loopback-tested Zalo adapter
  - [x] Sent, sending, and ambiguous outbound rows are never automatically resent
  - [x] Correct-mode retry promotes a previously mode-rejected event without duplicating it
- [x] M3 — Durable work depth
  - [x] PostgreSQL durable jobs, attempts, leases, heartbeats, retry/dead state,
        cancellation, per-account serialization, and the outbound-job bridge
  - [x] Hardening: database-clock lease deadlines, operator cancellation fenced
        by lease tokens, checked payload versions, redacted job summaries,
        outbound/job association checks, ambiguity metadata
  - [x] Supervised runtime worker executes leased jobs with bounded
        concurrency, heartbeats, and graceful SIGTERM drain without resend
  - [x] Concurrent-claim race root-caused (serialization-key unique violation
        surfaced as dependency error) and fixed with a post-advisory-lock
        recheck; 30/30 repeated concurrent rounds clean
- [x] M4 — Receipt-to-expense slice
  - [x] Bounded Zalo media intake (image parsing, SSRF-hardened download)
  - [x] Durable receipt lifecycle (ingest/extract/review/edit/confirm/reject/
        retention and early original deletion)
  - [x] Image webhook persists a receipt submission and `receipt.ingest` job
        in the same ingress transaction
  - [x] Runtime dispatches `receipt.ingest` / `receipt.extract` and enqueues
        one idempotent review card
  - [x] Review confirm/edit/reject text commands write through the open
        ingress transaction
  - [x] Vertical path proven: image webhook → download → extract →
        review/edit/confirm/reject → today/history, real PostgreSQL, no
        external network
- [x] M5 — Real extraction and object storage
  - [x] Filesystem object store (atomic put, path-traversal rejected)
  - [x] Path-style MinIO/S3 adapter with SigV4 behind the same seam
  - [x] Named Gemini profiles, capability validation, loopback generateContent
  - [x] 2048-pixel extraction downscale; attempt metadata is persisted
  - [x] Runtime selects store/extractor from config (`memory`/`fake` for tests)
- [x] M6 — Notifications, schedules, deletion, and insights
  - [x] Daily/monthly quotas and extraction/outbound kill switches
  - [x] IANA/DST-correct summary schedules; idle scheduler and retention roles
  - [x] Account deletion vs in-flight work (objects first; identities kept)
  - [x] Confirmed-only insight snapshots; optional aggregate-only narrative
- [x] M7 — Operator depth
  - [x] `status`, `jobs`, `doctor`, `ingress`, `backup`/`restore`, `logs`, `diagnose`
  - [x] Prometheus `/metrics` off by default; allowlisted labels only
  - [x] systemd `Type=notify`, watchdog, Caddy and MinIO deploy profiles
- [ ] M8 — Performance and security hardening
  - [x] Local: systemd `MemoryMax`/`TasksMax`, SBOM script, metrics privacy tests
  - [ ] Native amd64/arm64 resource measurements on the target profile
  - [ ] Webhook load, mixed soak, crash matrix, media abuse, disk-full on hosts
- [ ] M9 — Signed stable release and update
  - [x] Local: Ed25519 metadata signature, checksum, schema-gated rollback
  - [ ] Signed amd64/arm64 debs and tarballs produced on release hosts
  - [ ] Native install, reboot, update, and health-fail restore evidence

## Integration ledger

Record every isolated worker commit here after reviewing its full diff and
before or immediately after integration.

| Milestone | Worker | Commit | Reviewed | Integrated | Notes |
| --- | --- | --- | --- | --- | --- |
| M0 | `m0-domain-docs` | `33c9426` | yes, full base diff | `430a4e0` | Glossary/ADR; lead moved system rules to contracts |
| M0 | `m0-contract-docs` | `06c5710` | yes, full base diff | `d3225a7` | Seams, schema decisions, legacy dispositions |
| M0 | `m0-threat-doc` | `6ad163b` | yes, full base diff | `a1411fd` | Lead reconciled dedupe and error taxonomy |
| M1 | `m1-core` | `e2b4ef3` | yes, full base diff and untracked-output audit | `80adae9` | Harness reported failure only for generated `target/`; lead removed it before integration |
| M1 | `m1-packaging` | `8ac158a` | yes, full base diff | `8453904` | Debian, portable bundle, systemd, compose, and operator guide |
| M1 | lead integration | `8879b57` | yes, post-merge review | `8879b57` | Hardened config/errors/shutdown/migration tests and reconciled package docs |
| M1 | `m1-tests` | `7b800c6` | yes, full base diff | `40738ce` | Seven independent CLI/runtime acceptance checks; ordinary parallel runner also passed |
| M2 | `m2-conversation` | `576b5e3` | yes, full base diff | `8f1b14f` | Pure Vietnamese text-command seam; lead corrected consent/pending edge cases in `cc87d7e` and `d89f59a` |
| M2 | `m2-provider` | `7e822a5` | yes, full base diff | `67095d5` | Zalo wire adapter; lead closed redaction, token encoding, parsing, and input-bound gaps in `ac534d8` and `d89f59a` |
| M2 | `m2-ingress` | `642fca0` | yes, full base diff | `62c9cbd` | Transactional ingress persistence; lead corrected constraints, mode promotion, clocks, and draft lookup in `a62d964` and `d89f59a` |
| M2 | `m2-acceptance` | `6161716` | yes, full base diff | `24100c3` | Four independent public-seam acceptance tests; lead strengthened rejected-event recovery coverage |
| M2 | lead integration | `9a003f5`, `d89f59a` | yes, skeptical post-merge review | same | Wired HTTP/runtime/outbound vertical slice and fixed every confirmed M2-blocking review finding |
| M3 | lead hardening | `167be83` | yes, full staged diff | direct on `main` | Database-clock leases, operator cancel, payload-version bounds, redacted summaries, outbound association fencing |
| M3 | `m3-runtime-worker` | `f5fe1b4` | yes, full base diff | `3803922` | Leased-job worker: bounded concurrency, heartbeats, SIGTERM drain; 7 focused runtime tests re-run by lead |
| M3 | lead + Composer race fix | `6fb2b8d` | yes, full diff after lead root-cause | direct on `main` | Serialization-lease recheck under advisory xact lock; closes the claim race behind the `claim update failed` flake |
| M4 | `m4-media-provider` | `e5662d9` | yes, full base diff (prior lead review) | `9b4d871` | Bounded Zalo media intake; media gate re-run by lead before commit |
| M4 | `m4-receipt-core` | `1265c1c` | yes, Grok defect review + lead verification | `8aa83c7` | Receipt lifecycle; lead-directed corrections: retention sweep deletes objects before marking rows, edit/confirm lock order aligned |
| M4 | `m4-vertical-core` | `332c0e3` | yes, full base diff; lead fixed today-window test | `efb1536` | Migration 005, image accept, in-tx receipt APIs, receipt_review commands |
| M4 | `m4-vertical-wiring` | `6f1f63e` | yes, full base diff | `b0a8c64` | HTTP image dispatch, `dispatch_leased_job`, idempotent review follow-up |
| M4 | lead correction | `99ab0fd` | yes | `9f62a53` | Shared confirmation template; extract-job dedupe fence |
| M4 | `m4-vertical-tests` | `91d4d3f` | yes, full file review + independent suite | `b200d50` | Seven public-seam vertical tests; worker left the file uncommitted overnight, lead signed after 7/7 + full suite |
| M4 | lead docs | `8b057e5` | yes | `8b057e5` | Recorded M4 vertical-path completion |
| M5 | `m5-object-store` + lead | `eb21448` | yes, full diff + independent suite | `eb21448` | Filesystem/S3 stores, Gemini HTTP adapter, named profiles, 2048 downscale |
| M6 | Composer + lead | `eb21448` | yes; lead fixed midnight `/sched`, DST period math, insight narrative preserve | `eb21448` | Quotas, schedules, deletion/export, insight snapshots; schema 6–10 |
| M7 | Composer + lead | `eb21448` | yes; lead kept NOTIFY_SOCKET for watchdog, redacted `jobs show` | `eb21448` | Operator CLI, `/metrics`, notify/watchdog, Caddy/MinIO, runbook |
| M8 | lead | `eb21448` | local only | `eb21448` | SBOM, systemd resource limits, metrics label allowlist; native gates env-limited |
| M9 | lead | `eb21448` | local only | `eb21448` | `update preflight/apply/rollback`; native signed-package host evidence env-limited |

## Validation ledger

| Milestone | Command or artifact | Result | Notes |
| --- | --- | --- | --- |
| M0 | `git diff --check 66ce490..HEAD` | pass | Worker commits and lead reconciliation are whitespace-clean |
| M0 | Cross-document review | pass | Dedupe, errors, privacy defaults, and seam names agree |
| M1 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | 19 tests under the ordinary parallel runner; includes PostgreSQL, HTTP, exit, redaction, and SIGTERM cases |
| M1 | `cargo clippy --all-targets --all-features -- -D warnings` | pass | No warnings |
| M1 | `cargo fmt --check` | pass | Formatting clean |
| M1 | `./scripts/test-package.sh` | pass | Archive contents/modes, systemd syntax, Debian remove/purge semantics |
| M1 | Real Linux arm64 release build | pass | Rust 1.94 Debian build; binary and embedded migration installed from the generated package |
| M1 | Debian 12 systemd lifecycle | pass | Real package migrated, served live/ready, stopped in 552 ms, preserved state on remove, removed it on purge |
| M1 | arm64 artifact checksums | pass | deb `4a9d2809…bc2d3`; tarball `d727fb81…d3c5b` |
| M2 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | 83 tests with real PostgreSQL under the ordinary parallel runner; includes duplicate cross-ingress delivery, mode-rejected recovery, pending-window regression, and conservative outbound states |
| M2 | `cargo clippy --all-targets --all-features -- -D warnings` | pass | No warnings |
| M2 | `cargo fmt --all -- --check` and `git diff --check` | pass | Formatting and whitespace clean |
| M2 | Loopback Zalo HTTP contract and vertical webhook delivery | pass | Real adapter authenticates/parses/sends with no real external network and one idempotent reply |
| M3 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | 114 tests on combined hardening + runtime before M4 integration |
| M3 | 15× + 40× repeated `cargo test --test durable_work concurrent` | fail then fixed | Reproduced `claim update failed` (~1/15); PostgreSQL logs showed `idx_jobs_active_serialization_key` unique violation; after `6fb2b8d` 30/30 rounds clean |
| M3 | `cargo test --test m3_runtime_worker` | pass | 7 focused runtime tests re-run by lead in the runtime worktree |
| M4 | Media gate (`zalo_image_parse`, `zalo_media_download`, `zalo_http_contract`) | pass | Re-run by lead in the media worktree before signing |
| M4 | Receipt gate (lib, lifecycle+retention single-threaded, durable_work) | pass | Independently re-run by lead; re-run again after retention/lock-order corrections (24/24 incl. new rollback test) |
| M4 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | 178 tests after media/receipt cherry-picks; 186 after Task A; full suite green after Task B on `main` |
| M4 | `cargo test --test m4_ingress_image` | pass | 8 ingress image/review-command tests after lead today-window pin |
| M4 | `cargo test --test m4_runtime_jobs` | pass | 4 dispatch tests: ingest+extract, idempotent review card, SSRF/oversize → `failed_permanent`, unknown type |
| M4 | `cargo test --test m4_receipt_vertical` | pass | 7 public-seam tests: duplicate webhook, review card, confirm+today/recent, edit then confirm, reject, hash-duplicate absorb, 401/unsupported |
| M4 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` after `b200d50` | pass | Full suite including the 7 vertical tests; fmt/clippy clean in the isolated worktree |
| M5 | `cargo test --test object_store_fs --test object_store_s3` | pass | Filesystem durability/path-traversal; loopback path-style S3 including PUT 404 ≠ success |
| M5 | `cargo test --test gemini_http_contract --test unit_seams --test receipt_lifecycle` | pass | generateContent success/429/5xx/timeout/401/malformed/block/downscale; thinking_effort capability check; attempt metadata persistence |
| M5 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` | pass | Full suite green in the M5 worktree; fmt/clippy `-D warnings` clean |
| M5–M9 | `TEST_DATABASE_URL=… cargo test --all-targets --all-features` after `eb21448` | pass | Full suite including m6/m7/m8/m9; fmt/clippy `-D warnings` clean |
| M5–M9 | `./scripts/test-package.sh` | pass | Type=notify unit, Caddy/runbook in bundle, Debian 12 Docker remove/purge |
| M8 | `python3 scripts/generate-sbom.py` | pass | CycloneDX-lite from `cargo metadata` |
| M8 | `./scripts/security-audit.sh` | pass | Lockfile TLS check; `cargo-deny` not installed locally |
| M9 | `cargo test --test m9_update` | pass | Bad signature rejected; compatible rollback restores; incompatible rollback blocked |

## Accepted residual risks

- Two extractor workers may both perform extraction while a submission is
  `extracting`. Persistence is still idempotent; account-level job
  serialization bounds it. An `extracting_claimed_until` fence was deferred
  (no schema change in this slice). Cost is now a possible duplicate Gemini
  call, not just FakeExtractor work.
- Virtual-hosted-style S3 (`force_path_style = false`) is rejected at config
  load; only path-style MinIO/S3 is implemented.
- Live Google Gemini smoke is opt-in and is not a PR gate. Loopback
  generateContent contracts cover the wire classes.
- Default `extraction.backend = "fake"` keeps local/test runs off Gemini.
  Operators must set `backend = "gemini"` and a named profile to use the
  HTTP adapter.
- Insights LLM remains off by default (`[insights] llm_enabled = false`).
  Narrator input is the aggregate JSON only.
- `backup`/`restore` shell out to `pg_dump`/`pg_restore`; they are not
  exercised in CI when those tools are absent.
- `doctor --active zalo|gemini` can make live HTTP calls; Gemini refuses
  unless `api_base` is loopback.
- M8/M9 native Debian/Ubuntu amd64+arm64 resource, soak, signed-package,
  reboot, and host rollback evidence is still environment-limited.

## Environment-limited release gates

Native Debian/Ubuntu systemd, package upgrade/rollback, amd64 and arm64
resource measurements, and one-hour soak evidence require representative
hosts. Local substitutes may catch defects, but they do not count as the
release-gate proof defined by the product plan.
