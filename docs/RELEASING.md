# Releasing and publishing

## Current release status

This migrated repository currently validates and builds through [`.github/workflows/ci.yml`](../.github/workflows/ci.yml). That workflow:

- verifies Python formatting, linting, and type checks
- runs Rust formatting and clippy
- runs Python and Rust tests
- builds release binaries and Python wheels as CI artifacts

The previous Python-only publish workflow was not carried forward into this Rust-first migration. Until dedicated publish automation is added back, treat releases as a validated source-control and packaging process rather than an automated upload pipeline.

## Pre-release checks

Before tagging a release, run:

```bash
uv sync --group dev --python 3.12
uv run poe ci-full
uv build
```

If you changed the wrapper or packaging surface, it is also worth checking the compatibility entrypoint explicitly:

```bash
uv run pytest tests/test_cli.py -q
uv run copilot-plugin-manager --help
```

## Version updates

Before creating a release tag, update the repository version metadata in the places that define the shipped Python package and workspace release version, including:

- `pyproject.toml`
- `Cargo.toml`
- `python/cpm/__init__.py`

If future Cargo crates stop inheriting the workspace version, update this document accordingly.

## Release flow

1. Update version metadata and any release notes you want to publish.
2. Run the full validation commands locally.
3. Build local distributions with `uv build` if you want to inspect the sdist/wheel payload.
4. Create and push a version tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## Follow-up work

If automated publishing is reintroduced, update this document and link the new workflow here. The release story is intentionally conservative right now because the codebase replacement changed the build, packaging, and runtime model substantially.
