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
- The app-process Rust Core is the sole event-ordering and SQLite writer. It also owns any Flit PTY child, byte ordering, and process lifecycle; Swift does not create domain transitions, another data writer, or process authority.
- Provider-native runtimes own persisted conversations and credentials. A Flit terminal session owns only its PTY child and never derives provider session identity or structured facts from terminal bytes.
- Provider behavior uses documented, version-probed capabilities. Uncertainty degrades structured features to `Unknown` while the distinct user-driven terminal remains available; never invent a structured fallback.
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
