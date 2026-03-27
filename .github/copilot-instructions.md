# Copilot instructions for `copilot-plugin-manager`

## Build, test, and lint

Use the `uv` workflow for local validation:

```bash
uv python install 3.12
uv sync --group dev --python 3.12
uv run poe ci
```

Repo-specific commands:

```bash
uv run poe lint
uv run poe test
uv run pytest -q
uv run pytest tests/test_cli.py -k uv_run_materializes_local_skill_and_lockfile
cargo test --workspace --quiet
cargo test -p cpm-core manifest_writer_emits_canonical_arch2_shape -- --exact
cargo run -p cpm-cli -- --help
uv run cpm --help
```

`uv run cpm ...` is the preferred local CLI path; the Python package is only a thin wrapper over the Rust CLI.

## High-level architecture

This repo is a Rust-first CLI with a Python developer wrapper:

- `crates/cpm-cli` is the real product entrypoint. Command handlers load `cpm.toml`, merge runtime config, and call into `cpm-core`.
- `crates/cpm-core` owns the important execution pipeline: manifest/lockfile I/O, source normalization, ref resolution, license checks, source rewrites, and installation/materialization.
- `crates/cpm-types` defines the shared manifest, lockfile, settings, asset, and transport types used across the workspace.
- `python/cpm/_cli.py` does not implement product logic; it delegates to `cargo run -p cpm-cli -- ...` inside a source checkout, or to a configured/installed binary outside one.

The main asset flow is:

1. `cpm add` normalizes the source, resolves pinned revs, writes `cpm.toml`, writes `cpm.lock`, and materializes files immediately.
2. `cpm lock` resolves and writes `cpm.lock` without installation.
3. `cpm sync` re-materializes from the manifest or existing lock, applying scope/group filters and license policy checks.

## Key conventions

- `cpm.toml` is human-edited and project-oriented. Canonical sections are `[package]`, `[settings]`, `[sources]`, `[plugins]`, `[skills]`, `[agents]`, `[mcps]`, `[hooks]`, `[workflows]`, `[instructions]`, and optional `[groups.<name>]` metadata. Asset membership should normally be authored inline with `groups = ["<name>"]` (or `groups = ["default", "<name>"]` for multi-membership) rather than nested `[groups.<name>.*]` tables.
- The manifest loader is backward-compatible with legacy nested tables like `[plugins.partners]`, but the writer should emit the canonical project-style form instead. Do not reintroduce generic `toml::to_string_pretty(manifest)` serialization for `cpm.toml`.
- Repo-level `[sources]` rules are merged with user config sources; user config overrides same-named repo rules. Settings precedence is CLI flags > environment > repo `[settings]` > user config > built-in defaults.
- Local asset paths stored in `cpm.toml` should remain repo-relative when possible. Avoid persisting checkout-specific absolute paths.
- `cpm add` is expected to install immediately, not just update the manifest. Tests and docs assume that adding an asset also materializes `.github/...` content.
- Skills and plugins normalize to tree-level GitHub sources because they install as folders, not single markdown files. Collection roots like `.../skills` or `.../plugins` are catalogs and should be rejected as add targets.
- MCPs distinguish protocol from runner. Canonical authoring uses protocol/type (`stdio`, `http`, `sse`) plus a stdio runner (`uvx`, `npx`, `docker`, `binary`, `local`, `command`) instead of overloading one transport field; keep legacy parsing compatibility, but preserve the split in new write paths.
- For stdio package runners, keep entrypoint-aware runtime emission intact: `uvx` with an entrypoint becomes `uvx --from <package> <entrypoint> ...`, while `npx` becomes `npx -y --package <package> <entrypoint> ...`. Do not flatten git-backed package installs into a single opaque command string.
- The lockfile uses a custom canonical writer/reader at the persistence boundary. Keep lockfile compatibility logic in `crates/cpm-core/src/project.rs`, not spread across command handlers.
- The canonical lockfile file shape is structured inline entries like `files = [{ path = "...", sha256 = "...", executable = true }]`; keep legacy read compatibility, but do not regress the writer back to bare string paths.
- MCP runtime config writing and merge compatibility belong in `crates/cpm-core/src/installer.rs`. The current Copilot-facing JSON shape is rooted under `"servers"` in `.vscode/mcp.json` or `~/.copilot/mcp-config.json`.
- Progress reporting should stay dual-mode: rich `indicatif` output for interactive terminals, plain structured progress lines for CI/non-TTY output, with `CPM_PROGRESS=rich|plain` as the explicit override.
- Python tests in `tests/test_cli.py` intentionally verify the `uv run` / `python -m cpm` delegation path, while Rust tests cover manifest, lockfile, config, and materialization logic.
