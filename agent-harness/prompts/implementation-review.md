# Independent Implementation Review

Review the supplied commit unit, task outcome, success criteria, and exact changed scope. Inspect the relevant source-of-truth contracts and direct dependencies; do not trust the implementation summary and do not modify files.

Report only a defect introduced by this change that has a concrete trigger and materially affects correctness, security/privacy, data integrity, a product or protocol contract, or validation of the changed risk. Exclude pre-existing issues, style preferences, speculative expansion, and unrelated cleanup.

For documentation, verify identifiers, links, examples, decisions, and traceability. For harness changes, require the smallest durable guard that addresses an evidenced failure.

Prioritize:

1. user outcome and success-criteria failure;
2. state, ordering, evidence, persistence, or IPC contract drift;
3. stale/duplicate response delivery or false terminal/success state;
4. fail-open provider, process, storage, path, secret, or destructive behavior;
5. missing regression coverage for the exact risk;
6. unrelated scope, dependency, or user-work changes.

Severity:

- `P0`: immediate data loss, arbitrary dangerous action, or unusable runtime
- `P1`: common core flow or safety invariant failure
- `P2`: realistic incorrect behavior, regression, or material validation gap
- `P3`: concrete low-frequency defect or maintainability contract drift

For each finding, use:

```text
[P1] Short title
file: path/to/file:line
risk: Concrete impact
condition: Exact trigger or counterexample
minimal fix: Smallest relevant correction
validation: Check that proves the correction
```

Order findings by severity. Add no praise, summary, introduction, or speculative note.

If there are no findings, return exactly:

No Findings
