# Clean-slate Rust architecture

Status: accepted

The Zalo expense bot is a new Rust product, not a database- or process-compatible port of the legacy Go program. User-visible behavior from the Go deployment is discovery material; schema, migrations, process layout, packaging, and operator experience are redesigned.

We start a fresh PostgreSQL schema with a new migration series—no shared database, no copied Go migration ledger, and no requirement to read Go tables or queues. Ingress is webhook-first in production; polling is an explicit, audited fallback that normalizes through the same inbound interface and cannot run concurrently with webhook ownership. The default deployment is one supervised all-in-one process (ingress, worker, scheduler, maintenance) on a small VM; ingress, workers, and scheduler may later run as separate roles from the same binary, with all durable coordination still in PostgreSQL.

Background work is PostgreSQL-backed: transactional enqueue, leases, heartbeats, retries, dead-letter review, and bounded concurrency. Execution is explicitly at-least-once with idempotent effects; exactly-once delivery is not claimed. Outbound provider sends use durable intents with separate failed and ambiguous states; ambiguous outcomes forbid blind resend.

Delivery is package-first: signed amd64 and arm64 Debian packages and portable bundles install without a repository checkout or Rust/Go toolchain. systemd supervision, journald logging, and operator CLI commands are part of the product surface.

## Considered options

- **Incremental Go rewrite or shared database**: rejected because it would preserve incompatible schema, split-process assumptions, and migration debt while blocking webhook-first ingress and unified packaging goals.
- **In-memory or message-bus primary queue**: rejected because PostgreSQL already anchors the deployment and must own recovery, operator visibility, and split-role coordination.
- **Polling as the default ingress**: rejected because webhook delivery is the production default and polling increases duplicate and ownership complexity.

## Consequences

- Data import or coexistence with a live Go deployment is a separate future project, not an initial-release requirement.
- Rust-to-Rust upgrades must stay safe via expand-and-contract migrations; Go compatibility is not a migration constraint.
- Performance and operator tooling target the all-in-one profile first; split roles must pass the same behavioral suite before horizontal scale is encouraged.
