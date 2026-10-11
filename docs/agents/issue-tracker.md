# Issue tracker: GitHub

Issues and specs live in GitHub Issues in `vanyastaff/surge`.
Use the `gh` CLI from this repository.

## Operations

- Create: `gh issue create --repo vanyastaff/surge --title "..." --body-file <file>`.
- Read: `gh issue view <number> --repo vanyastaff/surge --json number,title,body,labels,comments`.
- List: `gh issue list --repo vanyastaff/surge --state open --json number,title,body,labels`.
- Comment: `gh issue comment <number> --repo vanyastaff/surge --body-file <file>`.
- Label: `gh issue edit <number> --repo vanyastaff/surge --add-label "..."`.
- Remove label: `gh issue edit <number> --repo vanyastaff/surge --remove-label "..."`.
- Close: `gh issue close <number> --repo vanyastaff/surge`.

Write multiline bodies to a temporary file and pass `--body-file`.
Use the label mapping in `docs/agents/triage-labels.md`.

## Skill instructions

When a skill says "publish to the issue tracker", create a GitHub issue.
When a skill says "fetch the relevant ticket", read the GitHub issue,
including its labels and comments.

## Pull requests as a triage surface

**PRs as a request surface: no.**

GitHub shares issue and PR numbers. If a reference is ambiguous,
resolve its type before acting.

## Parent tasks and dependencies

Use native GitHub sub-issues and issue dependencies when available.
GitHub dependency operations require numeric database IDs, not issue numbers.

If sub-issues are unavailable, add `Part of #<parent>` to the child
and link it from a task list in the parent.
If dependencies are unavailable, add `Blocked by: #<number>` to the child.

## Wayfinding

A map is one issue labelled `wayfinder:map`, with Notes,
Decisions-so-far, and Fog sections.

Child tickets link to the map and use `wayfinder:<type>`,
where type is research, prototype, grilling, or task.
Create any required wayfinder labels when that workflow is invoked.

The frontier consists of open, unassigned child tickets with no open
blockers; select the first in map order.
Claim by assigning the issue to the driving developer.
Resolve by commenting with evidence, closing the ticket, and adding
a result pointer to the map's Decisions-so-far.
