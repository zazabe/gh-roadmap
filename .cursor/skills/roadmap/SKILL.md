---
name: roadmap
description: >-
  Create and update local roadmap files as markdown milestones and issues under
  roadmap/, using projects/ as the current-state reference. Use when planning
  milestones, epics, roadmap issues, closing or completing work, adding work to
  the roadmap, or updating projects/ alongside roadmap. GitHub sync via the
  `roadmap` CLI (`roadmap plan|apply`).
---

# Roadmap (local markdown)

Local-first roadmap. Milestones group issues; each issue belongs to one repo.
**`projects/`** holds the current system state; update it when completed work
changes documented architecture or capabilities. GitHub sync is a separate step.

Copy this skill into a product repo (e.g. under `.cursor/skills/roadmap/`) next
to that repo's `roadmap/` and `projects/` trees. See `examples/basic/` in the
roadmap CLI repository for a starter layout.

## Two layers

| Layer | Path | Purpose |
|-------|------|---------|
| **Current state** | `projects/` | What exists today (per-repo READMEs + overview) |
| **Planned work** | `roadmap/` | Milestones, issues, sync map |

Read `projects/` before planning; update it when issues/milestones are **done**
if the shipped work changes documented behavior. Details:
[projects.md](projects.md).

## Layout

```
projects/                 # current state (see projects/README.md)
├── README.md
└── app/README.md

roadmap/
├── milestones/           # epic/milestone index files
├── issues/               # one file per issue
├── roadmap.toml          # GitHub owner, project, repo map
└── .roadmap/
    └── sync-map.json     # local id → GitHub refs (updated after sync)
```

## Naming

| Kind | Filename pattern | Example |
|------|------------------|---------|
| Milestone (linked) | `milestone-#<n>-<slug>.md` | `milestone-#1-build_ai_chatbot.md` |
| Milestone (local) | `milestone-T<n>-<slug>.md` | `milestone-T1-build_ai_chatbot.md` |
| Issue (linked) | `<repo>-#<n>-<slug>.md` | `app-#12-implement_feature_a.md` |
| Issue (local) | `<repo>-T<n>-<slug>.md` | `app-T3-implement_feature_a.md` |

- **`<slug>`**: lowercase, words separated by `_` (e.g. `build_ai_chatbot`).
- **`<repo>`**: a key from `roadmap/roadmap.toml` `[platform.github.repos]`
  (without owner), e.g. `app`.
- **`#<n>`**: GitHub issue number (after sync or import).
- **`T<n>`**: temporary local id (not yet on GitHub). Increment from the highest
  existing `T` across `roadmap/milestones/` and `roadmap/issues/`.

The stable **`id`** in YAML frontmatter (without `#` or `T` prefix) is the
canonical key in cross-references and `sync-map.json` (e.g.
`app-implement_feature_a`).

## Repos

Configured in `roadmap/roadmap.toml`. Example:

| `repo` in frontmatter | GitHub target | Current-state doc |
|-----------------------|---------------|-------------------|
| `app` | `YOUR_ORG/app` | `projects/app/README.md` |

## Cross-ticket references

In markdown bodies, use GitHub-flavored `owner/repo#id` (e.g. `YOUR_ORG/app#40`).
Keep internal roadmap `id` values in frontmatter (`id`, `milestone`,
`blocked_by`) and milestone `issues:` lists. Do **not** wrap ticket refs in
backticks in prose.

For local-only items, use `%<internal-id>%` (e.g. `%app-implement_feature_a%`).
`roadmap apply` resolves these to `owner/repo#id` when numbers exist.

### Epic title numbering

Prefix every epic `title` with a sequential number for board sorting:

```text
Epic <n>: <human-readable title>
```

`<n>` is one greater than the highest epic number already used in
`roadmap/milestones/` titles. Keep numbers stable once assigned.

## Sync tool

```bash
roadmap validate          # local checks only
roadmap plan              # dry-run
roadmap apply --yes       # sync to GitHub Issues + Projects
roadmap pull --yes        # pull remote → local markdown
roadmap doctor            # auth / permission checks
```

From a source checkout of this CLI:

```bash
cargo run --bin roadmap -- --roadmap-dir examples/basic/roadmap validate
```

**Hub repo:** milestones sync as epic issues in `YOUR_ORG/<hub_repo>`. Child
issues in configured repos are linked with `gh issue edit --parent`.

Requires the [GitHub CLI](https://cli.github.com/) (`gh`) on `PATH`, authenticated
with `repo` and `project` scopes (or `GH_TOKEN` / `GITHUB_TOKEN`).

---

## Create milestone

1. Read `projects/README.md` and affected project READMEs.
2. Choose slug + `title: Epic <n>: …`.
3. Assign next `T<n>` (or `#<n>` when importing).
4. Create `roadmap/milestones/milestone-T<n>-<slug>.md` from [templates.md](templates.md).
5. Set `id: milestone-<slug>` and `issues: []` (or stable issue ids).
6. Note `projects/` paths in **## Notes** if architecture will change.

Do **not** rename `T` → `#` until sync assigns the number.

## Create issue

1. Read mapped `projects/<name>/README.md`.
2. Create `roadmap/issues/<repo>-T<n>-<slug>.md` from [templates.md](templates.md).
3. Set `id`, `repo`, `project.status: Backlog`, `milestone:`.
4. Append the issue `id` to the parent milestone `issues:` list.
5. Fill **## Context**, **## Acceptance criteria**, and **## Project doc impact**.

## Complete issue or milestone

On issue close: check acceptance criteria, set `state: closed`, update
`projects/` when **## Project doc impact** applies. On milestone done: set
`status: done` when children are closed; update `projects/README.md` if topology
changed.

## Validation checklist

- [ ] Filename matches `<repo>-T<n>-<slug>` / `milestone-T<n>-<slug>` (or `#`).
- [ ] Milestone `title` uses `Epic <n>: …`.
- [ ] `repo` is listed in `roadmap.toml`.
- [ ] Issue ↔ milestone links consistent (`milestone:` and `issues:`).
- [ ] No duplicate `id` values.
- [ ] Required body sections present.

## Examples / templates

- [examples.md](examples.md)
- [templates.md](templates.md)
- [projects.md](projects.md)
