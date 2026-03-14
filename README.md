# copilot-plugin-manager

[![CI](https://img.shields.io/github/actions/workflow/status/Anselmoo/copilot-plugin-manager/cicd.yml?branch=main&label=ci)](https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml)
[![PyPI publish](https://img.shields.io/badge/publish-PyPI-blue)](https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml)
[![PyPI version](https://img.shields.io/pypi/v/copilot-plugin-manager)](https://pypi.org/project/copilot-plugin-manager/)
[![Python versions](https://img.shields.io/pypi/pyversions/copilot-plugin-manager)](https://pypi.org/project/copilot-plugin-manager/)
[![License](https://img.shields.io/github/license/Anselmoo/copilot-plugin-manager)](LICENSE)

A Python-first GitHub Copilot plugin manager built with `Typer`, `Rich`, and `Pydantic`.

It keeps Copilot setup management focused and reproducible:

- compose setups from profiles and themes
- install, update, and prune plugins
- sync local skills and agents into `~/.copilot`
- track repository-aware state under `~/.copilot/copilot-plugin-manager`
- refresh bundled catalogs from curated upstream sources

## Install from source

```bash
git submodule update --init --recursive
uv sync --group dev
uv run copilot-plugin-manager --help
```

The default invocation also works:

```bash
uv run copilot-plugin-manager
```

## Quick start

List what is available:

```bash
uv run copilot-plugin-manager list profiles
uv run copilot-plugin-manager list themes
uv run copilot-plugin-manager list sources
```

Run a one-off command straight from your terminal with `uvx`:

```bash
uvx copilot-plugin-manager list profiles
```

`uvx` is convenient for direct execution, but persistent shell completion is easier when the CLI is installed locally or run from a checked-out repository with `uv run`.

Activate a setup for the current repository:

```bash
uv run copilot-plugin-manager switch python-core
uv run copilot-plugin-manager switch-exclusive python-mcp
```

Refresh upstream sources and inspect state:

```bash
uv run copilot-plugin-manager repo-update --remote
uv run copilot-plugin-manager status
```

## Shell completion

Quick shell-init snippets:

```bash
uv run copilot-plugin-manager shell-init bash
uv run copilot-plugin-manager shell-init zsh
uv run copilot-plugin-manager shell-init fish
uv run copilot-plugin-manager shell-init powershell
uv run copilot-plugin-manager shell-init nushell
```

Managed completion files:

```bash
uv run copilot-plugin-manager completion-install fish
uv run copilot-plugin-manager completion-install bash
uv run copilot-plugin-manager completion-script powershell
```

## Documentation

| Document | Purpose |
| --- | --- |
| [`docs/USAGE.md`](docs/USAGE.md) | Managed content, state model, command reference, and shell setup. |
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
