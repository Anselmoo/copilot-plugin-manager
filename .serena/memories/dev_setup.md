# Dev Setup for copilot-plugin-manager

## Build & Test
Use `uv` workflow:
```bash
uv python install 3.12
uv sync --group dev --python 3.12
uv run poe ci
```

Key commands:
- `uv run poe lint` - Run linter
- `uv run poe test` - Run tests
- `uv run pytest tests/test_cli.py -k <test_name>` - Run specific test
- `cargo test --workspace --quiet` - Run Rust tests
- `uv run cpm --help` / `cargo run -p cpm-cli -- --help` - CLI help

## Architecture
- **cpm-types**: Shared manifest, lockfile, settings, asset, transport types
- **cpm-core**: Execution pipeline (manifest I/O, normalization, resolution, license checks, installation)
- **cpm-cli**: Command handlers using clap, delegates to cpm-core
- **python/cpm**: Thin wrapper over Rust CLI via `cargo run`

## Key Files
- CLI commands: `crates/cpm-cli/src/commands/*`
- Manifest/lock orchestration: `crates/cpm-core/src/project.rs`
- Install paths: `crates/cpm-core/src/installer.rs`
- Types: `crates/cpm-types/src/lib.rs`

## Code Conventions
- Use `camino::Utf8PathBuf` for user-facing paths
- Use `chrono::DateTime<Utc>` for timestamps
- Manifest/lock handling flows through `cpm_core::project`
- Library crates deny: missing docs, unwrap, expect, panic
- Use `tracing` in core libraries
