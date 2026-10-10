# projects/ — current state reference

`projects/` documents **what exists today**. `roadmap/` documents **planned and
in-flight work**. Keep them aligned when work ships.

## Mapping

| Roadmap `repo` | `projects/` path | GitHub repo |
|----------------|------------------|-------------|
| `app` | `projects/app/README.md` | `YOUR_ORG/app` |

Starter layout: `examples/basic/` in the roadmap CLI repo. In a product repo,
keep `projects/<name>/README.md` at the repo root. Update this table to match
your `roadmap.toml`.

## When to read `projects/`

Before creating or editing roadmap items:

1. Read `projects/README.md` for system context.
2. Read the target repo's README.
3. Use that context in the issue **## Context** section.
4. Avoid planning work that contradicts documented architecture unless the issue
   updates `projects/`.

## When to update `projects/`

When roadmap work **changes documented current state** (usually on close/done):

- new/removed workflows, services, integrations
- deploy/CD or environment changes
- data-flow / topology changes
- deprecations or replacements

Skip pure bugfixes/refactors with no architectural impact unless asked.

### Style

- short intro + **Repo** link
- factual, present-tense
- no task tracking in `projects/` (that stays in `roadmap/`)

## Project doc impact (issue body)

```markdown
## Project doc impact

- [ ] `projects/app/README.md` — document the new capability
```
