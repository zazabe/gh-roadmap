# roadmap

Local-first product roadmap as markdown, synced to **GitHub Issues** and
**GitHub Projects** via a small Rust CLI.

- Plan in `roadmap/` (milestones + issues)
- Document current state in `projects/`
- Sync with `roadmap plan` / `roadmap apply` (uses `gh`)

## Install

### From GitHub Releases (recommended)

After a release is published:

```bash
VERSION=v0.1.0
curl -fsSL -o roadmap.tar.gz \
  "https://github.com/zazabe/gh-roadmap/releases/download/${VERSION}/roadmap-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
tar -xzf roadmap.tar.gz
install -m 755 roadmap /usr/local/bin/roadmap
```

### From source

```bash
cargo install --git https://github.com/zazabe/gh-roadmap --locked --force
# or, from a checkout:
cargo install --path crates/roadmap-sync --locked --force
```

### Requirements for sync

- [GitHub CLI](https://cli.github.com/) (`gh`) on `PATH`
- Auth: `gh auth login` with `repo` and `project` scopes, or `GH_TOKEN` /
  `GITHUB_TOKEN`
- Optional Notion milestone report: [Notion CLI](https://developers.notion.com/cli/get-started/overview)
  (`ntn`) on `PATH`, `ntn login` (or `NOTION_API_TOKEN`)

`roadmap validate` and `roadmap plan --offline` work without network/`gh`.

## Quick start (in your product repo)

1. Interactive setup (auth, pick an existing GitHub Project + repos, write
   `roadmap.toml`, scaffold `projects/`, optional agent skill):

   ```bash
   roadmap setup
   ```

   Flags: `--force` (overwrite), `--no-skill`, `--skill-dir <path>`.

2. Fill `projects/` with current-state docs.

3. Create milestones/issues (see `.cursor/skills/roadmap/SKILL.md`).

4. Sync:

   ```bash
   roadmap validate
   roadmap plan
   roadmap apply --yes
   ```

Manual alternative: copy `examples/basic/{roadmap,projects}` and edit
`roadmap/roadmap.toml`.

CLI flag if the tree is not `./roadmap`:

```bash
roadmap --roadmap-dir path/to/roadmap validate
```

## CLI

| Command | Purpose |
|---------|---------|
| `setup` | Interactive auth + scaffold + `roadmap.toml` (optional Notion) |
| `setup --notion-only` | Reconfigure Notion data source + field mapping |
| `validate` | Local checks only |
| `plan [--offline] [--platform LIST]` | Dry-run create/update actions |
| `apply [--yes] [--platform LIST]` | Sync to enabled platforms (GitHub + optional Notion) |
| `pull [--yes] [--overwrite] [--import-missing]` | Pull remote → local (GitHub only) |
| `show --issue N [--repo ID]` | Inspect local vs GitHub fields / sync drift |
| `doctor` | Auth and permission checks |

`--platform` is a comma-separated list (`github`, `notion`). Default: all
enabled platforms. Notion is push-only (milestones → data source pages).

```bash
cargo run --bin roadmap -- --roadmap-dir examples/basic/roadmap validate
```

## Repository layout

| Path | Purpose |
|------|---------|
| `crates/roadmap-sync/` | `roadmap` binary |
| `.cursor/skills/roadmap/` | Agent skill (copy into product repos) |
| `examples/basic/` | Starter `roadmap/` + `projects/` |
| `.github/workflows/` | CI and release builds |

This repository is the **tooling**. Keep real product roadmaps in your app
repos (do not commit private planning data here).

## Development

```bash
./scripts/check.sh          # fmt, clippy, test
cargo run --bin roadmap -- --help
```

Dev Container: open in VS Code / Cursor with the included `.devcontainer/`.

## Releasing

Push a version tag to build and attach binaries:

```bash
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions builds a Linux (`x86_64-unknown-linux-gnu`) artifact and creates a Release.

## License

[MIT](LICENSE)
