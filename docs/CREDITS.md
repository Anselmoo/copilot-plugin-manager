# Credits

`copilot-plugin-manager` no longer vendors the large upstream catalog snapshot that powered the previous Python-only implementation. The current Rust-first project resolves assets directly from URLs or local paths declared in `cpm.toml`.

This repository does still check in a small amount of example content for docs, smoke checks, and fixtures. Those checked-in examples should remain attributable even though they are no longer part of a generated catalog.

## Checked-in example assets

| Path | Upstream/source | Notes | License |
| --- | --- | --- | --- |
| `.github/plugins/partners/` | [`github/awesome-copilot`](https://github.com/github/awesome-copilot) | Partner plugin bundle with checked-in agent definitions used as a realistic plugin fixture. | MIT |
| `.github/plugins/edge-ai-tasks/` | [`github/awesome-copilot`](https://github.com/github/awesome-copilot) and [`microsoft/edge-ai`](https://github.com/microsoft/edge-ai) | Planner/researcher plugin fixture bundled from the Awesome Copilot ecosystem. | MIT |
| `skills/alpha/SKILL.md` | Local repository example | Minimal skill fixture used for tests and examples. | Repository license |

## What changed from the old repo

The deleted `external/` catalog mirrors and generated credits tables belonged to the removed Python catalog manager. They are intentionally not recreated here because `cpm` does not depend on vendored upstream catalogs to function.

## Maintenance note

If you add or remove checked-in third-party example assets, update this file in the same pull request so licensing and attribution stay obvious.
