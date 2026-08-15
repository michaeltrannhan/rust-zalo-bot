# Engineering orchestration

Codex owns requirements, architecture, decomposition, integration, correctness, and the final review. Cursor CLI workers may handle bounded investigation, debugging, implementation, tests, and trivial mechanical changes.

Before the first Cursor delegation, parallel worker run, or worker-worktree cleanup in a task, read `.agents/README.md`.

## Delegation

For substantial work:

1. Establish acceptance criteria and inspect enough repository context to divide the work safely.
2. Delegate independent, bounded tasks with `.agents/bin/worker` when a Cursor worker is a good fit.
3. Prefer parallel read-only investigation. Use `.agents/bin/worker-parallel` for parallel writers only when file ownership is disjoint; it isolates each writer in its own branch and worktree.
4. Wait for required handoffs, then inspect the actual diff or commit. Worker summaries are leads, not proof.
5. Integrate deliberately, run repository-level validation, and perform a skeptical final review against the original acceptance criteria.

Route `investigate` and difficult diagnosis to Grok, routine `implement` and `test` work to Composer, and only genuinely mechanical low-risk work to `trivial`/Auto. Codex may override a model when the task warrants it.

Codex should implement directly when the change is tiny, delegation costs more than the work, edits are tightly coupled, a worker repeatedly fails, integration requires edits, or correctness/security sensitivity favors direct ownership.

## Completion gate

Before reporting substantial work complete, independently examine correctness, regressions, error handling, relevant concurrency and security implications, architectural consistency, test coverage, and unrelated changes. Fix substantive findings or issue a tightly scoped correction task and review again.

## Repository context

This is a clean-slate Rust implementation. The current product plan is `.cursor/plans/rust_expense_bot_port_0d6549cd.plan.md`. Discover build, lint, and test commands from the repository at task time; do not cache commands here while the project is still being scaffolded.
