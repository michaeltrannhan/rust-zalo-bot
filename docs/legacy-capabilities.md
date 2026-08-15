# Legacy Go capabilities disposition

Evidence-backed keep / improve / drop decisions for every user-visible
capability in the legacy Go bot (`/Users/michaeltrannhan/Desktop/hobby/zl-expese-bot`).
Rust target is clean-slate per
`.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md` — no shared database,
no Go migration ledger, discovery-only tests.

**Closed product decisions (affect schema or seams):**

- Textract: **drop** from first stable release; optional future adapter behind
  extraction seam.
- Duplicates: **exact SHA-256 only** for automatic absorption; perceptual /
  soft matching reserved for warning-only future work.
- Exports: operator-generated artifacts via secure configured mechanisms only;
  never chat filesystem paths.

## Disposition table

| Capability | Disposition | Evidence | Rust notes |
| --- | --- | --- | --- |
| Consent onboarding (`/batdau`, consent card, versioned consent) | **Keep** | `internal/bot/bot.go` (`consentFlow`, `consentVersion`); `internal/bot/text.go` | Same user-visible flow through conversation seam |
| Pilot allowlist gate | **Keep** | `internal/config/config.go` (`PilotAllowlist`, `Allowlisted`); `internal/bot/bot.go` (redacted pre-auth payloads) | Config-driven; redact before consent |
| Vietnamese + slash command parsing | **Keep** | `internal/conversation/parse.go` (`IntentKind`, `slashCommands`, aliases); `internal/conversation/conversation_test.go` | Pure parser; deterministic replies |
| Slash command menu registered with Zalo | **Keep** | `internal/conversation/parse.go` (`SlashMenu`, `BotCommand`) | Provider adapter registers commands |
| Help (`/help`, `/huongdan`) | **Keep** | `internal/conversation/parse.go` (`IntentHelp`); `internal/bot/text.go` | |
| Privacy policy text (`/privacy`) | **Keep** | `internal/conversation/parse.go` (`IntentPrivacy`); `docs/privacy-data-map.md` | Update retention copy to 7-day default |
| Receipt image ingest + fast ack | **Keep** | `internal/bot/commands.go` (`handleImage`); queues `receipt_process` job | Webhook ack stays fast; work via jobs |
| Receipt pipeline: download → validate → store → extract → draft → card | **Keep** | `internal/receipt/processor.go` (package comment, `Handle`); `cmd/receipt-worker/main.go` | Deep receipt module; same observable states |
| Confirm / discard draft (`xác nhận`, `bỏ qua`) | **Keep** | `internal/conversation/parse.go` (`IntentConfirm`, `IntentDiscard`); `internal/bot/text.go` | Pending action seam |
| Edit draft fields (amount, merchant, date, category, type) | **Keep** | `internal/conversation/parse.go` (edit intents); `internal/bot/text.go` (`resolveEdit`) | Two-step pending with expiry |
| Manual expense entry (`150000 ăn trưa`, `cafe 45k`) | **Keep** | `internal/conversation/parse.go` (`IntentManualEntry`); `internal/bot/commands.go` (`manualEntry`) | |
| Period summaries: today / week / month | **Keep** | `internal/conversation/parse.go` (`IntentToday`…`IntentLastMonth`); `internal/bot/commands.go` (`summary`); `internal/insight/insight.go` | Deterministic SQL aggregates |
| Vietnamese period aliases (`/homnay`, `/thangtruoc`, …) | **Keep** | `internal/conversation/parse.go` (alias maps); `internal/conversation/conversation_test.go` | |
| Recent transactions (`/recent`) | **Keep** | `internal/conversation/parse.go` (`IntentRecent`); `internal/bot/commands.go` (`recent`) | |
| Settings: timezone and default currency | **Keep** | `internal/conversation/parse.go` (`IntentSettings`); `internal/bot/commands.go` (`settings`) | Recalculates schedule on TZ change |
| Opt-in scheduled summaries (`/sched`) | **Keep** | `internal/conversation/parse.go` (`IntentSummarySchedule`); `internal/bot/commands.go` (`summarySchedule`); `internal/summaryschedule/runner.go` | DST-aware `NextDelivery` |
| Recategorise latest transaction | **Keep** | `internal/conversation/parse.go` (`IntentRecategory`); `internal/bot/commands.go` (`recategorise`) | Learning from correction |
| Delete recent transaction (two-step) | **Keep** | `internal/conversation/parse.go` (`IntentDeleteRecent`, `deleteRecentPhrases`); `internal/bot/text.go` | |
| Full account deletion (two-step `/delete`) | **Keep** | `internal/conversation/parse.go` (`IntentDeleteData`); `internal/bot/commands.go` (`requestDelete`, `deleteAccount`); `internal/account/delete.go` | Per-account serialization; object + export purge |
| Merchant resolution (alias → canonical → fuzzy → create) | **Keep** | `internal/categorisation/categorisation.go` (`ResolveMerchant`, `MatchKind`) | Deterministic, no ML |
| Category suggestion (user rule → extraction → default) | **Keep** | `internal/categorisation/categorisation.go` (`SuggestCategory`, `UserRuleApplyConfidence`) | |
| Merchant/category count-based learning rules | **Keep** | `internal/categorisation/categorisation.go` (`ruleConfidence`); store layer | |
| Exact image duplicate guard (SHA-256) | **Keep** | `internal/receipt/processor.go` (`FindReceiptByHash`, `armExtraction`); `cmd/simulate/main.go` (step 7) | **Automatic absorption only via SHA-256** |
| Soft / possible duplicate warning (±3 days, amount/merchant) | **Improve** | `internal/receipt/processor.go` (`warnPossibleDuplicate`, `dupWindow`); `internal/conversation/templates.go` (`DupLine`) | Warning-only; not automatic absorption |
| Perceptual hash field on receipts | **Drop** (v1) | `internal/domain/types.go` (`PerceptualHash`) | Reserved for future warning path; not stored in v1 schema |
| Per-user daily receipt quota | **Keep** | `internal/bot/commands.go` (`handleImage`, `IncrementUsage`); `internal/config/config.go` (`PerUserDailyReceiptLimit`) | `quota_exceeded` class |
| Monthly OCR / extraction quota | **Keep** | `internal/receipt/processor.go` (`MonthlyOCRLimit`); `internal/config/config.go` | Global budget counters in schema |
| Extraction kill switch | **Keep** | `internal/receipt/processor.go` (`ExtractionEnabled`); `internal/config/config.go` | `kill_switch` class |
| Outbound kill switch | **Keep** | `internal/config/config.go` (`OutboundEnabled`); README architecture | |
| Outbound idempotent enqueue | **Keep** | `internal/notify/notify.go` (`Enqueuer.Reply`, idempotency keys) | |
| Ambiguous outbound send state | **Keep** | `internal/notify/notify.go` (package comment); README | No blind resend |
| Zalo outbound delivery (sole sender process) | **Improve** | `cmd/notification-worker/main.go`; `internal/notify/notify.go` (`Sender`) | Same behavior; may run in all-in-one process |
| Webhook ingress | **Keep** | `cmd/api/main.go` (`POST /webhook/zalo`, `maxWebhookBody`); README | Production default |
| Long-polling ingress (`-poll`) | **Improve** | `cmd/api/main.go` (`poll` flag); `internal/messaging/zalo/zalo.go` | Explicit fallback with leader lease + audited switch |
| Provider message dedupe | **Keep** | `internal/bot/bot.go` (`ClaimProviderMessageOpts`, duplicate absorption) | Unique provider event IDs |
| Mock deterministic extractor | **Keep** | `internal/extraction/mock/register.go`; README quick start | Default local/test backend |
| Gemini vision extraction | **Keep** | `internal/extraction/gemini/register.go`, `gemini.go`; README (`EXTRACTOR=gemini`) | Behind **named profiles**, not single `GEMINI_MODEL` env |
| Image downscale before extraction | **Keep** | `internal/extraction/gemini/downscale.go` | Add decoded-pixel bomb limits (plan) |
| AWS Textract extractor | **Drop** (v1) | `internal/extraction/textract/register.go`; README (`EXTRACTOR=textract`) | Optional future adapter; not v1 release |
| Local filesystem object store | **Keep** | `internal/platform/objectstore/`; README default stack | Default under `/var/lib/zl-expense/` |
| S3-compatible object store | **Keep** | `internal/platform/objectstore/s3.go`; `internal/config/config.go` | Optional MinIO/S3 profile |
| Receipt original retention sweep | **Improve** | `internal/receipt/sweeper.go`; `docs/privacy-data-map.md` (30d) | **7-day default**; operator 1–30d |
| Early user delete of original (expense survives) | **Keep** | Plan + Go sweeper keeps transactions; deletion flow in `internal/account/delete.go` | Explicit receipt command / lifecycle |
| Self-service export CSV + JSON | **Keep** (delivery) **Drop** (chat paths) | `internal/bot/commands.go` (`export`); `internal/account/export.go`; `internal/conversation/templates.go` (`ExportReadyText` paths in chat) | Generate artifacts; deliver via secure operator mechanisms only |
| Deterministic insights (SQL) | **Keep** | `internal/insight/insight.go`; `internal/store/insights.go` | Always available without LLM |
| Optional LLM insight narrative | **Improve** | Plan Insights section; Go has SQL-only insights | Aggregate-only narrative behind profile + kill switch |
| Budget command (`/budget`, `/ngansach`) | **Drop** | `internal/conversation/parse.go` (`IntentBudget`); `internal/conversation/templates.go` (`BudgetUnsupportedText`) | Not MVP in Go; remains unsupported |
| Browser playground (`make playground`) | **Drop** | `cmd/playground/main.go`; README Level 0 | Use Rust vertical tests + loopback adapters |
| Deterministic simulate ladder (`make simulate`) | **Improve** | `cmd/simulate/main.go` (10-step asserted loop) | Replaced by seam-based integration tests, not verbatim port |
| Real E2E script against live providers | **Improve** | `scripts/run-real-e2e.sh`; README Level 2+ | Opt-in, quota-capped smoke; not PR gate |
| Three-process architecture (api + 2 workers) | **Improve** | README architecture; `Makefile` `run-local`; three `deploy/systemd/*.service` units | All-in-one default; split roles advanced |
| Default systemd `-poll` ExecStart | **Drop** | `deploy/systemd/zl-expense-api.service` (`ExecStart` … `-poll`) | Webhook-first packaged unit |
| Environment-variable secrets (`GEMINI_API_KEY`, tokens) | **Drop** | `internal/config/config.go` | `/etc/zl-expense/credentials/` + CLI `secret set` |
| Single hard-coded Gemini model env | **Improve** | `internal/config/config.go` (`GeminiModel`) | Named profiles with capability validation |
| `GET /healthz` combined probe | **Improve** | `cmd/api/main.go` (`GET /healthz`) | Split `/health/live` and `/health/ready` |
| Postgres-backed SQS-shaped queue as end state | **Drop** | `internal/platform/queue/queue.go`; README | Native versioned `jobs` table with leases |
| Go migration ledger (`0001`–`0012`) | **Drop** | `db/migrations/`; plan Product decision | Fresh Rust migration series |
| `contracts/events` v1 as implementation authority | **Drop** | `contracts/events`; README | Discovery only; Rust job payload versioning separate |
| Log provider (local fake chat) | **Improve** | `internal/messaging/logprovider/register.go` | Loopback HTTP test adapter at provider seam |
| Zalo monthly message quota tracking | **Keep** | `internal/config/config.go` (`ZaloMonthlyMessageLimit`) | Outbound quota enforcement |
| Provider message tombstones + redaction | **Keep** | `internal/bot/bot.go` (`redactedPayload`); `internal/receipt/sweeper.go` (tombstones) | Pre-consent and allowlist reject paths |
| Insight evidence rows on manual summary | **Keep** | `internal/bot/commands.go` (`summary` → `insights.Persist`) | Optional persistence; answer not blocked |
| Income / refund / transfer / adjustment types | **Keep** | `internal/domain/types.go` (`TxType`); edit flows | |
| Seeded system categories | **Keep** | Store seed migrations; categorisation defaults | Seed in Rust P2 phase |
| VM install scripts (`install-vm-services.sh`) | **Improve** | `scripts/install-vm-services.sh`; `docs/vm-pilot.md` | `zl-expense host install` + signed packages |
| Unsigned `make build` local binaries | **Improve** | `Makefile`; README | Signed Debian/portable bundles (M9) |
| Docker Compose PostgreSQL for dev | **Keep** | `docker-compose.yml`; README | Bundle pinned compose in deployment profile |

## Summary counts

| Disposition | Count |
| --- | --- |
| Keep | 42 |
| Improve | 18 |
| Drop | 11 |

## Explicit drops (first stable release)

| Item | Rationale |
| --- | --- |
| Textract OCR | Reduce deps/binary size; Gemini + mock suffice; adapter seam reserved |
| Perceptual hash automatic dedupe | SHA-256 exact match only for absorption |
| Chat export path replies | Security; operator secure delivery only |
| `/budget` | Never implemented in Go MVP |
| Playground UI | Replaced by automated seam tests |
| Go DB compatibility | Clean-slate Rust product |
| Default polling systemd unit | Webhook-first production default |
| Plaintext env API keys | Credential file + CLI policy |
| SQS-queue abstraction as architecture | PostgreSQL jobs with leases |
| `contracts/events` v1 as source of truth | Discovery material only |

## Dependencies on other M0 artifacts

- Public seams, error classes, exit codes, schema inventory:
  `docs/product-contracts.md`
- Domain terms and invariants: `CONTEXT.md`
- Security controls: `docs/threat-model.md`
