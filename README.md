# Flit

Flit is a local attention inbox for provider-native coding-agent sessions. It turns structured execution evidence into quiet, inspectable status and raises only the moments that need human attention, without requiring a worktree-centric IDE or an embedded terminal.

Phase 0 feasibility is complete and Phase 1 product implementation is explicitly approved. The first foundation slice is the next separately planned unit; no user-facing agent workflow is implemented yet.

## Open-source repository boundary

Everything committed to this repository is written in English.

The public repository contains:

- durable contributor and agent rules;
- reusable execution and review harnesses;
- source code, tests, and user-facing documentation once implementation starts.

Detailed product planning, architecture drafts, decision notes, delivery plans, and working checklists live under the ignored `local/` directory. They are intentionally not part of the open-source repository.

## Public harness

- `AGENTS.md`: durable repository rules and product invariants
- `agent-harness/workflow.md`: the slice, validation, and review loop
- `agent-harness/prompts/implementation-review.md`: independent review contract
- `agent-harness/templates/task-plan.md`: per-slice planning template
- `.codex/agents/reviewer.toml`: project-scoped read-only reviewer definition
- `.codex/config.toml`: subagent concurrency and nesting limits
- `scripts/validate-docs.sh`: public-rule validation, plus local design validation when `local/` exists

## Current technical direction

The working design uses a macOS-first Tauri 2 desktop shell, a React/TypeScript UI, and a Rust Core that is Flit's single event-ordering and SQLite-writing control plane. Codex and Claude Code remain owned by their documented native session runtimes, while provider adapters reconcile supported sessions into an evidence-backed attention queue. V1 deliberately excludes a Flit-owned Generic PTY, embedded terminal renderer and input, worktree orchestration, editor, browser, built-in diff, and mobile companion. The bounded read-only attention and event-store feasibility gates are complete, and implementation will proceed as independently reviewed vertical slices.

## Validation

```bash
./scripts/validate-docs.sh
```

The validator works in a fresh public clone without `local/`. Maintainers with the private local planning tree receive additional checks for requirements, decisions, traceability, and internal links.

## License

Flit is available under the [MIT License](LICENSE).
