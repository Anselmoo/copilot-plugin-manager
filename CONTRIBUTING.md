# Contributing to copilot-plugin-manager

Thanks for contributing.

## Ground rules

- Be respectful and follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Prefer focused pull requests with clear commit messages.
- Keep `README.md`, `docs/USAGE.md`, and `docs/RELEASING.md` aligned when commands or workflows change.
- If you discover a security issue, do not open a public issue. Follow the reporting process in [SECURITY.md](SECURITY.md).

## What changed in this repo

`copilot-plugin-manager` is now a Rust-first project. Product logic lives in the Cargo workspace under `crates/`, while the Python package in `python/cpm/` is a thin launcher that delegates to the Rust CLI. The old bundled catalog/submodule manager is gone.

## Development setup

Install the supported toolchains and create the local environment:

```bash
uv python install 3.12
uv sync --group dev --python 3.12
```

Optional but recommended local extras:

```bash
lefthook install
```

The installed `lefthook` setup keeps pre-commit checks lightweight; heavier validation runs on push and in CI/CD.

## Running the CLI locally

Use whichever entrypoint is most convenient:

```bash
uv run cpm --help
uv run copilot-plugin-manager --help
python -m cpm --help
cargo run -p cpm-cli -- --help
```

Inside a source checkout, the Python entrypoints delegate to `cargo run -p cpm-cli -- ...`, so you are always exercising the current Rust implementation.

## Common tasks

Developer tasks are wired through Poe the Poet:

```bash
uv run poe format
uv run poe lint
uv run poe test
uv run poe ci
uv run poe ci-full
```

Focused commands:

```bash
uv run pytest tests/test_cli.py -q
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

`uv run poe ci-full` is the closest match to the full CI verification path.

## Project layout

- `crates/cpm-cli/`: clap-based CLI surface
- `crates/cpm-core/`: manifest, lockfile, resolution, install, and status logic
- `crates/cpm-types/`: shared manifest and lockfile types
- `python/cpm/`: Python launcher package and compatibility entrypoints
- `tests/`: Python integration tests for the wrapper and end-to-end flows
- `cpm.toml`: self-hosted manifest example for this repo
- `cpm-reference.toml`: heavily commented reference manifest

## Documentation expectations

When you change behavior, update the docs that describe it in the same pull request. In particular:

- `README.md` for user-facing overview and quickstart
- `docs/USAGE.md` for command and workflow guidance
- `docs/RELEASING.md` for release process changes
- `docs/CREDITS.md` when checked-in example assets or upstream attributions change

## Pull request checklist

Before opening a pull request:

- run `uv run poe ci-full`
- update docs or examples if command behavior changed
- add or update tests for behavior changes
- mention any intentionally breaking behavior changes in the PR description
