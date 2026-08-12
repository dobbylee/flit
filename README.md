# Flit

Flit is a local attention inbox for provider-native coding-agent sessions. It turns structured execution evidence into quiet, inspectable status and raises only moments that need human action. It does not require a worktree-centric IDE or an embedded terminal.

The production application is an AppKit-first macOS app with selective SwiftUI and an in-process Rust Core connected through generated synchronous UniFFI bindings.

Flit is under active pre-release development.

## Current implementation

The current macOS application includes:

- guarded local Project registration and trust, exact-profile Codex managed Run start, durable session ownership, restart reconciliation, and explicit `Unknown` degradation when a provider capability cannot be proved;
- a Store-backed Dashboard and bounded Run Activity view with Core-owned lifecycle, activity, attention, evidence category, completion summary, and capability status;
- content-free Git baseline observation through a verified no-child-exec helper, exact or `Observed during run` terminal change attribution, bounded path-free Changes rows, and Core-guarded external file opening;
- deterministic Core-owned Possibly Stuck transitions using monotonic elapsed time and generation-bound process evidence, plus an exact Run-version/occurrence `Still working` action; and
- a retained macOS monitoring cadence that assesses active Runs and atomically converges bounded Dashboard pages while the app remains open or in the menu bar; and
- exact Possibly Stuck notification delivery through UserNotifications, with a durable Core receipt only after the same platform identifier appears in the delivered list.

Every supported conclusion is backed by structured evidence. When exact Git facts, provider history, raw provider content, or another capability is unavailable, the application preserves an explicit unavailable reason instead of inventing a result.

## Current boundaries

- Flit does not provide a generic PTY, embedded terminal, terminal replay, worktree orchestration, editor, browser, or built-in diff.
- Raw provider content and secrets are not retained by default. The current native evidence surface exposes structured event locators and truthful raw-content availability.
- Exact file-change counts require the verified Git observation boundary; unsupported or uncertain repository states remain unavailable.
- Notification settings, project overrides, quiet hours, permission education UI, and the complete permission/question action-queue presentation are not yet complete.
- The build script produces an ad-hoc signed development app, not a notarized distribution artifact.

## Build and validate

Building the native app requires macOS 14 or later, Xcode with Swift 6, the pinned Rust toolchain, and both macOS Rust targets:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
./scripts/build-macos.sh
```

The development app is written to `target/flit-macos/Flit.app`. Use the [execution workflow](agent-harness/workflow.md#4-validation) as the single source of truth for the complete schema, formatting, lint, workspace test, native, documentation, and diff gates.

## Contributing

Read [AGENTS.md](AGENTS.md) for repository invariants and [the execution workflow](agent-harness/workflow.md) for the current planning, review, and validation loop. The workflow is the single source of truth for executable validation commands.

The application source is under `apps/macos`. Protocol types and schemas originate in Rust; Swift bindings are generated from compiled `flit-bridge` metadata.

## License

Flit is available under the [MIT License](LICENSE).
