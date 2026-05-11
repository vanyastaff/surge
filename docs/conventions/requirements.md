# Requirements Artifact

`requirements.md` captures product requirements when a run needs a more formal
requirements surface than the bootstrap description.

Path: `requirements.md`  
Validator kind: `requirements`  
Schema version: none; human-readable Markdown contract.

## Minimal Valid Example

```markdown
# Requirements

## Overview
Operators need artifact validation before accepting generated plans.

## User Stories
- As an operator, I can see why a generated artifact was rejected.

## Functional Requirements
- The CLI validates canonical artifact paths.
- The hook stderr contains stable diagnostic codes.

## Success Criteria
- Invalid schema versions fail validation.
- Valid minimal artifacts pass validation.

## Out of Scope
- Live agent quality scoring.
```

## Checklist

- `## Overview` summarizes the need.
- `## User Stories` frames user value.
- `## Functional Requirements` lists concrete behavior.
- `## Success Criteria` lists measurable pass/fail checks.
- `## Out of Scope` limits the work.

## Profile Guidance

Use this artifact when the run needs product-level requirements before roadmap
planning. Keep implementation details for Spec artifacts.
