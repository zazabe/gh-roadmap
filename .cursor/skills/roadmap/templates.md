# Roadmap file templates

## Milestone (local, not yet on GitHub)

**File:** `roadmap/milestones/milestone-T1-build_ai_chatbot.md`

```markdown
---
id: milestone-build_ai_chatbot
title: Epic 1: Build AI Chatbot
status: planned
target_date: 2026-Q3
issues:
  - app-implement_feature_a
---

## Description

Epic goal and success criteria for this milestone.

## Notes

Optional planning notes, dependencies, or links.
```

### Milestone frontmatter fields

| Field | Required | Values / notes |
|-------|----------|----------------|
| `id` | yes | `milestone-<slug>` |
| `title` | yes | `Epic <n>: <human-readable title>` |
| `status` | yes | `planned` \| `in_progress` \| `done` \| `cancelled` |
| `target_date` | no | Quarter (`2026-Q3`) or ISO date |
| `issues` | yes | Stable issue `id`s (no `T`/`#`; empty `[]` ok) |

---

## Issue (local, not yet on GitHub)

**File:** `roadmap/issues/app-T1-implement_feature_a.md`

```markdown
---
id: app-implement_feature_a
repo: app
title: Implement feature A
state: open
labels:
  - roadmap
milestone: milestone-build_ai_chatbot
project:
  status: Backlog
  priority: null
  start: null
  due: null
blocked_by: []
---

## Context

Why this work exists. Reference `projects/app/README.md`.

## Acceptance criteria

- [ ] Criterion one
- [ ] Criterion two

## Project doc impact

- [ ] `projects/app/README.md` — describe change
- [ ] none — no doc update needed
```

### Issue frontmatter fields

| Field | Required | Values / notes |
|-------|----------|----------------|
| `id` | yes | `<repo>-<slug>` |
| `repo` | yes | Key from `roadmap/roadmap.toml` |
| `title` | yes | Issue title |
| `state` | yes | `open` \| `closed` |
| `labels` | no | Label names |
| `milestone` | no | Parent milestone `id` |
| `project` | no | Project board fields |
| `project.status` | yes (new issues) | Default `Backlog` |
| `project.priority` | no | e.g. `P0`, `P1`, `P2` |
| `project.start` | no | `YYYY-MM-DD` |
| `project.due` | no | `YYYY-MM-DD` |
| `blocked_by` | no | Issue `id`s |

---

## Issue (linked)

**File:** `roadmap/issues/app-#42-implement_feature_a.md`

Same template; `#42` in the filename is the GitHub issue number.

## Milestone (linked)

**File:** `roadmap/milestones/milestone-#1-build_ai_chatbot.md`

Same as local milestone; `#1` is the hub epic issue number after sync.
