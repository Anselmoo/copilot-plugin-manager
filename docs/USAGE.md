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
copilot-plugin-manager list [overview|all|sources|profiles|themes|plugins|skills|agents]
copilot-plugin-manager status
copilot-plugin-manager install [all|plugins|skills|agents|thirdparty]
copilot-plugin-manager update [all|plugins|skills|agents|thirdparty]
copilot-plugin-manager delete [all|plugins|skills|agents|thirdparty]
copilot-plugin-manager switch <profile-or-theme> [--save-repo-profile] [--repo-profile-location root|github]
copilot-plugin-manager switch-exclusive <profile-or-theme> [--save-repo-profile] [--repo-profile-location root|github]
copilot-plugin-manager repo-update [--remote/--no-remote]
copilot-plugin-manager self-update
copilot-plugin-manager shell-init <bash|zsh|fish|powershell|nushell>
copilot-plugin-manager completion-script <bash|zsh|fish|powershell|nushell>
copilot-plugin-manager completion-install <bash|zsh|fish|powershell|nushell> [--path PATH]
```

## Shell setup

Use `shell-init` for quick startup-file snippets:

```bash
uv run copilot-plugin-manager shell-init bash
uv run copilot-plugin-manager shell-init zsh
uv run copilot-plugin-manager shell-init fish
uv run copilot-plugin-manager shell-init powershell
uv run copilot-plugin-manager shell-init nushell
```

Use `completion-script` to inspect the full generated source:

```bash
uv run copilot-plugin-manager completion-script bash
uv run copilot-plugin-manager completion-script powershell
```

Use `completion-install` to write a completion file to a user-level location:

```bash
uv run copilot-plugin-manager completion-install fish
uv run copilot-plugin-manager completion-install bash
uv run copilot-plugin-manager completion-install nushell
```

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
uvx copilot-plugin-manager list profiles
```

Treat `uvx` as a convenience path for direct terminal use. For persistent completion integration, prefer an installed entry point or `uv run` from a local checkout.

Force ASCII-only output with:

```bash
export COPILOT_PLUGINS_ASCII=1
```

## Repo-local profile management

Use `switch` or `switch-exclusive` with `--save-repo-profile` to write the selected target into a repo-local hint file:

```bash
copilot-plugin-manager switch python-core --save-repo-profile
copilot-plugin-manager switch ts --save-repo-profile --repo-profile-location github
```

`--repo-profile-location root` writes `.copilot-profile` in the detected project root. `--repo-profile-location github` writes `.github/copilot-profile`.

## Sync warnings

When a third-party skill provider contains missing or dangling entries, the manager now skips the broken paths, persists a warning in state, and shows those warnings in `status` output. This makes partial syncs visible without aborting the entire profile switch.
