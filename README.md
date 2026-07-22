# Flit

Flit is a local attention inbox for provider-native coding-agent sessions. It turns structured execution evidence into quiet, inspectable status and raises only the moments that need human attention, without requiring a worktree-centric IDE or an embedded terminal.

Phase 0 feasibility is complete and Phase 1 product implementation is underway. The native AppKit health shell is the sole tracked application path and verifies the Rust Core contract through generated UniFFI bindings before storage, provider monitoring, and user-facing agent workflows are implemented.

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
- `scripts/build-macos.sh`: universal AppKit application build with a statically linked Rust Core
- `scripts/test-macos.sh`: generated-binding, fixture, strict-concurrency, architecture, and linkage validation
- `scripts/validate-docs.sh`: public-rule validation, plus local design validation when `local/` exists

## Current technical direction

The accepted design uses an AppKit-first macOS shell, selective SwiftUI for low-cardinality leaves, and an in-process Rust Core linked through a synchronous, coarse-grained UniFFI bridge. Rust remains Flit's sole event-ordering and SQLite-writing authority; Swift owns presentation and native macOS lifecycle, accessibility, and delivery adapters without adding another data writer. Codex and Claude Code remain owned by their documented native session runtimes, while provider adapters reconcile supported sessions into an evidence-backed attention queue. V1 deliberately excludes a separate Flit daemon, XPC service, Flit-owned Generic PTY, embedded terminal renderer and input, worktree orchestration, editor, browser, built-in diff, and mobile companion.

The application source lives under `apps/macos`. AppKit owns the window, lifecycle, high-cardinality layout, and accessibility; SwiftUI remains limited to low-cardinality hosted leaves.

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
./scripts/test-macos.sh
./scripts/validate-docs.sh
```

The documentation validator works in a fresh public clone without `local/`. Maintainers with the private local planning tree receive additional checks for requirements, decisions, traceability, and internal links.

Rust is the source of truth for protocol types and the current event schema. Regenerate the checked-in JSON Schema after changing an event protocol type:

```bash
cargo run --locked -p flit-protocol --bin generate-schema
```

`cargo test -p flit-protocol` fails when the generated event schema, current fixtures, or current/previous-minor compatibility manifest drift from the Rust contract.

The Swift bridge binding is generated from the compiled `flit-bridge` metadata into ignored `target/` output. `./scripts/test-macos.sh` generates it twice, compares every generated file byte-for-byte, compiles Swift 6 with complete strict concurrency, and verifies the universal application has no dynamic Rust-library dependency.

The dependency-free documentation validator uses the Node version pinned in `.node-version`; Node is not part of the application build or runtime.

## License

Flit is available under the [MIT License](LICENSE).
