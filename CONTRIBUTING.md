# Contributing to copilot-plugin-manager

Thanks for contributing.

## Ground rules

- Be respectful and follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Prefer focused pull requests with clear commit messages.
- Keep documentation in sync when behavior or workflows change.
- If you discover a security issue, do not open a public issue. Follow the reporting process in [SECURITY.md](SECURITY.md).

## Security reporting

For vulnerabilities or suspected security issues, use the private disclosure instructions in [SECURITY.md](SECURITY.md) instead of GitHub issues or pull requests.

## Development setup

Clone the repository, initialize upstream catalogs, and install the development environment:

```bash
git submodule update --init --recursive
uv sync --group dev
uv run pre-commit install
```

Run the CLI locally with:

```bash
uv run copilot-plugin-manager --help
```

## Common tasks

Developer tasks are wired through Poe the Poet:

```bash
uv run poe test
uv run poe test-cov
uv run poe pre-commit
uv run poe lint
uv run poe typecheck
uv run poe check
uv run poe build
```

Equivalent direct commands are also available when needed:

```bash
uv run pytest -q
uv run pre-commit run --all-files
uv run ruff check .
uv run ty check
uv build
uv run twine check dist/*
```

## Catalog maintenance

The bundled runtime catalog lives under `src/copilot_plugin_manager/catalog_data/`.

Refresh generated catalog metadata from the current submodules:

```bash
uv run poe refresh-catalog
```

Hard-reset entrypoint provenance history and rebuild provider metadata from upstream target files:

```bash
uv run poe refresh-catalog-reset
```

To update submodules from their remotes and persist the latest revision metadata:

```bash
uv run copilot-plugin-manager repo-update --remote
```

## Pull request checklist

Before opening a pull request:

- run `uv run poe check`
- run `uv run poe pre-commit` when touching Python files or workflow/config glue
- run `uv run poe build` when packaging behavior changes
- update docs or examples if command behavior changed
- add or update tests for behavior changes

## Project layout

- `src/copilot_plugin_manager/`: application code
- `src/copilot_plugin_manager/catalog_data/`: bundled catalog snapshot
- `test/`: automated tests
- `scripts/refresh_catalog.py`: catalog regeneration entrypoint
- `external/`: upstream catalog submodules
