# Task plan: <short outcome>

## User outcome

Describe one user-observable result in one sentence.

## Confirmed facts

- Facts verified from the current code, data, or runtime
- Applicable requirements, design sections, and decisions when local records exist

## Assumptions

- Safe defaults and their evidence
- Which decision must reopen if an assumption is false

## Scope

Included:

- One vertical slice of behavior

Excluded:

- Adjacent phases, future expansion, and unrelated cleanup

## Change contract

| File or module | Change | Contract to preserve |
| --- | --- | --- |
| path | smallest change | invariant or API |

## Success criteria

- [ ] Observable normal-path result
- [ ] Error and boundary-path result
- [ ] Security and data invariants
- [ ] Public rules and local contract/traceability stay synchronized

## Validation

Focused:

```bash
<command>
```

Full:

```bash
<command>
```

Manual or environment-specific:

- Execution condition and evidence format

## Risk and rollback

- Data and migration impact
- Process, permission, and security impact
- Reversible boundary and artifacts to preserve

## Review scope

- Changed files
- Risks the reviewer should prioritize
- Intentionally excluded areas

## Result record

- Focused validation:
- Reviewer:
- Full validation:
- Unrun checks:
- Next safe slice:
