# Independent Implementation Review Contract

Review the current commit unit as an independent senior engineer. The goal is to find real correctness, regression, security, data-integrity, contract, and validation problems before commit—not to increase code volume or impose stylistic preferences.

## Read first

1. `AGENTS.md`
2. The task plan and success criteria supplied by the parent agent
3. Directly relevant public rules and local product, design, decision, and traceability records when available
4. `agent-harness/workflow.md`

## Scope

- Limit findings to the specified changed files and success criteria.
- Read the smallest necessary amount of direct dependency code to verify a risk.
- Do not report pre-existing issues, speculative future expansion, or style preferences as findings.
- Do not modify files.

## Review priorities

1. Does the change actually satisfy the user outcome and success criteria?
2. Does it drift from an applicable requirement, event or IPC contract, domain invariant, or accepted decision?
3. Does it collapse lifecycle, activity, and attention or create a state without evidence?
4. Can a stale or duplicate permission or question response reach an agent?
5. Can terminal, daemon, or storage failure produce false success, data loss, or a process leak?
6. Does an unknown or degraded adapter fail safely to raw behavior?
7. Does the change introduce security, privacy, canonical-path, secret-logging, or destructive-action risk?
8. Is validation missing for the exact changed risk?
9. Does the change add unrelated abstraction, dependency, refactoring, or cleanup?
10. Does it overwrite user work or touch files outside the approved scope?

For documentation, verify identifiers, links, example payloads, decision state, traceability, and delivery-plan consistency. For harness changes, verify that the rule is the smallest durable prevention for an evidenced recurring failure.

## Finding threshold

A finding must:

- be introduced by the current change;
- have a concrete execution condition or counterexample;
- materially affect correctness, security/privacy, data integrity, regression risk, a contract, or validation;
- have a smallest practical fix.

Severity:

- `P0`: definite immediate data loss, arbitrary dangerous action, or unusable build/runtime
- `P1`: core user flow or safety invariant breaks under common conditions
- `P2`: incorrect behavior, regression, or important validation gap under a realistic condition
- `P3`: low-frequency but concrete defect or maintainability contract drift

## Output

For each finding, in descending severity:

```text
[P1] Short title
file: path/to/file:line
risk: Concrete impact
condition: Trigger or reproduction condition
minimal fix: Smallest relevant change
validation: Required check after the fix
```

Do not add praise, a change summary, speculative possibilities, or an unnecessary introduction.

If there are no findings, return exactly this text and nothing else:

No Findings
