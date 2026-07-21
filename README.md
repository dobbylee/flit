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
- `.codex/config.toml`: project reviewer role registration
- `scripts/validate-docs.sh`: public-rule validation, plus local design validation when `local/` exists

## Current technical direction

The working design uses a macOS-first Tauri desktop shell, React/TypeScript UI, and an independently-lived Rust daemon that owns agent processes, PTYs, event ordering, and local SQLite persistence. These choices remain subject to the documented feasibility spikes before product implementation.

## Validation

```bash
./scripts/validate-docs.sh
```

The validator works in a fresh public clone without `local/`. Maintainers with the private local planning tree receive additional checks for requirements, decisions, traceability, and internal links.

## License

Flit is available under the [MIT License](LICENSE).
