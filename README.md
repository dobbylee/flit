# Flit

Flit is a local Agent Development Environment for running multiple coding agents without continuously watching their terminals. It turns raw execution evidence into quiet, inspectable status and raises only the moments that need human attention.

The repository is currently in the **pre-implementation design phase**. Product code will begin only after the feasibility gates are complete and implementation is explicitly approved.

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

The working design uses a macOS-first Tauri 2 desktop shell, a React/TypeScript UI, and a Rust Core that is Flit's single control plane and SQLite writer. Codex and Claude Code remain owned by their documented native session runtimes; provider adapters reconcile those sessions into Flit, while Generic CLI runs use a Flit-owned PTY. The terminal surface defaults provisionally to xterm.js, with a Phase 0 comparison against ghostty-web. These choices remain subject to the documented feasibility spikes before product implementation.

## Validation

```bash
./scripts/validate-docs.sh
```

The validator works in a fresh public clone without `local/`. Maintainers with the private local planning tree receive additional checks for requirements, decisions, traceability, and internal links.

## License

Flit is available under the [MIT License](LICENSE).
