# Usage guide

## What `cpm` manages

`copilot-plugin-manager` is now the home of `cpm`, a Rust-first package manager for GitHub Copilot assets. It manages six asset kinds:

- plugins
- skills
- agents
- MCP servers
- hooks
- workflows

The Python package is a thin wrapper around the Rust CLI. The old bundled profile/theme catalog and submodule-driven sync model are no longer part of the project.

## Install and run locally

Inside a source checkout:

```bash
uv python install 3.12
uv sync --group dev --python 3.12
uv run cpm --help
```

Compatibility entrypoints are available too:

```bash
uv run copilot-plugin-manager --help
python -m cpm --help
cargo run -p cpm-cli -- --help
```

If you are invoking the checkout from another working directory, use `--project` so `uv` resolves this repository correctly:

```bash
uv run --project /path/to/copilot-plugin-manager cpm --help
uv run --project /path/to/copilot-plugin-manager python -m cpm --cargo --help
```

## Manifest and lockfile model

`cpm` uses two files:

- `cpm.toml`: the human-edited manifest that declares intent
- `cpm.lock`: the machine-generated lockfile that pins resolved content

The manifest declares which assets you want and at what scope. The lockfile records pinned revisions, file hashes, and installation metadata.

Use `cpm-reference.toml` in this repository when you need a commented example of the canonical file shape.

## Local and global scope

Most asset kinds support two scopes:

| Kind | Local install path | Global install path |
| --- | --- | --- |
| plugin | `.github/plugins/` | `~/.copilot/plugins/` |
| skill | `.github/skills/` | `~/.copilot/skills/` |
| agent | `.github/agents/` | `~/.copilot/agents/` |
| mcp | `.github/mcp/` | `~/.copilot/mcp/` |
| hook | `.github/hooks/` | `~/.copilot/hooks/` |
| workflow | `.github/workflows/` | local-only |

Workflows are always local because GitHub Actions only reads `.github/workflows/` inside the current repository.

For plugins, there are two behaviors:

- registry or repo-spec plugins delegated to `copilot plugin install` are always global in practice
- native plugin bundles added from GitHub tree/blob URLs or local paths still honor the selected scope

## Common command flows

Initialize a new manifest in the current directory:

```bash
uv run cpm init
uv run cpm init --name my-copilot-project
```

Add assets from GitHub or local paths:

```bash
uv run cpm add https://github.com/github/awesome-copilot/tree/main/plugins/partners --plugin
uv run cpm add https://github.com/anthropics/skills/tree/main/skills/pdf --skill
uv run cpm add ./hooks/guardrails --hook
uv run cpm add ./workflows/review.md --workflow
```

Refresh resolved state and materialized files:

```bash
uv run cpm lock
uv run cpm sync
uv run cpm sync --group research
```

Inspect or clean the current state:

```bash
uv run cpm overview
uv run cpm overview --json
uv run cpm doctor
uv run cpm reset --dry-run
uv run cpm reset --workflow --force
```

Authenticate for GitHub-backed resolution when you need better API limits or private repository access:

```bash
uv run cpm auth login
uv run cpm auth login --open
```

Public GitHub sources usually work without authentication. Use a token when you need private repository access or want to avoid GitHub API rate-limit surprises.

## Working with workflows and hooks

Hooks install as bundles containing `hooks.json` and executable scripts. `cpm` ensures bundled shell scripts are installed with executable permissions.

Workflows install as Markdown source files. For `github/awesome-copilot` workflows, compiled `*.lock.yml` files are treated as managed sidecars during overview/reset, but compilation still happens through the upstream tool:

```bash
gh extension install github/gh-aw
gh aw compile
```

## Development and verification commands

The repository's standard validation paths are:

```bash
uv run pytest tests/test_cli.py -q
uv run poe ci
uv run poe ci-full
```

`uv run poe ci-full` performs formatting checks, manifest validation, Python lint/type checks, Rust clippy, tests, and a release-mode Rust build.

## Bundled example content

This repo no longer vendors the large upstream catalogs that powered the previous Python implementation. The checked-in example assets are intentionally small and are mainly used for examples, tests, or smoke coverage:

- `.github/plugins/partners/`
- `.github/plugins/edge-ai-tasks/`
- `skills/alpha/SKILL.md`

See [CREDITS.md](CREDITS.md) for the attribution notes associated with those checked-in examples.
