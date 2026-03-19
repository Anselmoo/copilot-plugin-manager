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
- sync local skills plus agent outputs into either `~/.copilot` or `<repo>/.github/agents`
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

Open the guided menu for the current repository context:

```bash
copilot-plugin-manager
```

The top-level interactive menu now keeps switching and sync operations separate from discovery. Use the dedicated catalog browser submenu when you want profiles, themes, plugins, skills, agents, or MCPs without crowding the main menu.

For repo-local customization on top of the bundled catalog, use the `project` subgroup:

```bash
copilot-plugin-manager project init minimal --agent-scope local
copilot-plugin-manager project add skill kdense-bindingdb-database --theme core-dev --profile research-dev
copilot-plugin-manager project add theme paper-writing-review --profile research-dev
```

Browse what is available:

```bash
copilot-plugin-manager catalog
copilot-plugin-manager catalog profiles
copilot-plugin-manager catalog themes
copilot-plugin-manager catalog sources
copilot-plugin-manager catalog mcps
```

In an interactive terminal, bare `catalog` opens a compact catalog browser instead of dumping every section at once. Use an explicit section such as `catalog overview` when you want a stable non-interactive view for scripts or copy/paste output. The legacy `list` command remains available as a compatibility alias.

Activate a setup for the current repository:

```bash
copilot-plugin-manager switch python-core
copilot-plugin-manager switch paper-writing-review
copilot-plugin-manager switch python-core --agent-scope local
copilot-plugin-manager switch quantum-chemistry
copilot-plugin-manager switch python-core --save-repo-profile
copilot-plugin-manager switch-exclusive python-mcp
```

Initialize or clean up repo-local state explicitly:

```bash
copilot-plugin-manager repo-init python-core
copilot-plugin-manager repo-init --agent-scope local --mcp-scope local --mcp-profile team
copilot-plugin-manager repo-cleanup
```

Opt a repository into local agent and MCP behavior with the shared repo config:

```bash
copilot-plugin-manager repo-config --agent-scope local
copilot-plugin-manager repo-config --mcp-scope local --mcp-profile team
copilot-plugin-manager status
```

Refresh upstream sources and inspect state:

```bash
copilot-plugin-manager repo-update --remote
copilot-plugin-manager status
```

`status` now surfaces the repo-local target hint, the resolved theme composition, repo settings files, and persisted sync / verification warnings, so partial syncs or “selected target does not match the applied environment” problems are easier to spot.

When `status` shows verification warnings about missing or unexpected managed content, run `copilot-plugin-manager repo-cleanup` to reconcile the repo explicitly instead of relying on a read-only command to mutate state.

For repository-local setup, treat the files as complementary:

- `.copilot-profile` or `.github/copilot-profile` stores the selected profile or theme name for the repository.
- `.github/copilot-plugin-manager.json` stores repo-local agent/MCP defaults such as scope and preferred MCP profile.
- `copilot-plugin-manager status` lets you confirm both files and the effective resolved composition after catalog changes.

When bundled profile/theme composition changes, check `copilot-plugin-manager list profiles`, `copilot-plugin-manager list themes`, or `docs/THEMES.md` before writing a new repo-local target hint.
When bundled profile/theme composition changes, check `copilot-plugin-manager catalog profiles`, `copilot-plugin-manager catalog themes`, or `docs/THEMES.md` before writing a new repo-local target hint.

When you want project-specific themes or profiles layered on top of the bundled catalog, `project add` writes `.github/copilot-project-catalog.toml` in the repository. It can extend existing bundled themes, create new repo-local themes on demand, and optionally attach them to new or existing profiles.

When agent scope is `local`, synced agents are rewritten into `.github/agents/*.agent.md` using basename-friendly filenames such as `regular.agent.md`. Global scope keeps provider-prefixed outputs under `~/.copilot/agents`.

If you maintain the bundled upstream catalogs, `uv run poe broken-links` catches dangling symlinks in the repository and initialized submodules before refresh or sync work.

If you prefer one-off execution with `uvx`, the same commands work there too:

```bash
uvx copilot-plugin-manager list
uvx copilot-plugin-manager list profiles
uvx copilot-plugin-manager status
```

`uvx` is convenient for direct execution, but persistent shell completion is easiest when the CLI is installed locally.

## Catalog overview

<!-- generated:catalog-overview:start -->
_This section is generated from the bundled catalog data with `uv run poe generate-docs`._

- `44` profiles
- `34` themes
- `53` plugins
- `210` skill providers
- `58` agent providers

See also:
- [`docs/THEMES.md`](docs/THEMES.md) for the full theme and profile composition reference.
- [`docs/CREDITS.md`](docs/CREDITS.md) for upstream catalog credits.

### Current profile compositions

| Profile | Themes |
| --- | --- |
| `agentic-fullstack` | `core`, `data`, `frontend`, `mcp-agents`, `python`, `python-agents`, `security`, `testing`, `typescript` |
| `backend` | `core`, `data`, `openapi`, `python`, `security`, `testing` |
| `backend-api` | `core`, `data`, `openapi`, `python`, `security`, `testing` |
| `bioinformatics` | `bioinformatics`, `core`, `data`, `python`, `research`, `science` |
| `chemistry-research` | `chemistry`, `core`, `data`, `paper-writing-review`, `python`, `research`, `science` |
| `data-ai` | `core`, `data`, `ml-ai`, `research`, `science` |
| `data-science` | `core`, `data`, `ml-ai`, `python`, `research`, `science`, `testing` |
| `devops-sec` | `core`, `devops`, `github`, `security` |
| `docs` | `core`, `docs`, `python`, `testing` |
| `docs-lite` | `core`, `docs` |
| `docs-pro` | `core`, `docs`, `docs-design`, `planning`, `testing` |
| `dotnet-dev` | `core`, `devops`, `dotnet`, `testing` |
| `drug-discovery` | `bioinformatics`, `chemistry`, `core`, `data`, `python`, `research`, `science` |
| `enterprise` | `core`, `data`, `devops`, `enterprise`, `testing` |
| `enterprise-architect` | `core`, `enterprise`, `github`, `planning`, `security`, `testing` |
| `everything` | `agents`, `bioinformatics`, `chemistry`, `clinical`, `core`, `data`, `devops`, `docs`, `docs-design`, `dotnet`, `enterprise`, `frontend`, `github`, `infra`, `mcp`, `mcp-agents`, `ml-ai`, `openapi`, `paper-writing-review`, `planning`, `python`, `python-agents`, `python-cloud`, `quantum`, `quantum-chemistry`, `research`, `science`, `scientific-methods`, `security`, `specialized`, `spectroscopy`, `testing`, `typescript` |
| `fastapi-typer` | `core`, `openapi`, `python`, `testing` |
| `frontend-design` | `core`, `docs-design`, `frontend`, `testing`, `typescript` |
| `fullstack` | `core`, `data`, `frontend`, `python`, `testing`, `typescript` |
| `healthcare` | `clinical`, `core`, `data`, `planning`, `python`, `research`, `science` |
| `infra-platform` | `core`, `devops`, `github`, `infra`, `security` |
| `mcp-dev` | `core`, `mcp`, `python`, `testing` |
| `minimal` | `core` |
| `ml-engineering` | `core`, `data`, `ml-ai`, `python`, `science`, `testing` |
| `paper-writing-review` | `core`, `docs`, `paper-writing-review`, `planning`, `research` |
| `planner` | `core`, `github`, `planning` |
| `polyglot` | `core`, `dotnet`, `python`, `security`, `testing`, `typescript` |
| `pydantic` | `core`, `openapi`, `python`, `testing` |
| `python-agents` | `core`, `python`, `python-agents`, `testing` |
| `python-cloud` | `core`, `devops`, `python`, `python-cloud`, `testing` |
| `python-core` | `core`, `python`, `testing` |
| `python-dev` | `core`, `devops`, `python`, `testing` |
| `python-mcp` | `core`, `mcp`, `mcp-agents`, `python`, `python-agents`, `testing` |
| `python-plus-rust` | `core`, `data`, `python`, `rust`, `testing` |
| `quantum-chemistry` | `core`, `paper-writing-review`, `python`, `quantum`, `quantum-chemistry`, `research`, `science` |
| `quantum-computing` | `core`, `python`, `quantum`, `research`, `science` |
| `research` | `core`, `docs`, `paper-writing-review`, `planning`, `research`, `science` |
| `science` | `core`, `data`, `python`, `science` |
| `scientific-methods` | `core`, `paper-writing-review`, `research`, `science`, `scientific-methods` |
| `scientific-programming` | `core`, `data`, `python`, `research`, `science` |
| `spectroscopy` | `chemistry`, `core`, `data`, `paper-writing-review`, `python`, `research`, `science`, `spectroscopy` |
| `ts` | `core`, `frontend`, `testing`, `typescript` |
| `ts-fullstack` | `core`, `devops`, `frontend`, `mcp`, `mcp-agents`, `testing`, `typescript` |
| `ts-mcp` | `core`, `frontend`, `mcp`, `mcp-agents`, `testing`, `typescript` |
<!-- generated:catalog-overview:end -->

## Shell completion

The visible completion workflow now lives under a single `completion` command:

```bash
copilot-plugin-manager completion init bash
copilot-plugin-manager completion init zsh
copilot-plugin-manager completion init fish
copilot-plugin-manager completion init powershell
copilot-plugin-manager completion init nushell
```

Managed completion files:

```bash
copilot-plugin-manager completion install fish
copilot-plugin-manager completion install bash
copilot-plugin-manager completion script powershell
```

Legacy top-level aliases (`shell-init`, `completion-script`, and `completion-install`) still work for backward compatibility.

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
