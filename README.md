# Flit

Flit is a local attention inbox for provider-native coding-agent sessions. It turns structured execution evidence into quiet, inspectable status and raises only moments that need human action. It does not require a worktree-centric IDE or an embedded terminal.

The production application is an AppKit-first macOS app with selective SwiftUI and an in-process Rust Core connected through generated synchronous UniFFI bindings.

## Contributing

Read [AGENTS.md](AGENTS.md) for repository invariants and [the execution workflow](agent-harness/workflow.md) for the current planning, review, and validation loop. The workflow is the single source of truth for executable validation commands.

The application source is under `apps/macos`. Protocol types and schemas originate in Rust; Swift bindings are generated from compiled `flit-bridge` metadata.

## License

Flit is available under the [MIT License](LICENSE).
