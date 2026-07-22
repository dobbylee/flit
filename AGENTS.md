# Flit Repository Rules

## Read first

Before changing the repository, read `README.md`, this file, and `agent-harness/workflow.md`. Read the task's directly relevant source and tests. If `local/` exists, also read its index, current plan, decision register, implementation plan, and relevant design documents.

## Repository boundary

- All committed documentation, rules, prompts, code comments, configuration descriptions, and user-facing copy must be written in English.
- Detailed product planning, architecture drafts, decision notes, delivery plans, and working checklists belong under ignored `local/`.
- Public files may define durable rules and reproducible harnesses, but must not depend on `local/` being present in a fresh clone.
- Do not commit files from `local/` or remove `local/` from `.gitignore`.

## Current phase

- The repository is in the pre-implementation design phase.
- Do not add product code or dependency scaffolding until the user explicitly approves implementation and the feasibility gate recorded in `local/` is complete.
- Feasibility spikes must remain disposable and separate from production modules.

## Product invariants

- Normal progress stays quiet; only moments that need human action are promoted.
- Every summarized or inferred state must link to raw evidence such as an event, provider-history locator, command, file change, or diagnostic.
- Keep lifecycle, current activity, and attention level as independent state dimensions.
- Tauri Rust Core is Flit's sole event-ordering and SQLite writer. Provider-native runtimes own Codex and Claude Code sessions; V1 does not own Generic CLI PTYs or embed a terminal.
- Provider adapters use documented, version-probed surfaces and record source, confidence, capability, and evidence. Uncertain behavior degrades to `Unknown` and exposes only verified provider-open or raw-evidence navigation capabilities.
- Permission and question responses are bound to request identity and version. Reject stale and duplicate responses.
- Never create a persistent permission rule for an action, path, or scope the user was not shown.
- Treat provider history, raw evidence, and logs as local sensitive data that may contain secrets.

## Execution rules

- Keep each change small enough to explain as one commit unit.
- Record assumptions, success criteria, changed files, focused validation, and full validation before implementation.
- Build the smallest vertical slice and avoid unrelated refactors.
- Run focused validation, then use the independent review gate defined in `agent-harness/workflow.md` for only the changed scope.
- Fix findings, re-run the same checks, and repeat until the reviewer returns exactly `No Findings`.
- Run full validation and `git diff --check` before reporting completion.
- Preserve user changes and do not edit out-of-scope files.

The detailed loop and harness-promotion rubric live in `agent-harness/workflow.md`.

## Documentation rules

- Keep requirement IDs as `FR-*`, non-functional requirements as `NFR-*`, decisions as `D-*`, and risks as `R-*` when local design documents exist.
- Put each rule in one source of truth and link to it instead of duplicating it.
- Do not leave unfinished markers. Record unresolved decisions with an owner, safe default, and resolution gate in `local/`.
- Examples must match the protocol or contract they document.
- Document commands as current only when the referenced scripts exist and are executable.

## Validation

```bash
./scripts/validate-docs.sh
```

After product scaffolding exists, add the actual Rust and frontend commands to the public workflow, CI, and package scripts in the same change.
