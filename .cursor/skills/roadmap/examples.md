# Roadmap examples

## Example 1: New epic with one new issue

**Goal:** Add milestone "Build AI Chatbot" with one `app` issue (all local).

### Step 1 — Milestone

Create `roadmap/milestones/milestone-T1-build_ai_chatbot.md` with `issues: []`
and `title: Epic 1: Build AI Chatbot`.

### Step 2 — Issues

Create `roadmap/issues/app-T2-implement_feature_a.md` with `repo: app`,
`milestone: milestone-build_ai_chatbot`, `project.status: Backlog`.

### Step 3 — Link milestone

```yaml
issues:
  - app-implement_feature_a
```

---

## Example 2: Add issue to existing milestone

Milestone `milestone-platform_foundation` already has:

```yaml
issues:
  - app-api_v2
```

Create `roadmap/issues/app-T4-auth_flow.md`, then:

```yaml
issues:
  - app-api_v2
  - app-auth_flow
```

---

## Example 3: Import existing GitHub issue

GitHub issue `YOUR_ORG/app#192` exists.

Create `roadmap/issues/app-#192-implement_feature_a.md` and add
`app-implement_feature_a` to the milestone `issues:` list.

```json
{
  "app-implement_feature_a": {
    "repo": "YOUR_ORG/app",
    "issue_number": 192,
    "project_item_id": null,
    "updated_at": "2026-07-13T00:00:00Z"
  }
}
```

---

## Example 4: Rename after sync

Before: `app-T2-implement_feature_a.md`  
GitHub assigns **#194** → rename to `app-#194-implement_feature_a.md`.  
Stable `id` / milestone `issues:` entries stay `app-implement_feature_a`.

---

## Example 5: Close issue and update projects/

Close `app-T5-chatbot_streaming` (`state: closed`), update
`projects/app/README.md`, and mark the parent milestone `done` if it has no
open children left.
