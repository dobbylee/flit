# Flit Delivery Workflow

This file is the single source of truth for planning, review, validation, and commit execution. Repository and product invariants live in `AGENTS.md`; reviewer finding format lives in the review prompt.

## 1. Select the gate

- **Design:** change the relevant requirement, decision, traceability row, and contract before implementation.
- **Feasibility:** freeze the question and hard stop, use disposable code, record only decision-grade evidence, and never copy spike code into production.
- **Implementation:** deliver one observable vertical behavior after its decisions and feasibility gates are satisfied.
- **Release:** complete the release checklist and obtain explicit approval before publishing or changing external state.

## 2. Preflight and task contract

1. Inspect `git status --short --branch`; preserve user and out-of-scope changes.
2. Read the files required by `AGENTS.md` and verify the actual code/runtime path.
3. Confirm authorization and that no applicable decision is unresolved.
4. Create one ignored current-task record from `agent-harness/templates/task-plan.md`.

The task record fixes the outcome, evidence-backed assumptions, included and excluded scope, changed contracts, success criteria, focused/full/manual validation, risks, rollback, and review scope. Split the task if its outcome needs more than one sentence.

Do not guess through an unresolved decision, unverified provider capability, unprovable response acknowledgement, inexact destructive target, or success condition that cannot be observed. Use a documented safe default for non-blocking details.

Delete the task record after the commit; Git history, tests, and normative contracts own completed work.

## 3. Commit-unit loop

1. Add the smallest failing test or direct observable criterion.
2. Implement only the behavior required by the task contract.
3. Re-read the changed scope and run focused validation.
4. Invoke the project custom `reviewer` with the task, exact scope, contracts, and validation evidence.
5. Fix every finding, rerun the same checks, and repeat review until the response is exactly `No Findings`.
6. Run full validation and `git diff --check`.
7. Confirm generated artifacts, public English, local contract/traceability, and staged scope.
8. Record unrun environment-specific checks, then commit one logical unit.

Schema migrations, provider side effects, UI behavior, and release configuration should remain separate units unless one observable behavior requires them together.

### Reviewer invocation boundary

<!-- flit-reviewer-contract:v1 custom=reviewer nested-codex=forbidden fallback=hash-verified -->

- Use the custom role defined by `.codex/agents/reviewer.toml`; a generically named child is not the configured reviewer.
- The reviewer inspects the diff and source contracts independently and follows `agent-harness/prompts/implementation-review.md`.
- Prefer effective read-only isolation. If the client reapplies workspace-write, use the hash-verified no-write fallback only with explicit user acceptance: freeze every reviewed tracked and required local file, prohibit writes, and compare hashes before and after. Any change invalidates the review.
- Do not launch a nested Codex client with `codex exec` or another shell command to satisfy this gate.
- Treat an unverifiable custom role, reasoning configuration, isolation fallback, or changed frozen scope as a blocked review gate.

## 4. Validation

Choose focused checks that execute the changed risk:

| Change | Minimum focused evidence |
| --- | --- |
| Reducer or policy | Targeted tests and invariants |
| Event or generated contract | Fixtures, schema, and binding drift |
| SQLite or migration | Temporary DB migration, replay, rollback, and integrity |
| Provider/process | Fake integration, bounds, timeout, and cleanup |
| Permission response | Stale, duplicate, and delivery-failure matrix |
| Native UI | State matrix, keyboard, accessibility, and main-actor checks |
| Core bridge | Contract, ownership/error, reconnect, and bounded cursor tests |
| Rules or design | `./scripts/validate-docs.sh` |

Full native validation:

```bash
cargo run --locked -p flit-protocol --bin generate-schema
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
./scripts/test-macos.sh
./scripts/validate-docs.sh
git diff --check
```

Never claim a command, manual check, or external gate that was not run. Record the reason, risk, and next execution condition for omissions.

## 5. Keep the harness small

Promote a failure only when a durable guard is cheaper than recurrence:

| Factor | 0 | 1 | 2 | 3 |
| --- | --- | --- | --- | --- |
| Impact | cosmetic | local failure | — | data/security |
| Recurrence | once | twice | three or more | — |
| Detection | immediate | focused test | dogfood/release | — |
| Automation fit | low | partial | — | high |

- 7+: regression test or CI gate
- 5–6: test, otherwise reviewer/checklist
- 3–4: current task or local contract
- 0–2: fix without a durable rule

`AGENTS.md` receives only short cross-task invariants. Remove a rule when code, types, or automated validation fully enforce it.

## 6. Completion

Report the changed outcome, key files, validation and reviewer result, unrun gates, and next safe slice. A phase is complete only when its own acceptance evidence is current; a spike is never product completion.
