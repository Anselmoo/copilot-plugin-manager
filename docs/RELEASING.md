# Releasing and publishing

## Current release status

This repository validates, packages, and publishes through [`.github/workflows/cicd.yml`](../.github/workflows/cicd.yml). That workflow:

- verifies Python formatting, linting, and type checks
- runs Rust formatting and clippy
- runs Python and Rust tests
- builds platform-specific Python wheels and an sdist
- validates distributions with Twine
- publishes tagged releases through TestPyPI, PyPI, and GitHub Releases
- attaches an SPDX SBOM to the release artifacts

Pushes to `main` publish validation artifacts to TestPyPI as unique CI development releases derived from the checked-in base version, for example `0.2.0.dev<run-id><attempt>`. Version tags of the form `v*.*.*` run the full final release flow.

## Pre-release checks

Before tagging a release, run:

```bash
uv sync --group dev --python 3.12
uv run poe ci-full
uv build
```

For repo-managed release prep, the new helper tasks mirror the workflow used in the companion tooling repo:

```bash
uv run poe changelog_preview
uv run poe bump_patch
# or: uv run poe bump_minor / uv run poe bump_major
```

If you changed the wrapper or packaging surface, it is also worth checking the compatibility entrypoints explicitly:

```bash
uv run pytest tests/test_cli.py -q
uv run cpm --help
uv run copilot-plugin-manager --help
uv run python -m cpm --help
```

## Version updates

Before creating a release tag, update the repository version metadata in the places that define the shipped Python package and workspace release version, including:

- `pyproject.toml`
- `Cargo.toml`
- `python/cpm/__init__.py`

If future Cargo crates stop inheriting the workspace version, update this document accordingly.

For branch-based TestPyPI publishes, do **not** commit ad-hoc versions such as `0.2.0-20260327` into `pyproject.toml`. The workflow computes a CI-only PEP 440 development release version at build time so repeated uploads stay unique without mutating the source-controlled release version.

## Release flow

1. Update version metadata and any release notes you want to publish.
2. Run the full validation commands locally.
3. Build local distributions with `uv build` if you want to inspect the sdist/wheel payload.
4. Create and push a version tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

After the tag is pushed, GitHub Actions will:

1. build macOS, Linux, and Windows wheel artifacts plus an sdist,
2. verify them with Twine,
3. publish to TestPyPI and smoke-test the install,
4. publish to PyPI,
5. attach the distributions and SBOM to the GitHub release.

## Follow-up work

If the release flow changes again, update this document alongside the workflow so the operational steps stay honest and boring — the good kind of boring.
