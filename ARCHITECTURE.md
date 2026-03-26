# cpm — GitHub Copilot Instructions

## What this project is

`cpm` is a Rust-native package manager for GitHub Copilot assets. It follows the same
mental model as `uv` (astral-sh/uv) and Cargo: a fast Rust core, a thin Python launcher package for local and PyPI workflows,
and a deterministic lockfile (`cpm.lock`) that pins content hashes and
git SHAs, and a human-authored manifest (`cpm.toml`) that declares intent.

There is no bundled catalog, no registry, no hardcoded list of known assets. Every asset
is addressed by URL, path, or package name — exactly like `uv add git+https://...`.

---

## Repository layout

```
cpm/
├── crates/
│   ├── cpm-types/       # shared types — AssetKind, Scope, McpTransport, Manifest, Lock
│   ├── cpm-core/        # resolver, fetcher, installer, cache, doctor, status, auth
│   └── cpm-cli/         # clap CLI — all subcommands, progress bars, error rendering
├── python/              # thin Python launcher package
│   └── cpm/             # cpm Python package and compatibility entrypoints
├── tests/               # integration tests (workspace-level)
├── Cargo.toml           # workspace manifest
├── pyproject.toml       # uv_build metadata for the Python wrapper
└── cpm.toml             # self-hosting: cpm's own asset dependencies
```

---

## Asset taxonomy — six kinds, two scopes

cpm manages six distinct asset kinds. Every kind follows the same add/sync/update/remove
lifecycle. Scope is either `local` (.github/) or `global` (~/.copilot/).

```
┌──────────┬──────────────────────────────────────────┬──────────────────────────┐
│ Kind     │ What it is                               │ Installed as             │
├──────────┼──────────────────────────────────────────┼──────────────────────────┤
│ plugin   │ Copilot extension config file            │ *.yml / *.json           │
│ skill    │ Markdown prompt library                  │ *.md                     │
│ agent    │ Agent definition (.agent.md)             │ *.agent.md               │
│ mcp      │ MCP server — any transport               │ *.mcp.json config        │
│ hook     │ Event-driven CCA session scripts         │ hooks.json + *.sh bundle │
│ workflow │ Agentic workflow source (natural lang.)  │ *.md (compiled by gh aw) │
└──────────┴──────────────────────────────────────────┴──────────────────────────┘

Install paths:
┌──────────┬──────────────────────┬──────────────────────────┐
│ Kind     │ local                │ global                   │
├──────────┼──────────────────────┼──────────────────────────┤
│ plugin   │ .github/plugins/     │ ~/.copilot/plugins/      │
│ skill    │ .github/skills/      │ ~/.copilot/skills/       │
│ agent    │ .github/agents/      │ ~/.copilot/agents/       │
│ mcp      │ .github/mcp/         │ ~/.copilot/mcp/          │
│ hook     │ .github/hooks/       │ ~/.copilot/hooks/        │
│ workflow │ .github/workflows/   │ ERROR — always local     │
└──────────┴──────────────────────┴──────────────────────────┘
```

Workflow scope is always `local`. cpm must return `Err(CpmError::InvalidScope)` if
`scope = "global"` is set on any workflow entry — GitHub Actions reads `.github/workflows/`
which is inherently per-repo.

---

## Manifest — cpm.toml

The manifest is the only file a human ever edits. It declares intent. It never contains
SHAs, hashes, dates, file lists, or resolved versions — those live only in `cpm.lock`.

### Top-level tables

```toml
[package]          # project identity and THIS repo's SPDX license
[settings]         # repo-level defaults (scope, update policy, license policy)
[sources]          # private mirrors and auth overrides
[plugins]          # plugin declarations
[skills]           # skill declarations
[agents]           # agent declarations
[mcps]             # MCP server declarations (richest section)
[hooks]            # hook bundle declarations
[workflows]        # agentic workflow declarations
[groups.<name>]    # named opt-in groups (like Cargo dev-dependencies)
```

### Asset declaration forms

Every asset table supports two forms:

```toml
# Short form — URL only, inherits scope and group from [settings]
python-skills = "https://github.com/owner/python-skills"

# Long form — inline table for all options
python-skills = { url    = "https://github.com/owner/python-skills",
                  rev    = "v2.0.0",      # tag, branch, or commit — resolver pins to SHA
                  scope  = "global",      # overrides settings.default_scope
                  group  = "research",    # opt-in group name
                  path   = "./local/..." }# local path install (no network)
```

### [package] — THIS repo's identity and license

```toml
[package]
name        = "my-project"
description = "..."
version     = "0.1.0"
license     = "MIT"           # SPDX expression — describes THIS repo, not installed assets
authors     = ["Name <email>"]
repository  = "https://github.com/owner/repo"
created     = "2026-01-01"    # ISO date, set once by `cpm init`, never change
```

`license` uses SPDX expressions: `"MIT"`, `"Apache-2.0"`, `"MIT OR Apache-2.0"`,
`"GPL-3.0-only"`, `"UNLICENSED"` (proprietary). The licenses of installed assets are
fetched at resolve time and stored in `cpm.lock` — they never appear in `cpm.toml`.

### [settings] — repo-level defaults

```toml
[settings]
default_scope          = "local"          # "local" | "global"
update_policy          = "locked"         # "locked" | "latest" | "tagged"
license_policy         = "warn-copyleft"  # "allow-all" | "warn-copyleft" | "deny-copyleft" | "allow-list"
allowed_licenses       = []               # SPDX list for license_policy = "allow-list"
cache_dir              = "~/.cache/cpm"   # CPM_CACHE_DIR env var takes precedence
network_timeout        = 30              # seconds
auto_groups            = ["default"]      # groups installed by plain `cpm sync`
verify_on_sync         = false            # re-verify hashes even when rev matches
auto_compile_workflows = false            # run `gh aw compile` after workflow sync
```

### [sources] — private mirrors and URL rewriting

```toml
[sources.<name>]
url       = "https://git.internal.example.com"
token_env = "INTERNAL_GIT_TOKEN"      # env var name, never the value
replace   = "https://github.com/org"  # URLs starting with this are transparently rewritten
```

cpm.lock always records the original canonical URL, not the mirror URL. Token values
are never written to cpm.toml or cpm.lock.

### [mcps] — MCP transport types

Each MCP entry uses exactly one transport key. Transport is inferred from the key used.

```toml
# http — remote server, no local process
[mcps.<name>]
transport = "http"
url       = "https://..."

# sse — remote server over Server-Sent Events
[mcps.<name>]
transport = "sse"
url       = "https://..."

# npx — Node.js package, stdio, spawned via npx
[mcps.<name>]
transport = "npx"
package   = "@scope/package-name"
rev       = ">=1.0.0"             # npm version range; resolver pins exact version in lock
args      = ["--flag", "value"]
[mcps.<name>.env]
SECRET = "$ENV_VAR_NAME"          # $ prefix = resolved from process env at runtime

# uvx — Python package, stdio, spawned via uvx
[mcps.<name>]
transport = "uvx"
package   = "package-name"
rev       = ">=0.6.0"             # PEP 440 specifier; resolver pins exact version in lock

# docker — container, stdio, pulled at launch time (NOT at cpm sync)
[mcps.<name>]
transport = "docker"
image     = "ghcr.io/owner/image:tag"
args      = ["--flag"]

# binary — native binary (Rust, Go, C, Zig, …) from a GitHub release asset
# cpm infers OS/arch suffix automatically (x86_64-unknown-linux-gnu, etc.)
[mcps.<name>]
transport = "binary"
url       = "https://github.com/owner/repo/releases/latest"
bin       = "binary-name"         # filename inside the release archive
checksum  = "sha256:..."          # optional pre-verified hash; hard error on mismatch

# path — local dev binary (always local scope; cpm errors if scope = "global")
[mcps.<name>]
transport = "path"
path      = "./target/release/binary-name"
args      = ["--dev"]

# script — inline command, NOT passed through shell, no injection risk
[mcps.<name>]
transport = "script"
command   = "python"
args      = ["-m", "my_server"]
```

### [hooks] — event-driven CCA session scripts

Hook source repos must contain `hooks.json` (event bindings) and referenced `.sh`
scripts. cpm installs the entire folder and ensures all scripts are `chmod +x`.

Hook events and their blocking behaviour:
```
preToolUse   — BLOCKING: exit non-zero to deny the tool call
postToolUse  — non-blocking: exit code ignored
sessionStart — non-blocking
sessionEnd   — non-blocking
userPromptSubmitted — non-blocking
agentStop    — non-blocking
subagentStop — non-blocking
errorOccurred — non-blocking
```

```toml
[hooks.<name>]
url   = "https://github.com/owner/repo"
path  = "hooks/subfolder"         # subdirectory within the repo
rev   = "v1.0.0"
scope = "local"                   # "local" | "global"
group = "dev"

[hooks.<name>.env]
SECRET_VAR = "$ENV_VAR_NAME"      # resolved at runtime, never written to disk
```

For local dev hooks: `{ path = "./hooks/my-hook" }` — no network, hash recorded at sync.

### [workflows] — agentic workflows

Agentic workflows are Markdown files with YAML frontmatter that describe automation in
natural language. They are compiled to GitHub Actions YAML (`.lock.yml`) by `gh aw compile`.
cpm installs only the `.md` source file. Pre-compiled `.lock.yml` files from upstream
repos must never be installed directly — that would bypass the compilation security step.

Supported engines: `"copilot"` (default) | `"claude"` | `"codex"`

```toml
[workflows.<name>]
url    = "https://github.com/owner/repo"
path   = "workflows/my-workflow.md"   # path to the .md file within the repo
rev    = "v1.0.0"
engine = "copilot"                    # overrides the engine declared in frontmatter
group  = "ci"
```

Scope is always `local`. The `engine` field in `cpm.toml` overrides whatever engine
is declared in the workflow's own frontmatter — useful for changing engine without
editing the upstream source.

---

## Lockfile — cpm.lock

Machine-generated. Always commit it. Never edit it. Regenerated deterministically by
`cpm lock` or `cpm sync`. Same inputs always produce the same output.

### Per-asset lock entry fields

```toml
[[<kind>]]                 # plugin | skill | agent | mcp | hook | workflow
name      = "..."
url       = "..."          # canonical URL, not mirror URL
rev       = "..."          # full 40-char git SHA — always, never a branch name
date      = "..."          # ISO 8601 UTC commit date of the pinned rev
resolved  = "..."          # ISO 8601 UTC when cpm resolved this entry
hash      = "sha256:..."   # content hash — see algorithm below
scope     = "local"        # "local" | "global"
group     = "default"
files     = [...]          # every path written to disk

# Hook-specific additions
executable = [...]         # subset of files[] that were chmod +x'd

# Workflow-specific additions
engine        = "copilot"  # engine that will run the compiled workflow
compiled_path = ""         # path to .lock.yml if auto_compile_workflows = true, else ""

# MCP-specific additions
transport = "..."          # transport type string
env_keys  = [...]          # names of env vars — values NEVER stored here
# For npx/uvx: version = "1.4.2" (exact pinned version replaces rev)
# For binary: arch = "x86_64-unknown-linux-gnu", bin_path = "~/.cache/cpm/bins/..."
# For docker: image_digest = "sha256:..." (registry manifest digest)

[<kind>.<name>.license]
spdx     = "MIT"           # SPDX expression detected from source repo
url      = "..."           # link to LICENSE file at pinned rev
verified = true            # true = matched known SPDX text, false = heuristic
```

### Hash algorithm

**Git-sourced assets (plugins, skills, agents, hooks, workflows, git-URL MCPs):**
1. Collect all installed file paths, sort lexicographically
2. SHA-256 each file's contents individually
3. Final hash = SHA-256 of all per-file hashes concatenated in sorted order

**npx MCPs:** SHA-256 of the unpacked npm tarball tree (all files, sorted by relative path).

**uvx MCPs:** SHA-256 of the installed package files (all files, sorted by path).

**Binary MCPs:** SHA-256 of the raw downloaded binary before `chmod +x`.

**Docker MCPs:** SHA-256 of the registry image digest string (not the pulled image).

**Path MCPs / local hooks / local workflows:** SHA-256 of the file(s) at install time.
Recomputed on every `cpm sync` — a changed hash is a drift warning, not a hard error.

---

## Rust type definitions — crates/cpm-types

```rust
pub enum AssetKind {
    Plugin, Skill, Agent, Mcp, Hook, Workflow,
}

pub enum Scope { Local, Global }

pub enum McpTransport {
    Http   { url: String },
    Sse    { url: String },
    Npx    { package: String, rev: String, args: Vec<String> },
    Uvx    { package: String, rev: String, args: Vec<String> },
    Docker { image: String, args: Vec<String> },
    Binary { url: String, bin: String, checksum: Option<String> },
    Path   { path: Utf8PathBuf },
    Script { command: String, args: Vec<String> },
}

pub enum EnvValue {
    Literal(String),            // safe to write to disk
    FromEnv(String),            // $ prefix — resolved at runtime, NEVER written to disk
}

pub struct AssetSource {
    pub url:       Option<String>,
    pub rev:       Option<String>,          // user-specified ref — resolver pins to SHA
    pub path:      Option<Utf8PathBuf>,
    pub scope:     Scope,
    pub group:     String,
    pub transport: Option<McpTransport>,    // MCP only
    pub env:       Vec<(String, EnvValue)>, // MCP and hook only
    pub args:      Vec<String>,
    pub engine:    Option<WorkflowEngine>,  // workflow only
}

pub enum WorkflowEngine { Copilot, Claude, Codex }

pub struct ResolvedAsset {
    pub name:          String,
    pub kind:          AssetKind,
    pub source:        AssetSource,
    pub resolved_rev:  String,              // always 40-char git SHA, or pinned npm/PyPI version
    pub resolved_date: DateTime<Utc>,       // commit date of resolved_rev
    pub resolved_at:   DateTime<Utc>,       // when cpm ran the resolution
    pub hash:          String,              // "sha256:<hex>"
    pub scope:         Scope,
    pub files:         Vec<Utf8PathBuf>,    // every path written to disk
    pub executable:    Vec<Utf8PathBuf>,    // hook scripts that received chmod +x
    pub bin_path:      Option<Utf8PathBuf>, // binary MCP only
    pub arch:          Option<String>,      // binary MCP only — target triple
    pub compiled_path: Option<Utf8PathBuf>, // workflow only — .lock.yml if compiled
    pub license:       AssetLicense,
}

pub struct AssetLicense {
    pub spdx:     String,    // SPDX expression or "UNKNOWN"
    pub url:      String,    // link to LICENSE file at pinned rev
    pub verified: bool,      // matched known SPDX text vs heuristic
}

pub struct Manifest {
    pub package:   Option<PackageMeta>,
    pub settings:  Settings,
    pub sources:   IndexMap<String, SourceConfig>,
    pub plugins:   IndexMap<String, AssetSource>,
    pub skills:    IndexMap<String, AssetSource>,
    pub agents:    IndexMap<String, AssetSource>,
    pub mcps:      IndexMap<String, AssetSource>,
    pub hooks:     IndexMap<String, AssetSource>,
    pub workflows: IndexMap<String, AssetSource>,
    pub groups:    IndexMap<String, GroupDefinition>,
}

pub struct Lockfile {
    pub version:   u8,                  // always 1 for this format
    pub generated: DateTime<Utc>,
    pub plugins:   Vec<ResolvedAsset>,
    pub skills:    Vec<ResolvedAsset>,
    pub agents:    Vec<ResolvedAsset>,
    pub mcps:      Vec<ResolvedAsset>,
    pub hooks:     Vec<ResolvedAsset>,
    pub workflows: Vec<ResolvedAsset>,
}
```

---

## Rust module responsibilities — crates/cpm-core

### `resolver`
- Fetch remote git refs via `gitoxide` — never shell out to `git`, never use `libgit2`
- Resolve any ref (tag, branch, short SHA) to a full 40-char SHA
- Cache resolved SHAs in `~/.cache/cpm/refs/<escaped-url>/<ref>` with a 5-minute TTL
- For npm packages: query `registry.npmjs.org` — resolve semver range to exact version
- For PyPI packages: query `pypi.org/pypi/<pkg>/json` — resolve PEP 440 specifier
- For GH release binaries: query GitHub Releases API, select asset by OS/arch triple
- Detect and return `Err(CpmError::ScopeConflict)` when same name+kind appears in both
  local and global scope — never silently resolve conflicts
- Detect and return `Err(CpmError::InvalidScope)` for workflow entries with global scope
- Token resolution order: `CPM_TOKEN` → `GITHUB_TOKEN` → system keyring → anonymous

### `fetcher`
- Git sources: sparse checkout only the files matching the asset's file pattern
  (*.md for skills/agents/workflows, hooks/ folder for hooks, *.yml/*.json for plugins)
- Cache blobs in `~/.cache/cpm/objects/<sha>/`
- npm packages: HTTP download of registry tarball, verify against registry `integrity`
- PyPI packages: HTTP download of wheel/sdist, verify against `requires-dist` hash
- GH release binaries: stream download to `~/.cache/cpm/bins/<name>-<ver>-<arch>`,
  `chmod +x` on Unix, verify SHA-256 against `checksum` field if provided
- Docker: do NOT pull at install time — record image reference and registry digest only
- All HTTP via `reqwest` async with `tokio` — no blocking calls on async executors
- Respect `CPM_CACHE_DIR` env var

### `installer`
- Atomic writes: write to `<target>.tmp` then `rename()` — no partial installs
- For hook bundles: install entire folder, chmod +x every `.sh` file, record the list
  in `ResolvedAsset.executable`
- For workflows: install only the `.md` source file; if `settings.auto_compile_workflows`
  is true, shell out to `gh aw compile <path>` and record the output path in
  `ResolvedAsset.compiled_path`; hard error if `gh` or `gh-aw` extension is not found
- For binary MCPs: never copy binary to `.github/` or `~/.copilot/` — binary stays in
  `~/.cache/cpm/bins/`; write only a `.mcp.json` config pointing at it
- For path MCPs: validate that scope is not `Global` — return `Err(CpmError::InvalidScope)`
- `env` values with `EnvValue::FromEnv` are NEVER written to any `.mcp.json` or config
  file on disk — record only the key names in the lockfile's `env_keys` field
- Create parent directories if they don't exist

### `doctor`
- Walk every `files` entry in cpm.lock, recompute its hash, compare against stored hash
- Walk every `executable` entry, verify the file exists and is executable
- For binary MCPs: verify `bin_path` exists and its hash matches the lock entry
- Return `Err(CpmError::HashMismatch)` on first mismatch — or collect all mismatches
  and surface as a structured report, depending on the `--fail-fast` flag
- Exit 0 only if all entries pass

### `status`
- Compare resolved state of cpm.toml against cpm.lock against physical disk
- Three categories: `Clean`, `Drift` (disk differs from lock), `Stale` (lock differs from toml)
- For workflows: also report whether `.lock.yml` is missing (compiled_path set but file absent)

### `auth`
- Store and retrieve tokens via the OS system keyring (secretservice on Linux, Keychain on macOS)
- `cpm auth login` prompts for token, stores in keyring — never in cpm.toml or cpm.lock
- `cpm auth logout` removes from keyring
- `cpm auth status` shows which sources have stored credentials

---

## Error types — crates/cpm-types

```rust
#[derive(Debug, thiserror::Error)]
pub enum CpmError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("parse error in {file}: {msg}")]
    Parse { file: String, msg: String },

    #[error("hash mismatch for '{name}': expected {expected}, got {actual}")]
    HashMismatch { name: String, expected: String, actual: String },

    #[error("scope conflict: '{name}' ({kind:?}) exists in both local and global scope")]
    ScopeConflict { name: String, kind: AssetKind },

    #[error("invalid scope: {kind:?} assets cannot use global scope")]
    InvalidScope { kind: AssetKind },

    #[error("lock is out of date — run `cpm sync`")]
    LockOutOfDate,

    #[error("unsupported URL: {url}")]
    UnsupportedUrl { url: String },

    #[error("workflow compilation failed: {msg} (is `gh aw` installed?)")]
    WorkflowCompileFailed { msg: String },

    #[error("hook missing executable: {path}")]
    HookNotExecutable { path: String },

    #[error("pre-installed asset blocked by preToolUse hook '{hook}': {reason}")]
    HookDenied { hook: String, reason: String },

    #[error("auth required for {url} — run `cpm auth login`")]
    AuthRequired { url: String },

    #[error("license denied: '{name}' has license '{spdx}' (policy: {policy})")]
    LicenseDenied { name: String, spdx: String, policy: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## CLI surface — crates/cpm-cli

```sh
# ── Add ──────────────────────────────────────────────────────────────────────
cpm add <url>                                  # infer kind from content
cpm add <url> --plugin / --skill / --agent
cpm add <url> --mcp / --hook / --workflow
cpm add <url> --mcp --scope global
cpm add <url> --skill --group research --rev v2.0.0
cpm add <url> --mcp --npx                      # shorthand: infer npx transport
cpm add <url> --mcp --docker
cpm add <url> --mcp --release --bin my-server
cpm add ./path --mcp --path
cpm add ./path --hook                          # local dev hook folder

# ── Sync / install ───────────────────────────────────────────────────────────
cpm sync                                       # install everything per lock + settings
cpm sync --group research                      # also install named group
cpm sync --scope global                        # only global-scoped entries
cpm sync --frozen                              # fail if lock would change (CI)
cpm sync --compile-workflows                   # also run gh aw compile on all workflows

# ── Remove ───────────────────────────────────────────────────────────────────
cpm remove <name> --skill
cpm remove <name> --hook --scope global

# ── Scope promotion / demotion (atomic) ──────────────────────────────────────
cpm promote <name> --mcp                       # local → global, removes local
cpm demote  <name> --mcp                       # global → local, removes global
cpm scope default local                        # set repo-level default scope

# ── Update ───────────────────────────────────────────────────────────────────
cpm update                                     # update all to latest SHA
cpm update <name>                              # update one asset
cpm update --dry-run

# ── Lock ─────────────────────────────────────────────────────────────────────
cpm lock                                       # resolve without installing
cpm lock --check                               # exit 1 if lock is stale (CI)

# ── Inspect ──────────────────────────────────────────────────────────────────
cpm list
cpm list --hook --scope local
cpm list --workflow
cpm show <name>                                # full entry: url, rev, date, hash, files
cpm tree                                       # grouped asset tree

# ── Health ───────────────────────────────────────────────────────────────────
cpm doctor                                     # verify all file hashes match lock
cpm doctor --fail-fast                         # stop at first mismatch
cpm status                                     # drift: toml vs lock vs disk

# ── Workflow compilation ──────────────────────────────────────────────────────
cpm compile                                    # compile all installed workflow .md files
cpm compile <name>                             # compile one workflow

# ── Cache ────────────────────────────────────────────────────────────────────
cpm cache dir
cpm cache clean
cpm cache prune                                # remove entries not in any current lock

# ── Auth ─────────────────────────────────────────────────────────────────────
cpm auth login
cpm auth logout
cpm auth status

# ── Run without installing ───────────────────────────────────────────────────
cpm run <url> --agent                          # fetch, verify, run ephemerally
cpm run <url> --mcp
```

---

## Scope conflict rules

An asset with the same `name` and same `kind` cannot exist in both `local` and `global`
scope simultaneously. cpm detects this at resolve time:

```
error: scope conflict — 'github' (mcp) exists in both local (.github/mcp/) and global
  (~/.copilot/mcp/).

  hint: run `cpm demote github --mcp` to move to local only,
        or  `cpm promote github --mcp` to move to global only.
```

`cpm promote` and `cpm demote` are atomic: they install in the target scope and remove
from the source scope in a single filesystem transaction (write tmp → rename → delete old).

The dev workflow for building a new MCP or hook:
```sh
cpm add ./mcp/my-new-mcp --mcp --scope local   # iterate safely, isolated
cpm sync                                        # re-register hash after each cargo build
cpm doctor                                      # verify binary matches lock
cpm promote my-new-mcp --mcp                   # ship to global when ready
```

---

## Testing conventions

- Unit tests: `#[cfg(test)]` mod in the same file as the implementation
- Integration tests: `tests/` at workspace root — use `tempfile::TempDir` for all FS work
- Network tests: `#[cfg(feature = "network-tests")]` — never run in default `cargo test`
- Mock HTTP with `wiremock`; mock the GitHub API, npm registry, PyPI, GH Releases
- Never touch `~/.copilot/`, `~/.cache/cpm/`, or the real system keyring in tests
- Hook tests: create a tempdir with a fake `hooks.json` + `.sh` file, verify chmod +x
- Workflow tests: mock `gh aw compile` via a fake executable in PATH; assert `.lock.yml`
  is created at the expected path; assert `.md` source is never deleted
- Scope conflict tests: set up two entries with the same name+kind in different scopes,
  assert `Err(CpmError::ScopeConflict { .. })`
- env secret tests: set `[mcps.x.env] SECRET = "$MY_VAR"`, assert `MY_VAR` value does
  not appear anywhere in the installed `.mcp.json` or in `cpm.lock`

---

## Code conventions

- `rustfmt` defaults — no custom `rustfmt.toml`
- `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` in all lib crates
- Never use `.unwrap()` or `.expect()` outside `#[cfg(test)]` blocks
- All public functions return `Result<T, CpmError>`
- `tracing` for all diagnostics — never `println!` or `eprintln!`
- `indicatif` for CLI progress: operation spinners plus streamed download bars when rich output is enabled (`CPM_PROGRESS=rich` can force it, `CPM_PROGRESS=plain` disables it)
- `miette` for CLI error rendering — colored, with source context and `help:` text
- `camino::Utf8PathBuf` for all user-facing paths
- `tokio` async runtime — no blocking calls inside async contexts
- `chrono::DateTime<Utc>` for all timestamps — never `std::time::SystemTime` directly
- `gitoxide` for all git operations — never shell out to `git`, never use `libgit2`
- `reqwest` async for all HTTP — never the blocking client
- `indexmap::IndexMap` to preserve declaration order from cpm.toml in output
- All public items must have doc comments

---

## What Copilot must never do

- Add any bundled catalog, hardcoded asset list, known-URL table, or default registry
- Shell out to `git` — use `gitoxide`
- Use `reqwest` blocking API
- Write `EnvValue::FromEnv` values to any file on disk or to `cpm.lock`
- Allow `scope = "global"` on workflow or path-transport MCP entries — always error
- Pull Docker images at `cpm sync` time — record image ref only, pull at launch
- Install a pre-compiled `.lock.yml` from an upstream repo — install `.md` source only
- Write `cpm.lock` unless a resolution actually ran
- Silently resolve scope conflicts — always return `CpmError::ScopeConflict`
- Silently ignore hash mismatches — always error and prompt `cpm doctor`
- Mix sync and async code in the same call path
- Use `.unwrap()` or `.expect()` in library code
