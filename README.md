# copilot-plugin-manager

[![CI](https://img.shields.io/github/actions/workflow/status/Anselmoo/copilot-plugin-manager/cicd.yml?branch=main&label=ci)](https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml)
[![PyPI publish](https://img.shields.io/badge/publish-PyPI-blue)](https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml)
[![PyPI version](https://img.shields.io/pypi/v/copilot-plugin-manager)](https://pypi.org/project/copilot-plugin-manager/)
[![Python versions](https://img.shields.io/pypi/pyversions/copilot-plugin-manager)](https://pypi.org/project/copilot-plugin-manager/)
[![License](https://img.shields.io/github/license/Anselmoo/copilot-plugin-manager)](LICENSE)

A Python-first CLI for managing GitHub Copilot plugins, skills, and agents.

It keeps Copilot setup management focused and reproducible:

- compose setups from profiles and themes
- install, update, and prune plugins
- sync local skills and agents into `~/.copilot`
- track repository-aware state under `~/.copilot/copilot-plugin-manager`
- refresh bundled catalogs from curated upstream sources

## Install

Install the CLI into your current Python environment:

```bash
pip install copilot-plugin-manager
```

Or run it without installing anything permanently:

```bash
uvx copilot-plugin-manager --help
```

Once installed with `pip`, the command is available directly:

```bash
copilot-plugin-manager --help
```

## Quick start

List what is available:

```bash
copilot-plugin-manager list profiles
copilot-plugin-manager list themes
copilot-plugin-manager list sources
```

Activate a setup for the current repository:

```bash
copilot-plugin-manager switch python-core
copilot-plugin-manager switch-exclusive python-mcp
```

Refresh upstream sources and inspect state:

```bash
copilot-plugin-manager repo-update --remote
copilot-plugin-manager status
```

If you prefer one-off execution with `uvx`, the same commands work there too:

```bash
uvx copilot-plugin-manager list profiles
uvx copilot-plugin-manager status
```

`uvx` is convenient for direct execution, but persistent shell completion is easiest when the CLI is installed locally.

## Catalog overview

<!-- generated:catalog-overview:start -->
_This section is generated from the bundled catalog data with `uv run poe generate-docs`._

- `21` profiles
- `20` themes
- `53` plugins
- `37` skill providers
- `58` agent providers

See also:
- [`docs/THEMES.md`](docs/THEMES.md) for the full theme and profile composition reference.
- [`docs/CREDITS.md`](docs/CREDITS.md) for upstream catalog credits.

### Current profile compositions

| Profile | Themes |
| --- | --- |
| `minimal` | `core` |
| `docs` | `core`, `docs`, `python`, `testing` |
| `docs-lite` | `core`, `docs` |
| `docs-pro` | `core`, `docs`, `docs-design`, `planning`, `testing` |
| `python-dev` | `core`, `python`, `testing`, `devops` |
| `python-core` | `core`, `python`, `testing` |
| `python-agents` | `core`, `python`, `testing`, `python-agents` |
| `python-mcp` | `core`, `python`, `mcp`, `testing`, `python-agents`, `mcp-agents` |
| `mcp-dev` | `core`, `mcp`, `python`, `testing` |
| `frontend-design` | `core`, `frontend`, `docs-design`, `testing` |
| `backend-api` | `core`, `python`, `openapi`, `data`, `testing`, `security` |
| `fullstack` | `core`, `frontend`, `python`, `testing`, `data` |
| `agentic-fullstack` | `core`, `frontend`, `python`, `testing`, `data`, `security`, `python-agents`, `mcp-agents` |
| `science` | `core`, `science`, `python`, `data` |
| `data-ai` | `core`, `data`, `science`, `research` |
| `research` | `core`, `docs`, `science`, `research`, `planning` |
| `devops-sec` | `core`, `devops`, `security`, `github` |
| `planner` | `core`, `planning`, `github` |
| `enterprise` | `core`, `enterprise`, `data`, `testing`, `devops` |
| `enterprise-architect` | `core`, `enterprise`, `planning`, `security`, `github`, `testing` |
| `everything` | `core`, `docs`, `docs-design`, `python`, `python-agents`, `frontend`, `mcp`, `mcp-agents`, `testing`, `science`, `openapi`, `enterprise`, `data`, `devops`, `security`, `planning`, `github`, `research`, `specialized`, `agents` |
<!-- generated:catalog-overview:end -->

## Shell completion

Quick shell-init snippets:

```bash
copilot-plugin-manager shell-init bash
copilot-plugin-manager shell-init zsh
copilot-plugin-manager shell-init fish
copilot-plugin-manager shell-init powershell
copilot-plugin-manager shell-init nushell
```

Managed completion files:

```bash
copilot-plugin-manager completion-install fish
copilot-plugin-manager completion-install bash
copilot-plugin-manager completion-script powershell
```

## From source

If you want to run the project from a local checkout instead of installing it from PyPI:

```bash
git submodule update --init --recursive
uv sync --group dev
uv run copilot-plugin-manager --help
```

## Documentation

| Document | Purpose |
| --- | --- |
| [`docs/USAGE.md`](docs/USAGE.md) | Managed content, state model, command reference, and shell setup. |
| [`docs/THEMES.md`](docs/THEMES.md) | Generated overview of current themes and profile compositions. |
| [`docs/CREDITS.md`](docs/CREDITS.md) | Generated credits for bundled upstream catalog sources. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Local development setup, test/lint/build commands, and PR workflow. |
| [`docs/RELEASING.md`](docs/RELEASING.md) | Build, TestPyPI, and PyPI publishing flow. |
| [`SECURITY.md`](SECURITY.md) | Vulnerability reporting and supported versions. |
| [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) | Community expectations and reporting guidance. |
| [`LICENSE`](LICENSE) | Project license terms. |

## Project links

- PyPI package: <https://pypi.org/project/copilot-plugin-manager/>
- CI/CD workflow: <https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml>
- Catalog refresh workflow: <https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/catalog-refresh.yml>
- Security policy: <https://github.com/Anselmoo/copilot-plugin-manager/security/policy>
- Issue tracker: <https://github.com/Anselmoo/copilot-plugin-manager/issues>
