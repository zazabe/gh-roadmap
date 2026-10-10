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
# Linux x86_64 example — adjust tag, OS, and arch
VERSION=v0.1.0
curl -fsSL -o roadmap.tar.gz \
  "https://github.com/OWNER/roadmap/releases/download/${VERSION}/roadmap-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
tar -xzf roadmap.tar.gz
install -m 755 roadmap /usr/local/bin/roadmap
```

Replace `OWNER/roadmap` with this repository’s GitHub path.

### From source

```bash
cargo install --git https://github.com/OWNER/roadmap --locked --force
# or, from a checkout:
cargo install --path crates/roadmap-sync --locked --force
```

### Requirements for sync

- [GitHub CLI](https://cli.github.com/) (`gh`) on `PATH`
- Auth: `gh auth login` with `repo` and `project` scopes, or `GH_TOKEN` /
  `GITHUB_TOKEN`

`roadmap validate` and `roadmap plan --offline` work without network/`gh`.

## Quick start (in your product repo)

1. Copy the example tree:

   ```bash
   cp -R examples/basic/roadmap examples/basic/projects .
   # optional: copy agent skill
   mkdir -p .cursor/skills
   cp -R .cursor/skills/roadmap .cursor/skills/
   ```

2. Edit `roadmap/roadmap.toml` (`owner`, `project_number`, repo map).

3. Fill `projects/` with current-state docs.

4. Create milestones/issues (see `.cursor/skills/roadmap/SKILL.md`).

5. Sync:

   ```bash
   roadmap validate
   roadmap doctor
   roadmap plan
   roadmap apply --yes
   ```

CLI flag if the tree is not `./roadmap`:

```bash
roadmap --roadmap-dir path/to/roadmap validate
```

## CLI

| Command | Purpose |
|---------|---------|
| `validate` | Local checks only |
| `plan [--offline]` | Dry-run create/update actions |
| `apply [--yes]` | Sync to GitHub |
| `pull [--yes] [--overwrite] [--import-missing]` | Pull remote → local |
| `doctor` | Auth and permission checks |

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

GitHub Actions builds Linux, macOS, and Windows artifacts and creates a Release.

## License

[MIT](LICENSE)
