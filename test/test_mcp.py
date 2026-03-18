"""Tests for MCP management: config parsing, sync idempotency, add/remove,
local MCP discovery, and invalid/missing config handling."""

from __future__ import annotations

import json
from pathlib import Path
from typing import cast

import pytest

from copilot_plugin_manager.catalog import load_catalog_bundle
from copilot_plugin_manager.manager import PluginManager
from copilot_plugin_manager.models import McpRecord
from copilot_plugin_manager.paths import ManagerPaths
from copilot_plugin_manager.runner import CommandResult, ShellRunner

# ─── Helpers ─────────────────────────────────────────────────────────────────


class FakeRunner(ShellRunner):
    """Runner that records calls and returns deterministic output."""

    def __init__(self, npm_versions: dict[str, str] | None = None) -> None:
        self.calls: list[tuple[str, ...]] = []
        self.npm_versions = npm_versions or {}

    def require(self, name: str) -> None:
        return None

    def which(self, name: str) -> str | None:
        if name == "npm":
            return "/usr/bin/npm"
        return None

    def run(
        self,
        args: list[str],
        cwd: Path | None = None,
        check: bool = True,
    ) -> CommandResult:
        self.calls.append(tuple(args))
        if args[:2] == ["npm", "view"] and len(args) >= 3 and args[2] in self.npm_versions:
            return CommandResult(tuple(args), self.npm_versions[args[2]], "", 0)
        if args[:2] == ["npm", "view"]:
            return CommandResult(tuple(args), "", "", 1)
        return CommandResult(tuple(args), "", "", 0)


class NoNpmRunner(FakeRunner):
    """Runner that simulates npm being absent."""

    def which(self, name: str) -> str | None:
        return None


def _make_manager(tmp_path: Path, runner: ShellRunner | None = None) -> PluginManager:
    paths = ManagerPaths(
        tmp_path / ".copilot",
        tmp_path / ".copilot" / "copilot-plugin-manager",
        tmp_path / ".copilot" / "skills",
        tmp_path / ".copilot" / "agents",
        tmp_path / ".copilot" / "active-profile",
        tmp_path / ".copilot" / "copilot-plugin-manager" / "state.json",
        tmp_path / ".copilot" / "copilot-plugin-manager" / "sources",
        tmp_path / ".copilot" / "mcp-config.json",
    )
    return PluginManager(load_catalog_bundle(), paths, runner=runner or FakeRunner())


def _get_servers(manager: PluginManager) -> dict[str, object]:
    """Return the ``servers`` dict from the MCP config, typed as dict[str, object]."""
    config = manager.read_mcp_config()
    raw = config.get("servers", {})
    assert isinstance(raw, dict)
    return cast(dict[str, object], raw)


def _get_entry(servers: dict[str, object], name: str) -> dict[str, object]:
    """Return a single server entry, typed as dict[str, object]."""
    raw = servers[name]
    assert isinstance(raw, dict)
    return cast(dict[str, object], raw)


# ─── Catalog loading ──────────────────────────────────────────────────────────


def test_catalog_loads_default_mcps() -> None:
    bundle = load_catalog_bundle()
    assert len(bundle.mcps) >= 8
    required = {
        "zen-of-docs",
        "zen-of-languages",
        "context7",
        "ai-agent-guidelines",
        "playwright",
        "magic",
        "serena",
        "chrome-devtools",
    }
    assert required.issubset(set(bundle.mcps))


def test_catalog_mcp_kinds() -> None:
    bundle = load_catalog_bundle()
    assert bundle.mcps["context7"].kind == "http"
    assert bundle.mcps["context7"].url is not None
    assert bundle.mcps["zen-of-docs"].kind == "pip"
    assert bundle.mcps["zen-of-languages"].kind == "pip"
    assert bundle.mcps["serena"].kind == "pip"
    for name, record in bundle.mcps.items():
        if name != "context7":
            if record.kind == "npm":
                assert record.package is not None, f"npm MCP '{name}' missing package"


def test_catalog_mcp_details() -> None:
    bundle = load_catalog_bundle()
    details = bundle.mcp_details("playwright")
    assert details["kind"] == "npm"
    assert "@playwright/mcp" in details["identifier"]
    assert details["description"]
    assert details["use_when"]


# ─── read_mcp_config / write_mcp_config ──────────────────────────────────────


def test_read_mcp_config_returns_empty_when_missing(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    assert manager.read_mcp_config() == {}


def test_read_mcp_config_returns_empty_on_invalid_json(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    config_path = manager._mcp_config_path()
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text("{ invalid json }")
    assert manager.read_mcp_config() == {}


def test_read_mcp_config_returns_empty_when_not_a_dict(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    config_path = manager._mcp_config_path()
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text("[1, 2, 3]")
    assert manager.read_mcp_config() == {}


def test_write_and_read_mcp_config_roundtrip(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    original: dict[str, object] = {"servers": {"my-mcp": {"type": "http", "url": "https://example.com/mcp"}}}
    manager.write_mcp_config(original)
    assert manager.read_mcp_config() == original


# ─── build_mcp_server_entry ──────────────────────────────────────────────────


def test_build_mcp_server_entry_http(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    entry = manager.build_mcp_server_entry("context7", record)
    assert entry == {"type": "http", "url": "https://mcp.context7.com/mcp"}


def test_build_mcp_server_entry_http_raises_when_no_url(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http")
    with pytest.raises(ValueError, match="no url is set"):
        manager.build_mcp_server_entry("missing-url", record)


def test_build_mcp_server_entry_npm_with_pinned_tag(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="@playwright/mcp", pinned_tag="1.2.3")
    entry = manager.build_mcp_server_entry("playwright", record)
    assert entry["type"] == "stdio"
    assert entry["command"] == "npx"
    args = cast(list[object], entry["args"])
    assert "@playwright/mcp@1.2.3" in args


def test_build_mcp_server_entry_npm_with_probed_version(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="@playwright/mcp")
    entry = manager.build_mcp_server_entry("playwright", record, installed_version="2.0.0")
    args = cast(list[object], entry["args"])
    assert "@playwright/mcp@2.0.0" in args


def test_build_mcp_server_entry_npm_latest_when_no_version(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="@playwright/mcp")
    entry = manager.build_mcp_server_entry("playwright", record)
    args = cast(list[object], entry["args"])
    assert "@playwright/mcp" in args


def test_build_mcp_server_entry_npm_custom_env(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="my-pkg", env={"API_KEY": "secret"})
    entry = manager.build_mcp_server_entry("my-pkg", record)
    assert entry.get("env") == {"API_KEY": "secret"}


# ─── probe_mcp_npm_version ────────────────────────────────────────────────────


def test_probe_mcp_npm_version_returns_version_when_npm_available(tmp_path: Path) -> None:
    runner = FakeRunner(npm_versions={"@playwright/mcp": "1.5.0\n"})
    manager = _make_manager(tmp_path, runner=runner)
    version = manager.probe_mcp_npm_version("@playwright/mcp")
    assert version == "1.5.0"


def test_probe_mcp_npm_version_returns_none_when_npm_absent(tmp_path: Path) -> None:
    runner = NoNpmRunner()
    manager = _make_manager(tmp_path, runner=runner)
    assert manager.probe_mcp_npm_version("@playwright/mcp") is None


def test_probe_mcp_npm_version_returns_none_on_unknown_package(tmp_path: Path) -> None:
    runner = FakeRunner(npm_versions={})
    manager = _make_manager(tmp_path, runner=runner)
    assert manager.probe_mcp_npm_version("@unknown/does-not-exist") is None


# ─── sync_mcp ────────────────────────────────────────────────────────────────


def test_sync_mcp_writes_http_entry(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp", source_url="https://github.com/upstash/context7")
    state = manager.sync_mcp("context7", record, probe_version=False)

    servers = _get_servers(manager)
    assert "context7" in servers
    entry = _get_entry(servers, "context7")
    assert entry["type"] == "http"
    assert entry["url"] == "https://mcp.context7.com/mcp"
    assert state.kind == "http"
    assert state.url == "https://mcp.context7.com/mcp"


def test_sync_mcp_writes_npm_entry_with_probed_version(tmp_path: Path) -> None:
    runner = FakeRunner(npm_versions={"@playwright/mcp": "2.1.0"})
    manager = _make_manager(tmp_path, runner=runner)
    record = McpRecord(kind="npm", package="@playwright/mcp")
    state = manager.sync_mcp("playwright", record, probe_version=True)

    servers = _get_servers(manager)
    entry = _get_entry(servers, "playwright")
    assert "@playwright/mcp@2.1.0" in cast(list[object], entry["args"])
    assert state.installed_version == "2.1.0"


def test_sync_mcp_uses_pinned_tag_over_probe(tmp_path: Path) -> None:
    runner = FakeRunner(npm_versions={"@playwright/mcp": "3.0.0"})
    manager = _make_manager(tmp_path, runner=runner)
    record = McpRecord(kind="npm", package="@playwright/mcp", pinned_tag="1.0.0")
    state = manager.sync_mcp("playwright", record, probe_version=True)

    servers = _get_servers(manager)
    entry = _get_entry(servers, "playwright")
    assert "@playwright/mcp@1.0.0" in cast(list[object], entry["args"])
    assert state.installed_version == "1.0.0"


def test_sync_mcp_falls_back_to_pinned_sha(tmp_path: Path) -> None:
    runner = NoNpmRunner()
    manager = _make_manager(tmp_path, runner=runner)
    record = McpRecord(kind="npm", package="@new/mcp", pinned_sha="abc123def456")
    state = manager.sync_mcp("new-mcp", record, probe_version=True)

    assert state.installed_sha == "abc123def456"
    servers = _get_servers(manager)
    entry = _get_entry(servers, "new-mcp")
    assert "@new/mcp" in cast(list[object], entry["args"])


def test_sync_mcp_persists_state(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False)

    stored = manager.state_store.read_mcp_state("context7")
    assert stored is not None
    assert stored.kind == "http"
    assert stored.url == "https://mcp.context7.com/mcp"
    assert stored.config_signature is not None
    assert stored.updated_at is not None


def test_sync_mcp_preserves_existing_unrelated_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    existing: dict[str, object] = {"servers": {"existing-mcp": {"type": "http", "url": "https://other.example"}}}
    manager.write_mcp_config(existing)

    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False)

    servers = manager.read_mcp_config()["servers"]
    assert isinstance(servers, dict)
    assert "existing-mcp" in servers
    assert "context7" in servers


# ─── remove_mcp ──────────────────────────────────────────────────────────────


def test_remove_mcp_removes_existing_entry(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False)

    removed = manager.remove_mcp("context7")

    assert removed is True
    servers = _get_servers(manager)
    assert "context7" not in servers
    assert manager.state_store.read_mcp_state("context7") is None


def test_remove_mcp_returns_false_when_not_present(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    removed = manager.remove_mcp("nonexistent")
    assert removed is False


def test_remove_mcp_preserves_other_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    manager.write_mcp_config(
        {
            "servers": {
                "a": {"type": "http", "url": "https://a.example"},
                "b": {"type": "http", "url": "https://b.example"},
            }
        }
    )
    manager.remove_mcp("a")

    servers = _get_servers(manager)
    assert "a" not in servers
    assert "b" in servers


# ─── reconcile_mcps ──────────────────────────────────────────────────────────


def test_reconcile_mcps_adds_all_catalog_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    results = manager.reconcile_mcps(tmp_path, probe_version=False)

    servers = _get_servers(manager)
    for name in manager.catalog.mcps:
        assert name in servers, f"Expected {name!r} in servers"
    assert all(action in {"added", "updated", "skipped"} for action in results.values())


def test_reconcile_mcps_is_idempotent(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.reconcile_mcps(tmp_path, probe_version=False)
    first_config = manager.read_mcp_config()

    results2 = manager.reconcile_mcps(tmp_path, probe_version=False)
    second_config = manager.read_mcp_config()

    assert first_config == second_config
    assert all(action == "skipped" for action in results2.values())


def test_reconcile_mcps_updates_stale_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.reconcile_mcps(tmp_path, probe_version=False)

    # Tamper with a config entry to simulate staleness.
    config = manager.read_mcp_config()
    servers = _get_servers(manager)
    servers["playwright"] = {"type": "stdio", "command": "npx", "args": ["-y", "@playwright/mcp@OLD"]}
    config["servers"] = servers
    manager.write_mcp_config(config)
    manager.state_store.clear_mcp_state("playwright")

    results2 = manager.reconcile_mcps(tmp_path, probe_version=False)
    assert results2["playwright"] in {"added", "updated"}


def test_reconcile_mcps_remove_unlisted(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.write_mcp_config(
        {
            "servers": {
                "should-be-removed": {"type": "http", "url": "https://gone.example"},
            }
        }
    )

    manager.reconcile_mcps(tmp_path, probe_version=False, remove_unlisted=True)

    servers = _get_servers(manager)
    assert "should-be-removed" not in servers


def test_reconcile_mcps_keep_unlisted_by_default(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.write_mcp_config(
        {
            "servers": {
                "keep-me": {"type": "http", "url": "https://keep.example"},
            }
        }
    )

    manager.reconcile_mcps(tmp_path, probe_version=False, remove_unlisted=False)

    servers = _get_servers(manager)
    assert "keep-me" in servers


# ─── discover_local_mcps ──────────────────────────────────────────────────────


def test_discover_local_mcps_from_vscode_mcp_json(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    vscode_dir = tmp_path / ".vscode"
    vscode_dir.mkdir()
    mcp_json = vscode_dir / "mcp.json"
    mcp_json.write_text(
        json.dumps(
            {
                "servers": {
                    "local-tool": {
                        "type": "stdio",
                        "command": "node",
                        "args": ["./my-mcp/index.js"],
                    }
                }
            }
        )
    )

    local = manager.discover_local_mcps(tmp_path)

    assert "local-tool" in local
    local_tool = cast(dict[str, object], local["local-tool"])
    assert local_tool["command"] == "node"


def test_discover_local_mcps_accepts_mcpservers_key(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    vscode_dir = tmp_path / ".vscode"
    vscode_dir.mkdir()
    mcp_json = vscode_dir / "mcp.json"
    mcp_json.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "alt-tool": {"type": "http", "url": "https://alt.example"},
                }
            }
        )
    )

    local = manager.discover_local_mcps(tmp_path)
    assert "alt-tool" in local


def test_discover_local_mcps_returns_empty_when_no_file(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    assert manager.discover_local_mcps(tmp_path) == {}


def test_discover_local_mcps_returns_empty_on_invalid_json(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    vscode_dir = tmp_path / ".vscode"
    vscode_dir.mkdir()
    (vscode_dir / "mcp.json").write_text("not json at all")
    assert manager.discover_local_mcps(tmp_path) == {}


def test_reconcile_mcps_merges_local_definitions(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    vscode_dir = tmp_path / ".vscode"
    vscode_dir.mkdir()
    (vscode_dir / "mcp.json").write_text(json.dumps({"servers": {"repo-local": {"type": "http", "url": "https://local.example"}}}))

    manager.reconcile_mcps(tmp_path, probe_version=False)

    servers = _get_servers(manager)
    assert "repo-local" in servers


# ─── manage_mcps ─────────────────────────────────────────────────────────────


def test_manage_mcps_install_reconciles(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    results = manager.manage_mcps("install", tmp_path)
    assert isinstance(results, dict)
    assert len(results) == len(manager.catalog.mcps)


def test_manage_mcps_delete_removes_catalog_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.manage_mcps("install", tmp_path)
    results = manager.manage_mcps("delete", tmp_path)

    servers = _get_servers(manager)
    for name in manager.catalog.mcps:
        assert name not in servers
        assert results.get(name) in {"removed", "skipped"}


def test_manage_mcps_raises_on_unknown_operation(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    with pytest.raises(ValueError, match="Unknown MCP operation"):
        manager.manage_mcps("explode", tmp_path)


# ─── manage_target integration ───────────────────────────────────────────────


def test_manage_target_mcps_dispatches_correctly(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    manager.manage_target("install", "mcps", tmp_path)

    servers = _get_servers(manager)
    assert len(servers) >= len(manager.catalog.mcps)


# ─── state persistence ───────────────────────────────────────────────────────


def test_mcp_state_persists_and_round_trips(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="@playwright/mcp", pinned_tag="1.0.0")
    state = manager.sync_mcp("playwright", record, probe_version=False)

    reloaded = manager.state_store.read_mcp_state("playwright")
    assert reloaded is not None
    assert reloaded.name == "playwright"
    assert reloaded.installed_version == "1.0.0"
    assert reloaded.config_signature == state.config_signature


def test_mcp_state_clear_removes_entry(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False)

    manager.state_store.clear_mcp_state("context7")
    assert manager.state_store.read_mcp_state("context7") is None


# ─── pip kind ────────────────────────────────────────────────────────────────


def test_build_mcp_server_entry_pip_no_version(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="pip", package="markitdown-mcp")
    entry = manager.build_mcp_server_entry("markitdown", record)
    assert entry["type"] == "stdio"
    assert entry["command"] == "uvx"
    args = cast(list[object], entry["args"])
    assert "markitdown-mcp" in args


def test_build_mcp_server_entry_pip_custom_uvx_args(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(
        kind="pip",
        package="mcp-zen-of-languages",
        command="uvx",
        args=["--from", "mcp-zen-of-languages", "zen-mcp-server"],
    )
    entry = manager.build_mcp_server_entry("zen-of-languages", record, installed_version="9.9.9")

    assert entry["command"] == "uvx"
    assert cast(list[object], entry["args"]) == ["--from", "mcp-zen-of-languages", "zen-mcp-server"]


def test_build_mcp_server_entry_pip_with_pinned_tag(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="pip", package="markitdown-mcp", pinned_tag="0.1.2")
    entry = manager.build_mcp_server_entry("markitdown", record)
    args = cast(list[object], entry["args"])
    assert "markitdown-mcp==0.1.2" in args


def test_build_mcp_server_entry_pip_with_probed_version(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="pip", package="markitdown-mcp")
    entry = manager.build_mcp_server_entry("markitdown", record, installed_version="0.2.0")
    args = cast(list[object], entry["args"])
    assert "markitdown-mcp==0.2.0" in args


def test_probe_mcp_pip_version_parses_available_versions_line(tmp_path: Path) -> None:
    """Regression: probe_mcp_pip_version must parse 'Available versions: X, Y, ...' correctly."""

    class PipRunner(FakeRunner):
        def which(self, name: str) -> str | None:
            return "/usr/bin/pip" if name in {"pip", "pip3"} else None

        def run(self, args: list[str], cwd: Path | None = None, check: bool = True) -> CommandResult:
            if args[:1] == ["/usr/bin/pip"] and "index" in args and "versions" in args:
                # Simulate actual pip output format.
                stdout = "markitdown-mcp (0.1.0)\nAvailable versions: 0.1.0, 0.0.9, 0.0.8\n  INSTALLED: 0.0.9\n  LATEST:    0.1.0\n"
                return CommandResult(tuple(args), stdout, "", 0)
            return CommandResult(tuple(args), "", "", 0)

    manager = _make_manager(tmp_path, runner=PipRunner())
    result = manager.probe_mcp_pip_version("markitdown-mcp")
    assert result == "0.1.0"


def test_probe_mcp_pip_version_returns_none_when_pip_absent(tmp_path: Path) -> None:
    runner = NoNpmRunner()
    manager = _make_manager(tmp_path, runner=runner)
    assert manager.probe_mcp_pip_version("markitdown-mcp") is None


def test_sync_pip_mcp_writes_uvx_entry(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    record = McpRecord(kind="pip", package="markitdown-mcp", pinned_tag="0.1.0")
    state = manager.sync_mcp("markitdown", record, probe_version=False)

    servers = _get_servers(manager)
    entry = _get_entry(servers, "markitdown")
    assert entry["command"] == "uvx"
    assert "markitdown-mcp==0.1.0" in cast(list[object], entry["args"])
    assert state.installed_version == "0.1.0"


# ─── catalog: azure + markitdown ─────────────────────────────────────────────


def test_catalog_contains_azure_mcp() -> None:
    bundle = load_catalog_bundle()
    assert "azure" in bundle.mcps
    record = bundle.mcps["azure"]
    assert record.kind == "npm"
    assert record.package == "@azure/mcp"
    assert "server" in record.args
    assert "start" in record.args


def test_catalog_contains_markitdown_mcp() -> None:
    bundle = load_catalog_bundle()
    assert "markitdown" in bundle.mcps
    record = bundle.mcps["markitdown"]
    assert record.kind == "pip"
    assert record.package == "markitdown-mcp"


def test_catalog_contains_uvx_mcps() -> None:
    bundle = load_catalog_bundle()

    zen_docs = bundle.mcps["zen-of-docs"]
    zen_languages = bundle.mcps["zen-of-languages"]
    serena = bundle.mcps["serena"]

    assert zen_docs.command == "uvx"
    assert zen_docs.args == ["--from", "mcp-zen-of-docs", "mcp-zen-of-docs-server"]
    assert zen_languages.command == "uvx"
    assert zen_languages.args == ["--from", "mcp-zen-of-languages", "zen-mcp-server"]
    assert serena.command == "uvx"
    assert serena.args == ["--from", "git+https://github.com/oraios/serena", "serena", "start-mcp-server"]


def test_azure_mcp_entry_includes_server_start_args(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    record = manager.catalog.mcps["azure"]
    entry = manager.build_mcp_server_entry("azure", record)
    args = cast(list[object], entry["args"])
    assert "server" in args
    assert "start" in args


# ─── scope: global vs local ──────────────────────────────────────────────────


def test_sync_mcp_defaults_to_global_scope(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    state = manager.sync_mcp("context7", record, probe_version=False)

    assert state.scope == "global"
    servers = _get_servers(manager)
    assert "context7" in servers
    # Should NOT appear in local config (no .vscode/mcp.json written).
    assert not (tmp_path / ".vscode" / "mcp.json").exists()


def test_sync_mcp_local_scope_writes_vscode_mcp_json(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    state = manager.sync_mcp("context7", record, probe_version=False, scope="local", cwd=tmp_path)

    assert state.scope == "local"
    # Should NOT be in global config.
    assert "context7" not in _get_servers(manager)
    # Should be in local config.
    local_config = manager.read_local_mcp_config(tmp_path)
    local_servers = cast(dict[str, object], local_config.get("servers", {}))
    assert "context7" in local_servers


def test_sync_mcp_local_scope_requires_cwd(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    with pytest.raises(ValueError, match="cwd must be provided"):
        manager.sync_mcp("context7", record, probe_version=False, scope="local")


def test_sync_mcp_uses_repo_config_default_scope(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    (tmp_path / ".git").mkdir()
    manager.write_repo_config(tmp_path, mcp_scope="local")

    state = manager.sync_mcp("context7", record, probe_version=False, cwd=tmp_path)

    assert state.scope == "local"
    assert "context7" not in _get_servers(manager)
    local_config = manager.read_local_mcp_config(tmp_path)
    local_servers = cast(dict[str, object], local_config.get("servers", {}))
    assert "context7" in local_servers


# ─── move_mcp_to_scope ───────────────────────────────────────────────────────


def test_move_global_to_local(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False, scope="global")

    # Confirm it's in global config.
    assert "context7" in _get_servers(manager)

    # Move to local.
    state = manager.move_mcp_to_scope("context7", "local", tmp_path)

    assert state.scope == "local"
    # Removed from global.
    assert "context7" not in _get_servers(manager)
    # Present in local.
    local_config = manager.read_local_mcp_config(tmp_path)
    local_servers = cast(dict[str, object], local_config.get("servers", {}))
    assert "context7" in local_servers


def test_move_local_to_global(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False, scope="local", cwd=tmp_path)

    # Confirm it's in local config only.
    assert "context7" not in _get_servers(manager)

    # Move back to global.
    state = manager.move_mcp_to_scope("context7", "global", tmp_path)

    assert state.scope == "global"
    # Present in global.
    assert "context7" in _get_servers(manager)
    # Removed from local.
    local_config = manager.read_local_mcp_config(tmp_path)
    local_servers = cast(dict[str, object], local_config.get("servers", {}))
    assert "context7" not in local_servers


def test_move_global_to_local_raises_when_not_in_global(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    with pytest.raises(KeyError, match="not found in global config"):
        manager.move_mcp_to_scope("nonexistent", "local", tmp_path)


def test_move_local_to_global_raises_when_not_in_local(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    with pytest.raises(KeyError, match="not found in local config"):
        manager.move_mcp_to_scope("nonexistent", "global", tmp_path)


def test_reconcile_skips_locally_scoped_entries(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    # Add context7 as local; state.scope == "local".
    manager.sync_mcp("context7", record, probe_version=False, scope="local", cwd=tmp_path)

    # Reconcile global should skip context7 because it's locally scoped.
    results = manager.reconcile_mcps(tmp_path, probe_version=False)

    assert results.get("context7") == "skipped"
    # Should NOT appear in global config.
    assert "context7" not in _get_servers(manager)


def test_reconcile_mcps_uses_repo_config_local_scope(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    (tmp_path / ".git").mkdir()
    manager.write_repo_config(tmp_path, mcp_scope="local")

    results = manager.reconcile_mcps(tmp_path, probe_version=False)

    assert results["context7"] in {"added", "updated", "skipped"}
    assert "context7" not in _get_servers(manager)
    local_config = manager.read_local_mcp_config(tmp_path)
    local_servers = cast(dict[str, object], local_config.get("servers", {}))
    assert "context7" in local_servers


def test_scope_persists_in_state(tmp_path: Path) -> None:
    manager = _make_manager(tmp_path)
    record = McpRecord(kind="npm", package="@playwright/mcp", pinned_tag="1.0.0")
    manager.sync_mcp("playwright", record, probe_version=False, scope="local", cwd=tmp_path)

    stored = manager.state_store.read_mcp_state("playwright")
    assert stored is not None
    assert stored.scope == "local"

    # Move back to global; scope in state should update.
    manager.move_mcp_to_scope("playwright", "global", tmp_path)
    stored2 = manager.state_store.read_mcp_state("playwright")
    assert stored2 is not None
    assert stored2.scope == "global"


# ─── local scope repo-awareness ──────────────────────────────────────────────


def test_reconcile_adds_globally_when_local_config_deleted(tmp_path: Path) -> None:
    """After a local .vscode/mcp.json is deleted, mcp-sync should re-add globally."""
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    # Sync context7 as local scope.
    manager.sync_mcp("context7", record, probe_version=False, scope="local", cwd=tmp_path)

    # Simulate the local config being deleted.
    local_cfg = tmp_path / ".vscode" / "mcp.json"
    local_cfg.unlink()

    # reconcile_mcps should see scope==local but no local config → add globally.
    results = manager.reconcile_mcps(tmp_path, probe_version=False)
    assert results.get("context7") in {"added", "updated"}
    assert "context7" in _get_servers(manager)


def test_reconcile_adds_globally_when_entry_missing_from_local_config(tmp_path: Path) -> None:
    """If a locally-scoped MCP is removed from .vscode/mcp.json, re-add globally."""
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    # First sync as local.
    record = McpRecord(kind="http", url="https://mcp.context7.com/mcp")
    manager.sync_mcp("context7", record, probe_version=False, scope="local", cwd=tmp_path)

    # Wipe only context7 from the local config.
    manager.write_local_mcp_config(tmp_path, {"servers": {}})

    # reconcile_mcps should detect the missing entry and add it to global.
    results = manager.reconcile_mcps(tmp_path, probe_version=False)
    assert results.get("context7") in {"added", "updated"}
    assert "context7" in _get_servers(manager)


# ─── reconcile: local env not copied to global ───────────────────────────────


def test_reconcile_strips_env_from_local_entries_in_global(tmp_path: Path) -> None:
    """Env vars from .vscode/mcp.json must not be copied into the global config."""
    manager = _make_manager(tmp_path, runner=NoNpmRunner())
    vscode_dir = tmp_path / ".vscode"
    vscode_dir.mkdir()
    # Write a local MCP with a secret env var.
    (vscode_dir / "mcp.json").write_text(
        json.dumps(
            {
                "servers": {
                    "secret-tool": {
                        "type": "stdio",
                        "command": "node",
                        "args": ["./server.js"],
                        "env": {"SECRET_TOKEN": "hunter2"},
                    }
                }
            }
        )
    )

    manager.reconcile_mcps(tmp_path, probe_version=False)

    servers = _get_servers(manager)
    assert "secret-tool" in servers
    entry = _get_entry(servers, "secret-tool")
    # env must NOT be present in the global config.
    assert "env" not in entry


# ─── probe_version staleness detection ───────────────────────────────────────


def test_reconcile_detects_newer_npm_version_when_probe_enabled(tmp_path: Path) -> None:
    """When a newer npm version is available, reconcile must update the entry."""

    class NewVersionRunner(FakeRunner):
        """Always returns version 3.0.0 regardless of which package is asked."""

        def __init__(self) -> None:
            super().__init__(npm_versions={"@myco/test-mcp": "3.0.0"})

    manager = _make_manager(tmp_path, runner=NewVersionRunner())
    # Use a record with no pinned_tag so version probing is triggered.
    record = McpRecord(kind="npm", package="@myco/test-mcp")
    # Sync once without probing so the stored version is None (stale).
    manager.sync_mcp("test-mcp", record, probe_version=False)

    # Directly inject this record into the catalog so reconcile_mcps picks it up.
    manager.catalog.mcps["test-mcp"] = record

    # Now reconcile with probe enabled – should detect 3.0.0 ≠ stored None.
    results = manager.reconcile_mcps(tmp_path, probe_version=True)
    assert results.get("test-mcp") in {"added", "updated"}
    stored = manager.state_store.read_mcp_state("test-mcp")
    assert stored is not None
    assert stored.installed_version == "3.0.0"
