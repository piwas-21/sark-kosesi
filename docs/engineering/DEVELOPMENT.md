# Development and review

AGENTS.md is binding. Work on owned `codex/*` branches from develop. Use npm’s committed lockfile and `npm ci --ignore-scripts`; any necessary dependency build script must be narrowly reviewed and explicitly invoked. Do not use an unpinned `npx` download during CI.

## Change design

Define the user/operator trigger, expected state and failure/retry behavior before implementation. Name an authoritative source for price, stock, identity, locale and payment state. Add or update an API schema and migration when the contract changes. Prefer a vertical slice that can be tested and deployed independently.

Components should express product decisions, not internal implementation. Separate pure domain calculation from persistence and provider I/O. Use expand/contract migrations and compatible workers during rollout. Never do destructive schema changes in the same release that first stops reading the old field.

## Independent review

An independent reviewer reads the full diff against its target base. A review agent is permitted for this specific review task; it must not edit the implementation or approve its own work. Supply AGENTS.md and the actual staged/committed diff. Review for concrete guideline violations, security defects, state/race errors, missing recovery, accessibility and useful tests. Report high-confidence actionable issues with file/line, severity and fix. Resolve findings and re-review material changes.

Save the reviewer’s report under `.local/reviews/` and record the reviewed head/base with `python3 scripts/record_review.py --reviewer NAME --report PATH`. The script records a hash of the report. This receipt proves what was recorded locally; it is not a cryptographic attestation of reviewer independence. Do not fabricate it. Current GitHub approval count is zero, as in Sofra’s solo-owner workflow; required CI and resolved conversations are enforced by the server, while independent review remains an explicit workflow requirement.

The pre-push hook checks the reviewed head and local checks. The merge gate also verifies remote CI, the exact current head/base, open review conversations and active server protection. It refuses empty or skipped required results. Run:

```sh
python3 scripts/merge_gate.py 123
python3 scripts/merge_gate.py 123 --merge
```

Do not use a bare `gh pr merge`. If develop advances, rebase/update the owned branch through review and rerun the checks. No auto-merge workaround is part of this repository’s initial policy. After promotion, merge a reviewed `main → develop` synchronization PR with a merge commit and no branch deletion. This carries the release merge commit into develop so the next release can satisfy current-base ancestry.

## Exceptions

An exception must name its precise scope, rationale, compensating control, owner and expiry. Keep it in the relevant specification/PR; do not hide it behind a broad ignore. It cannot waive payment integrity, customer ownership, secrets, missing required tests or known exploitable critical/high findings. Budget changes require measured evidence and an ADR or PR explanation, not a larger limit chosen solely to make CI green.

## Repository visibility

Public visibility was explicitly requested for setup. It does not make the project open source; the repository remains unlicensed for reuse. Run `python3 scripts/audit_github.py` after any visibility/plan/rules change. On GitHub Free, converting back to private removes protected-branch enforcement and may make native security features unavailable. The production release gate must fail until server protections and required security scans are available again. Keep the Gitleaks CLI and dependency audit independent of GitHub’s extra security features.

## Evidence

A source test, a green remote check, an image build, a served revision, a provider outcome and physical delivery are different observations. Use the relevant one. Evidence must identify source SHA, environment, dataset/tool versions and the actual acceptance result. Never claim successful deployment from a workflow’s title alone. No secrets, room photos or customer PII in public artifacts, screenshots, fixtures or PR logs.
