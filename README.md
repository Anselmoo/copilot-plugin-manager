# Copilot Plugin Manager (cpm)

[![CI](https://img.shields.io/github/actions/workflow/status/Anselmoo/copilot-plugin-manager/cicd.yml?branch=main&label=ci)](https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml)
[![PyPI version](https://img.shields.io/pypi/v/copilot-plugin-manager)](https://pypi.org/project/copilot-plugin-manager/)

**Copilot Plugin Manager** or just `cpm` is a package manager for GitHub Copilot assets. It helps you define, install, update, inspect, and reset plugins, skills, agents, MCPs, hooks, workflows, and instructions with a reproducible manifest and lockfile workflow.

> The package is published as `copilot-plugin-manager`, but the command you run is `cpm`.

## Why use cpm?

- keep your Copilot setup in `cpm.toml`
- lock resolved revisions in `cpm.lock`
- install assets into repository-local or global Copilot locations
- manage assets from marketplace specs, GitHub sources, and local paths
- inspect drift with `overview`, `show`, `tree`, `doctor`, and `status`

## Install

For one-off usage:

```bash
uvx --from copilot-plugin-manager cpm --help
```

or:

```bash
uvx copilot-plugin-manager --help
```

For a persistent install:

```bash
uv tool install copilot-plugin-manager
```

Or with `pip`:

```bash
pip install copilot-plugin-manager
```

## Quick start

If you installed `cpm`, use `cpm ...`. If you prefer not to install it globally, replace `cpm` with `uvx --from copilot-plugin-manager cpm` in the examples below.

Initialize a new project:

```bash
cpm init
```

Add a plugin and a skill:

```bash
cpm add spark@copilot-plugins --plugin
cpm add https://github.com/anthropics/skills/tree/main/skills/pdf --skill
```

Apply the lockfile to disk:

```bash
cpm sync
```

Inspect what is installed:

```bash
cpm list
cpm status
```

## Common examples

Install a plugin from a Copilot marketplace registry:

```bash
cpm add spark@copilot-plugins --plugin
```

Registry plugins delegated to `copilot plugin install` are effectively global installs. Native plugin bundles added from GitHub tree URLs or local paths still honor local vs global scope.

Install a plugin bundle from a GitHub tree URL:

```bash
cpm add https://github.com/github/awesome-copilot/tree/main/plugins/testing-automation --plugin
```

Install a skill from GitHub:

```bash
cpm add https://github.com/anthropics/skills/tree/main/skills/pdf --skill
```

Inspect one asset in detail:

```bash
cpm show testing-automation --plugin
```

See the consolidated view of manifest, lockfile, and installed state:

```bash
cpm overview
```

## Core commands

| Command | What it does |
| --- | --- |
| `cpm init` | Create a new `cpm.toml` and `cpm.lock` |
| `cpm add` | Add an asset to the manifest and resolve it |
| `cpm sync` | Install everything recorded in `cpm.lock` |
| `cpm update` | Update one or all managed assets |
| `cpm remove` | Remove a managed asset |
| `cpm lock` | Resolve without installing |
| `cpm reset` | Remove managed state and/or installed assets |
| `cpm overview` | Show the combined manifest, lockfile, and disk view |
| `cpm list` | List installed assets |
| `cpm show` | Show full details for a single asset |
| `cpm tree` | Show the dependency tree |
| `cpm doctor` | Verify installed files match the lockfile |
| `cpm status` | Show drift between manifest, lockfile, and disk |

## How cpm works

- `cpm.toml` records the assets you want
- `cpm.lock` records the resolved versions and hashes
- `cpm sync` materializes those assets into `.github/` for local scope or `~/.copilot/` for global scope
- concrete GitHub file and tree sources are fetched directly by `cpm`
- delegated Copilot plugin installs are used where the Copilot CLI is the installer of record

## Documentation

- [`docs/USAGE.md`](docs/USAGE.md) for usage details
- [`CONTRIBUTING.md`](CONTRIBUTING.md) for development and contributor workflows
- [`ARCHITECTURE.md`](ARCHITECTURE.md) for the current system design

## License

See [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
