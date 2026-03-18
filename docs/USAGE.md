# Usage guide

## What the manager handles

`copilot-plugin-manager` manages three content types:

- marketplace or GitHub-sourced Copilot plugins
- local skills copied into `~/.copilot/skills`
- local agents copied into `~/.copilot/agents`

Profiles and themes are bundled in the catalog data under `src/copilot_plugin_manager/catalog_data/`.

## Catalog data

The bundled catalogs are split into two layers:

- hand-maintained structural catalogs such as repositories, providers, themes, and profiles
- generated entrypoint and provenance data in `entrypoints.toml`

The committed snapshot in `src/copilot_plugin_manager/catalog_data/` is the runtime source of truth.

Refresh generated metadata from the current submodules:

```bash
uv run poe refresh-catalog
```

Before refreshing, you can scan the repository and initialized upstream trees for dangling symlinks:

```bash
uv run poe broken-links
```

Hard-reset entrypoint history and rebuild provider metadata directly from upstream target files:

```bash
uv run poe refresh-catalog-reset
```

## Upstream sources

The repository tracks the upstream catalogs via submodules:

- `external/awesome-copilot`
- `external/anthropics-skills`
- `external/kdense-science`
- `external/microsoft-hve-core`
- `external/microsoft-skills`
- `external/agency-agents`
- `external/voltagent-subagents`

Initialize them with:

```bash
git submodule update --init --recursive
```

Refresh them with:

```bash
uv run copilot-plugin-manager repo-update
```

`repo-update` persists the latest git revision and, when available, generic manifest versions exposed by upstream repositories.

## State model

Global manager state is stored under:

- `~/.copilot/copilot-plugin-manager/state.json`
- `~/.copilot/copilot-plugin-manager/sources/`

Compatibility state is also written to:

- `~/.copilot/active-profile`

Repository-specific setup history is tracked per detected project root in the manager state file. Repo-local hints such as `.copilot-profile` and `.github/copilot-profile` are still honored for autodetection.

Repo activation state is normalized to the detected project root, so switching from one subdirectory is reflected when you later run commands from another subdirectory in the same repository.

Generated entrypoint data also records:

- exact upstream source path
- Copilot-local output name
- measured git revision
- `measured_at`, `first_seen_at`, and `last_seen_at`

## Command reference

```text
copilot-plugin-manager
copilot-plugin-manager menu
copilot-plugin-manager list [overview|all|sources|profiles|themes|plugins|skills|agents|mcps]
copilot-plugin-manager repo-init [profile-or-theme] [--repo-profile-location root|github] [--agent-scope global|local] [--mcp-scope global|local] [--mcp-profile NAME] [--force]
copilot-plugin-manager repo-cleanup [profile-or-theme] [--agent-scope global|local]
copilot-plugin-manager repo-config [--agent-scope global|local] [--mcp-scope global|local] [--mcp-profile NAME]
copilot-plugin-manager status
copilot-plugin-manager install [all|plugins|skills|agents|thirdparty] [--agent-scope global|local] [--mcp-scope global|local]
copilot-plugin-manager update [all|plugins|skills|agents|thirdparty] [--agent-scope global|local] [--mcp-scope global|local]
copilot-plugin-manager delete [all|plugins|skills|agents|thirdparty] [--agent-scope global|local] [--mcp-scope global|local]
copilot-plugin-manager switch <profile-or-theme> [--agent-scope global|local] [--save-repo-profile] [--repo-profile-location root|github]
copilot-plugin-manager switch-exclusive <profile-or-theme> [--agent-scope global|local] [--save-repo-profile] [--repo-profile-location root|github]
copilot-plugin-manager repo-update [--remote/--no-remote]
copilot-plugin-manager self-update
copilot-plugin-manager completion init <bash|zsh|fish|powershell|nushell>
copilot-plugin-manager completion script <bash|zsh|fish|powershell|nushell>
copilot-plugin-manager completion install <bash|zsh|fish|powershell|nushell> [--path PATH]
```

`copilot-plugin-manager` with no subcommand now opens a guided interactive menu when running in an interactive terminal. In non-interactive contexts, it falls back to a compact status view.

`copilot-plugin-manager list` follows the same pattern: in an interactive terminal it opens a catalog browser with focused views for overview, profiles, themes, sources, plugins, skills, agents, and MCPs. In non-interactive contexts, or when you pass a section explicitly, it renders that specific section directly.

## Repo config and scoped sync

Repository-local settings are split intentionally:

- `.copilot-profile` or `.github/copilot-profile` stores the selected profile or theme name for the repo.
- `.github/copilot-plugin-manager.json` stores repo-local defaults for agent scope, MCP scope, and preferred MCP profile.

Before writing either file, inspect the current bundled composition with:

```bash
copilot-plugin-manager list profiles
copilot-plugin-manager list themes
```

You can also review `docs/THEMES.md` for the generated profile/theme composition reference.

Use the shared repo config file at `.github/copilot-plugin-manager.json` when you want repository-local defaults for agent and MCP scope:

```bash
copilot-plugin-manager repo-config --agent-scope local
copilot-plugin-manager repo-config --mcp-scope local --mcp-profile team
copilot-plugin-manager repo-config
```

Use `repo-init` when a repository does not have a target hint yet and you want to create one explicitly without changing installed plugins, skills, or agents:

```bash
copilot-plugin-manager repo-init python-core
copilot-plugin-manager repo-init --repo-profile-location github
copilot-plugin-manager repo-init python-core --agent-scope local --mcp-scope local --mcp-profile team
```

Use `repo-cleanup` when `status` surfaces verification warnings about missing or unexpected managed content and you want an explicit cleanup pass:

```bash
copilot-plugin-manager repo-cleanup
copilot-plugin-manager repo-cleanup python-core
copilot-plugin-manager repo-cleanup --agent-scope local
```

When agent scope is `local`, synced agents are rewritten into `.github/agents/*.agent.md` using basename-oriented names. Global scope keeps provider-prefixed outputs in `~/.copilot/agents`.

You can also override the effective scope per command without changing the saved repo config:

```bash
copilot-plugin-manager install thirdparty --agent-scope local
copilot-plugin-manager update mcps --mcp-scope local
copilot-plugin-manager switch python-core --agent-scope local
```

## Shell setup

Use `completion init` for quick startup-file snippets:

```bash
uv run copilot-plugin-manager completion init bash
uv run copilot-plugin-manager completion init zsh
uv run copilot-plugin-manager completion init fish
uv run copilot-plugin-manager completion init powershell
uv run copilot-plugin-manager completion init nushell
```

Use `completion script` to inspect the full generated source:

```bash
uv run copilot-plugin-manager completion script bash
uv run copilot-plugin-manager completion script powershell
```

Use `completion install` to write a completion file to a user-level location:

```bash
uv run copilot-plugin-manager completion install fish
uv run copilot-plugin-manager completion install bash
uv run copilot-plugin-manager completion install nushell
```

Legacy top-level aliases (`shell-init`, `completion-script`, and `completion-install`) are still available for existing scripts, but they are hidden from the main help output.

Notes by shell:

- `bash`: installs to an XDG-style `bash-completion` path. If your shell does not auto-load it, source the file from your startup config.
- `zsh`: installs to `~/.zfunc/_copilot-plugin-manager`. If `~/.zfunc` is not already on `fpath`, add it and run `compinit`.
- `fish`: installs to the standard Fish completions directory and auto-loads from there.
- `powershell`: installs a `.ps1` completion file. Add a dot-source line for that file to `$PROFILE`.
- `nushell`: installs a `.nu` completion file. Add a `source ...` line for it to `config.nu`.

## Terminal-first usage

For quick one-off execution without installing the package into your environment, `uvx` works well:

```bash
uvx copilot-plugin-manager status
uvx copilot-plugin-manager list
uvx copilot-plugin-manager list profiles
```

Treat `uvx` as a convenience path for direct terminal use. For persistent completion integration, prefer an installed entry point or `uv run` from a local checkout.

Force ASCII-only output with:

```bash
export COPILOT_PLUGINS_ASCII=1
```

## Repo-local profile management

Use `switch` or `switch-exclusive` with `--save-repo-profile` to write the selected profile or theme into a repo-local target hint file:

```bash
copilot-plugin-manager switch <profile-or-theme> --save-repo-profile
copilot-plugin-manager switch <profile-or-theme> --save-repo-profile --repo-profile-location github
copilot-plugin-manager status
```

`--repo-profile-location root` writes `.copilot-profile` in the detected project root. `--repo-profile-location github` writes `.github/copilot-profile`.

After saving a repo-local target hint, use `status` to confirm the resolved target type, themes, and any repo-local scope defaults from `.github/copilot-plugin-manager.json`.

If you want to initialize the repo-local hint without changing the active installation, prefer `repo-init`.

## Sync warnings

When a third-party provider contains missing or dangling entries, the manager skips the broken paths, persists a warning in state, and shows those warnings in `status` output.

Profile switches also run a post-apply verification step. If the requested target was selected but the applied plugins / skills / agents do not fully match, the manager persists a strong verification warning instead of silently claiming success.

When those verification warnings mention missing or unexpected managed content, run `repo-cleanup` for an explicit reconciliation pass.
