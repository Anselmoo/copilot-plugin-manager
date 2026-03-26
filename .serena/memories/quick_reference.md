# Quick Task Reference

## Adding a New CLI Command
1. Define command struct in `crates/cpm-cli/src/commands/`
2. Impl handler logic (delegate to `cpm_core` when possible)
3. Register in `crates/cpm-cli/src/main.rs` clap subcommand
4. Add tests in `tests/test_cli.py` (Python) or `crates/cpm-cli/tests/`

## Manifest Format Changes
- Keep `cpm.toml` canonical sections: `[package]`, `[settings]`, `[sources]`, `[plugins]`, `[skills]`, `[agents]`, `[mcps.<name>]`, `[groups.<name>.*]`
- Backward-compatible with legacy nested tables during parse
- Use canonical form in writer (see `cpm_core::project` for logic)
- Repo-relative paths preferred; avoid absolute checkoutpaths

## Key Architectural Constraints
- Asset flow: `cpm add` → normalize → resolve → write manifest/lock → materialize
- `cpm lock` resolves without install
- `cpm sync` re-materializes from manifest/lock with filters
- MCP: protocol (`stdio`, `http`, `sse`) + runner (`uvx`, `npx`, `docker`, `binary`, `local`, `command`)
- Entrypoint-aware runtime: `uvx --from <pkg> <entrypoint>` vs `npx -y --package <pkg> <entrypoint>`
- Lockfile canonical shape: `files = [{ path = "...", sha256 = "...", executable = true }]`
