# Cursor worker harness

Codex is the engineering lead. These scripts give it a stable local bridge to Cursor CLI for bounded worker tasks; Cursor's internal subagent tool is not involved.

## Prerequisites

- `agent` or `cursor-agent` on `PATH`, authenticated (`agent status`)
- `jq`, Git, and Bash
- Cursor account access to the configured model IDs (`agent models`)
- At least one repository commit before using isolated write workers

The repository currently has an unborn `main` branch. Read-only and sequential workers work now, but Git cannot create a worktree until the first project commit exists. The harness reports that condition and never creates a bootstrap commit itself.

## Routing

| Role | Default model | Default access |
| --- | --- | --- |
| `investigate` | `cursor-grok-4.6-high-fast` | read-only |
| `debug` | `cursor-grok-4.6-high-fast` | write-enabled |
| `implement` | `composer-2.5` | write-enabled |
| `test` | `composer-2.5` | write-enabled |
| `trivial` | `auto` | write-enabled |

Every real run asks Cursor for the account's current model list and fails before execution if the selected ID is unavailable. Override routing explicitly when needed:

```bash
.agents/bin/worker --model composer-2.5-fast implement \
  "Implement the bounded change described in plan.md"
```

Set `CURSOR_AGENT_BIN` when the executable has a nonstandard name or path.
Keep credentials and secret values out of worker task text. The harness does not print environment dumps or command traces, and parallel run directories are owner-only, but tasks still pass through the local Cursor process.

## One worker

Sequential work uses the current working tree:

```bash
.agents/bin/worker investigate \
  "Trace how authentication tokens are refreshed"

.agents/bin/worker implement \
  "Implement refresh-token rotation according to plan.md"

.agents/bin/worker test \
  "Add coverage for refresh-token reuse detection"
```

`investigate` always uses Cursor plan mode. Any role can be forced read-only:

```bash
.agents/bin/worker --read-only debug "Find the root cause; do not fix it"
```

For an isolated writer, choose a short unique ownership name:

```bash
.agents/bin/worker --isolated auth-backend implement \
  "Implement only the backend token rotation changes"
```

That command creates branch `agent/auth-backend` at `HEAD`, checks it out at `.agents/worktrees/auth-backend`, and requires the worker to leave a clean worktree with at least one new commit. Use `--base <ref>` to select another reviewed base. It never merges or cherry-picks.

Use `--dry-run` to validate routing and arguments without listing remote models, invoking Cursor, or creating a worktree:

```bash
.agents/bin/worker --dry-run --isolated auth-backend implement "Disposable check"
```

## Parallel workers

Create a JSON Lines file outside the repository or under an ignored path. Each job needs a unique worktree-safe `name`, a `role`, and a bounded `task`:

```json
{"name":"auth-map","role":"investigate","task":"Map the current refresh-token flow"}
{"name":"auth-backend","role":"implement","task":"Implement backend rotation; own only backend files"}
{"name":"auth-tests","role":"test","task":"Add rotation tests; own only test files"}
```

Run up to three jobs at a time by default:

```bash
.agents/bin/worker-parallel --max 3 /tmp/auth-jobs.jsonl
```

Read-only jobs share the current repository. Every write job automatically gets `agent/<name>` and `.agents/worktrees/<name>`. Optional JSON fields are `model`, `read_only`, and `base`. Assign disjoint files and behavior to concurrent writers; isolation prevents filesystem races, not merge conflicts.

Results and stderr logs go to the ignored, owner-only `.agents/runs/<timestamp>-<pid>/` directory. The command prints a compact `summary.json` after all jobs finish; every result includes its JSONL `job_name`, and empty or malformed worker output becomes an explicit failed result.

## Handoff and integration

Workers return JSON containing status, summary, changed files, checks and results, assumptions, risks, orchestrator-attention items, and a commit SHA when isolated. The wrapper validates the Cursor result envelope, normalizes the handoff, rejects contradictory completion claims, and independently verifies an isolated worker's branch, base ancestry, changed-file list, and commit with Git.

Codex must still inspect every result. For an isolated worker:

```bash
git show --stat <commit>
git diff <base-commit>..<commit>
```

Only after review should Codex integrate with `git cherry-pick <commit>` (or another deliberate repository-appropriate method), resolve conflicts, and run repository-level checks. A worker's `completed` status is never correctness evidence by itself.

## Cleanup

List ownership before removing anything:

```bash
git worktree list
git branch --list 'agent/*'
```

After a worker commit is reviewed and integrated, remove its explicit worktree and then its branch:

```bash
git worktree remove .agents/worktrees/auth-backend
git branch -d agent/auth-backend
git worktree prune
```

Use `git worktree remove --force` or `git branch -D` only after inspecting and intentionally discarding uncommitted or unmerged work.

## Troubleshooting

- **CLI missing:** install Cursor CLI or set `CURSOR_AGENT_BIN`.
- **Authentication/model failure:** run `agent status` and `agent models`; use `--model` with an ID actually listed for the account.
- **Unborn branch:** create the project's intentional first commit before requesting isolation.
- **Existing path or branch:** inspect `.agents/worktrees/<name>` and `agent/<name>`; integrate or clean it rather than reusing ownership blindly.
- **Commit signing:** isolated workers honor repository Git configuration. Ensure non-interactive signing works, or explicitly choose and document a repository-appropriate unsigned-worker policy; the harness never disables signing itself.
- **Cursor failed:** read the worker's stderr (for parallel jobs, `<name>.stderr.log`). The wrapper does not print environment variables or command traces.
- **Malformed handoff:** inspect the compact excerpt in `needs_orchestrator_attention`, then retry with a tighter task if needed.
- **Interrupted run:** inspect both `git status` in the worktree and the worker branch before cleanup.

No `.codex/config.toml` is required: Codex discovers the root `AGENTS.md` directly. No `.cursor/agents/` personas are required because this design deliberately invokes independent Cursor CLI processes. An MCP wrapper can replace the shell boundary later without changing the role contract.
