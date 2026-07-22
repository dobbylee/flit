# Flit Execution Harness

This document defines the repeatable loop for design, feasibility, implementation, and release work. `AGENTS.md` owns short repository-wide invariants. Detailed product contracts and working plans live in ignored `local/` when available.

## 1. Gate types

### Design gate

Use this gate when a requirement or contract changes.

- Update the affected product, design, decision, and traceability records under `local/`.
- Change the contract before its implementation.
- Validate examples and schema consistency.
- Run documentation validation and independent review.

### Feasibility gate

Use this gate for disposable experiments that reduce architecture uncertainty.

- Record the question and pass/fail criteria before writing the spike.
- Keep the spike separate from production crates and packages.
- Record the environment, exact commands, measurements, and outcome.
- Update the relevant decision and delivery gate.
- Do not copy spike code into production code.

### Implementation gate

Use this gate only after explicit implementation approval and the applicable feasibility gate.

- Deliver one user-observable result as the smallest vertical slice.
- Run focused validation, independent review, and full validation.
- Update the affected contract and traceability records in the same unit.

### Release gate

Use this gate for signing, packaging, deployment, or any external state change.

- Complete the release checklist under `local/` when it exists.
- Run independent security, data-side-effect, and rollback review.
- Obtain explicit user approval before publishing.

## 2. Start a task

### 2.1 Preflight

1. Inspect `git status --short --branch` and preserve user changes.
2. Read `README.md`, `AGENTS.md`, this workflow, and the directly relevant source and tests.
3. If `local/` exists, read its index, current plan, decision register, implementation plan, and relevant design documents.
4. Verify the current code and runtime path instead of inferring behavior from nearby state or prior summaries.
5. Confirm that the required gate and user authorization are satisfied.
6. Identify out-of-scope files and possible overlap with user work.

### 2.2 Plan contract

Copy `agent-harness/templates/task-plan.md` into an ignored working plan and fix:

- one user-observable outcome;
- confirmed facts and explicit assumptions;
- included and excluded scope;
- changed files and affected contracts;
- success and failure criteria;
- focused and full validation;
- security, data, migration, and rollback impact;
- documentation and traceability updates.

If the change cannot be explained in one sentence, split it again.

### 2.3 Blocking conditions

Do not guess through these conditions:

- an applicable decision is unresolved before the current gate;
- an agent adapter capability has not been verified against the real supported CLI;
- a permission response cannot verify request identity and delivery acknowledgement;
- a migration, deletion, or force-kill target is not exact;
- success cannot be expressed as an executable check or direct observation.

For non-blocking details, use the documented safe default and record the assumption.

## 3. Smallest vertical slice

Good slices include:

- one event flowing through domain, persistence, the in-process Core bridge, and UI or testkit;
- one failure reproduced, fixed, and closed with a regression test;
- one user action with both success and error states.

Bad slices include:

- abstractions added only for hypothetical future use;
- several delivery phases scaffolded together;
- unrelated renames, formatting, or refactors;
- interfaces and mocks without an observable behavior.

Keep schema migrations, adapter behavior, UI copy, and release configuration in separate commit units when practical. Keep a behavior and the tests that directly prove it together.

## 4. Implementation loop

1. Add the smallest failing test or direct observable criterion.
2. Implement the minimum change that satisfies it.
3. Re-read the changed files for scope and invariant drift.
4. Run focused validation.
5. Delegate to the project-scoped custom agent whose configured `name` is `reviewer`, with the plan, success criteria, changed files, and validation evidence.
6. Fix each finding with the smallest relevant change.
7. Re-run the same focused checks and review.
8. Continue until the reviewer returns exactly `No Findings` with no additional text.
9. Run impact-appropriate full validation and `git diff --check`.
10. Check public English-only rules and local contract/traceability drift.
11. Record results and any unrun checks, then commit one logical unit.

The reviewer must inspect the diff and source-of-truth contracts directly rather than trusting the implementation summary.

### Reviewer invocation boundary

<!-- flit-reviewer-contract:v1 custom=reviewer nested-codex=forbidden fallback=hash-verified -->

- Use the project custom agent defined by `.codex/agents/reviewer.toml`; Codex selects it by the file's `name = "reviewer"` field.
- Confirm that the spawned agent uses that custom role and its configured reasoning effort. A generic child whose task label or nickname is merely `reviewer` does not satisfy the gate.
- Prefer an effective `sandbox_mode = "read-only"` reviewer. If the current client instead reapplies the parent `workspace-write` permission to the selected custom reviewer, record that limitation and use the fallback only with explicit user acceptance: prohibit reviewer writes, freeze the reviewed scope, and compare aggregate tracked-diff and required-local-source hashes before and after the pass. Compare hashes programmatically and report only changed/unchanged; any change invalidates the review.
- Keep the reviewer independent from the implementing agent's conclusions.
- Do not launch a nested Codex client with `codex exec` or another shell command to satisfy this gate.
- Report the review gate as blocked if the custom role or configured reasoning cannot be verified, the user does not accept the sandbox fallback, or the integrity comparison changes. Deterministic validation and the implementing agent's self-review do not count as an independent review.

## 5. Choose focused validation

| Change | Minimum focused validation |
| --- | --- |
| Pure reducer or policy | Targeted unit tests and property invariants |
| Event or schema | Schema fixtures and generated binding drift |
| SQLite or migration | Temporary DB migration, replay, rollback, and integrity check |
| PTY or process | Fake-agent integration with timeout and cleanup |
| Adapter parser | All target fixtures with chunk and resize variations |
| Permission response | Stale, duplicate, and delivery-failure matrix |
| Native UI component | Component state matrix, keyboard, accessibility, and main-actor checks |
| Core bridge | Contract, ownership/error, reconnect, and bounded cursor tests |
| Public rules or local design | `./scripts/validate-docs.sh` |

Confirm that a test exercises the changed risk instead of relying on a convenient test name.

## 6. Independent reviewer

The project custom agent is defined and discovered from `.codex/agents/reviewer.toml`. `.codex/config.toml` contains only global subagent limits. Its output contract is `agent-harness/prompts/implementation-review.md`.

Always provide:

- task outcome and success criteria;
- exact changed files or base-to-head diff;
- applicable public rules and local contracts;
- validation commands and results;
- intentionally excluded scope.

Each finding must contain severity, file and line, risk, occurrence condition, smallest fix, and required validation. A clean review returns exactly `No Findings`.

## 7. Full validation

Current tracked migration-baseline validation follows. The native parity change must extend it with real native production scripts while retaining checks for still-tracked legacy code. Remove legacy commands only in the cleanup unit that removes the code they validate; never document a proposed command as current.

```bash
pnpm check
pnpm test
pnpm build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm tauri:build
./scripts/test-macos.sh
./scripts/validate-docs.sh
git diff --check
```

Never record a nonexistent script or an unrun manual check as passing. Record the exact reason, risk, and next execution condition for any skipped validation.

## 8. Promote failures into the harness

Do not turn every one-off issue into a permanent rule. Score a failure first.

| Factor | Score |
| --- | ---: |
| Impact: cosmetic / local failure / data or security risk | 0 / 1 / 3 |
| Recurrence: once / twice / three or more times | 0 / 1 / 2 |
| Detection: immediate / focused test / dogfood or release | 0 / 1 / 2 |
| Automation fit: low / partial / high | 0 / 1 / 3 |

Promotion target:

- 7 or more: regression test and, when appropriate, CI gate;
- 5–6: test first, otherwise review prompt or checklist;
- 3–4: task plan edge case or local design note;
- 0–2: fix the current change without a permanent rule.

Promote only short, cross-task invariants into `AGENTS.md`. Keep detailed procedures here, product contracts under `local/`, and mechanically verifiable behavior in tests or CI.

## 9. Completion report

Lead with the outcome and include:

- user-visible behavior or contract that changed;
- key changed files;
- validation commands and reviewer result;
- remaining decisions, unrun checks, and known limitations;
- the next safe phase or slice.

Do not call a spike a completed product feature, and do not call incomplete work complete.
