# Flit

Flit is a local workspace for coding-agent CLI sessions with a quiet, evidence-backed attention inbox. Its planned terminal base remains usable across provider updates, while verified provider integrations add structured lifecycle, attention, and safe controls without inferring facts from terminal output.

The production application is an AppKit-first macOS app with selective SwiftUI and an in-process Rust Core connected through generated synchronous UniFFI bindings.

Flit is under active pre-release development.

## Current implementation

The current macOS application includes:

- guarded local Project registration and trust, exact-profile Codex managed Run start, durable session ownership, restart reconciliation, and explicit `Unknown` degradation when a provider capability cannot be proved;
- a Store-backed Dashboard and bounded Run Activity view with Core-owned lifecycle, activity, attention, evidence category, completion summary, and capability status;
- content-free Git baseline observation through a verified no-child-exec helper, exact or `Observed during run` terminal change attribution, bounded path-free Changes rows, and Core-guarded external file opening;
- deterministic Core-owned Possibly Stuck transitions using monotonic elapsed time and generation-bound process evidence, plus an exact Run-version/occurrence `Still working` action;
- exact highest-priority attention cards with safe unavailable states and failure-only acknowledgement that never implies the Run was resolved;
- a retained macOS monitoring cadence that assesses active Runs and atomically converges bounded Dashboard pages while the app remains open or in the menu bar; and
- policy-aware macOS notifications for permission, question, failure, completion, and Possibly Stuck, with global and Project settings, device-local quiet hours, durable claims, and exact delivered receipts.

Every supported conclusion is backed by structured evidence. When exact Git facts, provider history, raw provider content, or another capability is unavailable, the application preserves an explicit unavailable reason instead of inventing a result.

## Current boundaries

- The current build does not yet provide the planned Core-owned PTY and native terminal surface. Phase 5 confirmed that pinned public `libghostty-vt` is a consumable VT state engine but requires a Flit-owned renderer; choosing that scope or reopening another renderer is a product decision before adding a production dependency.
- Terminal bytes will not be parsed into structured lifecycle, permission, question, completion, or delivery facts. Once the terminal base lands, unknown provider versions will keep user-driven terminal interaction while only verified structured capabilities are disabled.
- Flit does not provide worktree orchestration, an editor, a browser, or a built-in diff.
- Raw provider content and secrets are not retained by default. The current native evidence surface exposes structured event locators and truthful raw-content availability.
- Exact file-change counts require the verified Git observation boundary; unsupported or uncertain repository states remain unavailable.
- Permission education and complete permission/question response controls are not yet available; unsupported actions remain visibly disabled until the required provider facts and documented delivery capability exist.
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
