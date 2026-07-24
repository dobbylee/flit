# Flit Repository Rules

## Before editing

Read `README.md`, this file, `agent-harness/workflow.md`, and the directly relevant source and tests. When `local/` exists, follow its routing index: read the current plan and decision index, then only the contract documents needed by the current slice.

Inspect the current worktree and preserve user changes. Implement only the current smallest vertical slice; do not scaffold adjacent phases.

## Repository boundary

- Committed documentation, rules, prompts, comments, configuration descriptions, and user-facing copy are English.
- Detailed working plans and unpublished product records stay under ignored `local/`. Never commit it or remove it from `.gitignore`.
- Public source and documentation must remain complete in a fresh clone without `local/`.
- Feasibility code is disposable and separate from production modules.

## Product and safety invariants

- Normal progress stays quiet; promote only moments that need human action.
- Every summary or inference links to raw evidence or an explicit unavailable reason.
- Lifecycle, current activity, and attention are independent state dimensions.
- The app-process Rust Core is the sole event-ordering and SQLite writer. Swift does not create domain transitions or another data writer.
- Provider-native runtimes own sessions and credentials. V1 has no Flit-owned Generic PTY or embedded terminal.
- Provider behavior uses documented, version-probed capabilities. Uncertainty degrades to `Unknown`; never invent a fallback.
- Permission and question responses require the exact current request identity and version. Reject stale and duplicate responses.
- Never persist a permission rule for an action, path, or scope the user was not shown.
- Provider history, raw evidence, and logs are local sensitive data; do not retain secrets or raw provider content by default.

## Change boundaries

- Keep one explainable commit unit and avoid unrelated refactors.
- Preserve replacement parity before removing an approved obsolete runtime, then remove all obsolete production paths in a separate unit.
- Put each rule or contract in one source of truth and link to it elsewhere.
- Record unresolved decisions with an owner, safe default, and resolution gate.
- Preserve out-of-scope files and never report an unrun check as passing.

Follow `agent-harness/workflow.md` for task contracts, review, validation, commits, and releases.
