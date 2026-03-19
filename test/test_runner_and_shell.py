from pathlib import Path

from typer.main import get_command
from typer.testing import CliRunner

from copilot_plugin_manager import cli
from copilot_plugin_manager.catalog import load_catalog_bundle
from copilot_plugin_manager.cli import app
from copilot_plugin_manager.completion import completion_source, default_completion_path, shell_init_snippet
from copilot_plugin_manager.models import ActivationTarget
from copilot_plugin_manager.paths import ManagerPaths
from copilot_plugin_manager.runner import parse_installed_plugins

runner = CliRunner()


PLUGIN_LIST = """Installed plugins:
  • automate-this@awesome-copilot (v1.0.0)
  • microsoft/hve-core/plugins/hve-core
"""


def test_parse_installed_plugins() -> None:
    parsed = parse_installed_plugins(PLUGIN_LIST)
    assert [item.name for item in parsed] == ["automate-this", "hve-core"]


def test_shell_init_snippets_cover_supported_shells() -> None:
    assert "completion script bash" in shell_init_snippet("bash")
    assert "completion script zsh" in shell_init_snippet("zsh")
    assert "completion script fish" in shell_init_snippet("fish")
    assert "completion script powershell" in shell_init_snippet("powershell")
    assert "completion install nushell" in shell_init_snippet("nushell")
    assert "source " in shell_init_snippet("nushell")


def test_completion_source_covers_supported_shells() -> None:
    command = get_command(app)
    bash_source = completion_source(command, "bash")
    zsh_source = completion_source(command, "zsh")
    fish_source = completion_source(command, "fish")
    powershell_source = completion_source(command, "powershell")
    nushell_source = completion_source(command, "nushell")

    assert "_COPILOT_PLUGIN_MANAGER_COMPLETE=complete_bash" in bash_source
    assert "complete -o default -F _copilot_plugin_manager_completion copilot-plugin-manager" in bash_source
    assert "#compdef copilot-plugin-manager" in zsh_source
    assert "complete --command copilot-plugin-manager --no-files" in fish_source
    assert "Register-ArgumentCompleter -Native -CommandName copilot-plugin-manager" in powershell_source
    assert 'extern "copilot-plugin-manager"' in nushell_source


def test_cli_completion_init_command() -> None:
    result = runner.invoke(app, ["completion", "init", "bash"])
    assert result.exit_code == 0
    assert "completion script bash" in result.stdout


def test_cli_completion_script_command() -> None:
    result = runner.invoke(app, ["completion", "script", "powershell"])
    assert result.exit_code == 0
    assert "Register-ArgumentCompleter -Native -CommandName copilot-plugin-manager" in result.stdout


def test_cli_completion_install_command(tmp_path) -> None:
    target = tmp_path / "copilot-plugin-manager.nu"
    result = runner.invoke(app, ["completion", "install", "nushell", "--path", str(target)])
    assert result.exit_code == 0
    assert target.read_text(encoding="utf-8").startswith("def _copilot_plugin_manager_completion")
    assert f"Installed nushell completion to {target}" in result.stdout


def test_cli_legacy_completion_aliases_still_work(tmp_path) -> None:
    target = tmp_path / "copilot-plugin-manager.nu"

    init_result = runner.invoke(app, ["shell-init", "bash"])
    script_result = runner.invoke(app, ["completion-script", "powershell"])
    install_result = runner.invoke(app, ["completion-install", "nushell", "--path", str(target)])

    assert init_result.exit_code == 0
    assert "completion script bash" in init_result.stdout
    assert script_result.exit_code == 0
    assert "Register-ArgumentCompleter -Native -CommandName copilot-plugin-manager" in script_result.stdout
    assert install_result.exit_code == 0
    assert target.exists()


def test_default_completion_paths_cover_supported_shells(monkeypatch, tmp_path) -> None:
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))

    assert default_completion_path("bash").name == "copilot-plugin-manager"
    assert default_completion_path("zsh").name == "_copilot-plugin-manager"
    assert default_completion_path("fish").suffix == ".fish"
    assert default_completion_path("powershell").suffix == ".ps1"
    assert default_completion_path("nushell").suffix == ".nu"


def test_cli_help_hides_legacy_completion_entrypoints() -> None:
    result = runner.invoke(app, ["--help"])

    assert result.exit_code == 0
    assert "completion" in result.stdout
    assert "shell-init" not in result.stdout
    assert "completion-script" not in result.stdout
    assert "completion-install" not in result.stdout


def test_cli_switch_can_save_repo_profile(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []
            self.saved_paths: list[Path] = []

        def switch_target(
            self,
            target: str,
            cwd: Path,
            exclusive_plugins: bool = False,
            agent_scope: str | None = None,
        ) -> ActivationTarget:
            return self.catalog.resolve_target(target)

        def write_repo_profile(self, cwd: Path, target_name: str, location: str = "root") -> Path:
            path = cwd / (".copilot-profile" if location == "root" else ".github/copilot-profile")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(target_name + "\n")
            self.saved_paths.append(path)
            return path

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(
        app,
        ["switch", "python-core", "--cwd", str(tmp_path), "--save-repo-profile", "--repo-profile-location", "github"],
    )

    assert result.exit_code == 0
    assert "Saved repo target hint to" in result.stdout
    saved_path = tmp_path / ".github" / "copilot-profile"
    assert saved_path.read_text() == "python-core\n"


def test_cli_default_invocation_opens_guided_menu(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []
            self.paths = ManagerPaths(
                tmp_path / ".copilot",
                tmp_path / ".copilot" / "copilot-plugin-manager",
                tmp_path / ".copilot" / "skills",
                tmp_path / ".copilot" / "agents",
                tmp_path / ".copilot" / "active-profile",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "state.json",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "sources",
            )

        def read_active_target(self, cwd: Path) -> str:
            return ""

        def repo_profile_hint(self, cwd: Path) -> str:
            return ""

        def status_snapshot(self, cwd: Path) -> dict[str, object]:
            return {
                "repo_hint": "",
                "repo_profile_file": "",
                "active_target": None,
                "installed_plugins": [],
                "skill_count": 0,
                "agent_count": 0,
                "sync_warnings": [],
                "last_verified_at": None,
                "source_revisions": [],
            }

    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: StubManager())
    monkeypatch.setattr("copilot_plugin_manager.cli._supports_interactive_menu", lambda: True)

    result = runner.invoke(app, [], input="1\nn\n")

    assert result.exit_code == 0
    assert "Choose an action" in result.stdout
    assert "catalog" in result.stdout
    assert "Do you want to choose another action?" in result.stdout


def test_cli_menu_browse_action_opens_catalog_browser(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []
            self.paths = ManagerPaths(
                tmp_path / ".copilot",
                tmp_path / ".copilot" / "copilot-plugin-manager",
                tmp_path / ".copilot" / "skills",
                tmp_path / ".copilot" / "agents",
                tmp_path / ".copilot" / "active-profile",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "state.json",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "sources",
            )

        def read_active_target(self, cwd: Path) -> str:
            return ""

        def repo_profile_hint(self, cwd: Path) -> str:
            return ""

        def status_snapshot(self, cwd: Path) -> dict[str, object]:
            return {
                "repo_hint": "",
                "repo_profile_file": "",
                "active_target": None,
                "installed_plugins": [],
                "skill_count": 0,
                "agent_count": 0,
                "sync_warnings": [],
                "last_verified_at": None,
                "source_revisions": [],
            }

    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: StubManager())
    monkeypatch.setattr("copilot_plugin_manager.cli._supports_interactive_menu", lambda: True)

    result = runner.invoke(app, ["menu"], input="2\n1\nn\nn\n")

    assert result.exit_code == 0
    assert "Catalog browser" in result.stdout
    assert "Choose a catalog view" in result.stdout


def test_cli_list_without_section_opens_catalog_browser(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []

        def read_active_target(self, cwd: Path) -> str:
            return ""

        def repo_profile_hint(self, cwd: Path) -> str:
            return ""

    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: StubManager())
    monkeypatch.setattr("copilot_plugin_manager.cli._supports_interactive_menu", lambda: True)

    result = runner.invoke(app, ["list"], input="1\nn\n")

    assert result.exit_code == 0
    assert "Catalog browser" in result.stdout
    assert "Choose a catalog view" in result.stdout
    assert "Browse another catalog view?" in result.stdout


def test_cli_catalog_group_renders_requested_section(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []

        def read_active_target(self, cwd: Path) -> str:
            return ""

        def repo_profile_hint(self, cwd: Path) -> str:
            return ""

    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: StubManager())

    result = runner.invoke(app, ["catalog", "themes", "--cwd", str(tmp_path)])

    assert result.exit_code == 0
    assert "Themes" in result.stdout


def test_cli_install_can_override_agent_and_mcp_scope(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.sync_warnings: list[str] = []
            self.calls: list[tuple[str, str, Path, str | None, str | None]] = []

        def manage_target(
            self,
            operation: str,
            target: str,
            cwd: Path,
            *,
            agent_scope: str | None = None,
            mcp_scope: str | None = None,
        ) -> None:
            self.calls.append((operation, target, cwd, agent_scope, mcp_scope))

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(
        app,
        ["install", "all", "--cwd", str(tmp_path), "--agent-scope", "local", "--mcp-scope", "local"],
    )

    assert result.exit_code == 0
    assert manager.calls == [("install", "all", tmp_path.resolve(), "local", "local")]


def test_cli_switch_can_override_agent_scope(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []
            self.calls: list[tuple[str, Path, bool, str | None]] = []

        def switch_target(
            self,
            target: str,
            cwd: Path,
            exclusive_plugins: bool = False,
            agent_scope: str | None = None,
        ) -> ActivationTarget:
            self.calls.append((target, cwd, exclusive_plugins, agent_scope))
            return self.catalog.resolve_target(target)

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(app, ["switch", "python-core", "--cwd", str(tmp_path), "--agent-scope", "local"])

    assert result.exit_code == 0
    assert manager.calls == [("python-core", tmp_path.resolve(), False, "local")]


def test_cli_repo_config_updates_and_renders_effective_preferences(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.paths = ManagerPaths(
                tmp_path / ".copilot",
                tmp_path / ".copilot" / "copilot-plugin-manager",
                tmp_path / ".copilot" / "skills",
                tmp_path / ".copilot" / "agents",
                tmp_path / ".copilot" / "active-profile",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "state.json",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "sources",
                tmp_path / ".copilot" / "mcp-config.json",
            )
            self.agent_scope = "global"
            self.mcp_scope = "global"
            self.mcp_profile: str | None = None
            self.write_calls: list[tuple[Path, str | None, str | None, str | None]] = []

        def write_repo_config(
            self,
            cwd: Path,
            *,
            agent_scope: str | None = None,
            mcp_scope: str | None = None,
            mcp_profile: str | None = None,
        ) -> Path:
            if agent_scope is not None:
                self.agent_scope = agent_scope
            if mcp_scope is not None:
                self.mcp_scope = mcp_scope
            if mcp_profile is not None:
                self.mcp_profile = mcp_profile or None
            self.write_calls.append((cwd, agent_scope, mcp_scope, mcp_profile))
            return cwd / ".github" / "copilot-plugin-manager.json"

        def status_snapshot(self, cwd: Path) -> dict[str, object]:
            return {
                "repo_hint": "",
                "repo_profile_file": "",
                "repo_config_file": str(cwd / ".github" / "copilot-plugin-manager.json"),
                "repo_config": {
                    "version": 1,
                    "agents": {"scope": self.agent_scope},
                    "mcps": {"scope": self.mcp_scope, "profile": self.mcp_profile},
                },
                "agent_scope": self.agent_scope,
                "agent_root": str(cwd / ".github" / "agents"),
                "mcp_scope": self.mcp_scope,
                "mcp_profile": self.mcp_profile,
                "active_target": None,
                "installed_plugins": [],
                "skill_count": 0,
                "agent_count": 0,
                "sync_warnings": [],
                "last_verified_at": None,
                "source_revisions": [],
            }

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(
        app,
        [
            "repo-config",
            "--cwd",
            str(tmp_path),
            "--agent-scope",
            "local",
            "--mcp-scope",
            "local",
            "--mcp-profile",
            "team",
        ],
    )

    assert result.exit_code == 0
    assert manager.write_calls == [(tmp_path.resolve(), "local", "local", "team")]
    assert "Updated repo config" in result.stdout
    assert "Agent scope" in result.stdout
    assert "local" in result.stdout
    assert "MCP profile" in result.stdout
    assert "team" in result.stdout


def test_cli_status_shows_repo_config_and_effective_scopes(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.paths = ManagerPaths(
                tmp_path / ".copilot",
                tmp_path / ".copilot" / "copilot-plugin-manager",
                tmp_path / ".copilot" / "skills",
                tmp_path / ".copilot" / "agents",
                tmp_path / ".copilot" / "active-profile",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "state.json",
                tmp_path / ".copilot" / "copilot-plugin-manager" / "sources",
                tmp_path / ".copilot" / "mcp-config.json",
            )

        def status_snapshot(self, cwd: Path) -> dict[str, object]:
            return {
                "repo_hint": "python-core",
                "repo_hint_kind": "profile",
                "repo_hint_themes": ["core", "python", "testing"],
                "repo_profile_file": str(cwd / ".copilot-profile"),
                "repo_config_file": str(cwd / ".github" / "copilot-plugin-manager.json"),
                "repo_config": {
                    "version": 1,
                    "agents": {"scope": "local"},
                    "mcps": {"scope": "local", "profile": "team"},
                },
                "agent_scope": "local",
                "agent_root": str(cwd / ".github" / "agents"),
                "mcp_scope": "local",
                "mcp_profile": "team",
                "active_target": None,
                "installed_plugins": [],
                "skill_count": 2,
                "agent_count": 3,
                "sync_warnings": [],
                "last_verified_at": None,
                "source_revisions": [],
            }

    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: StubManager())

    result = runner.invoke(app, ["status", "--cwd", str(tmp_path)])

    assert result.exit_code == 0
    assert ".github/copilot-plugin-manager.json" in result.stdout
    assert "python-core" in result.stdout
    assert "python, testing" in result.stdout
    assert "Agent scope" in result.stdout
    assert "Agent root" in result.stdout
    assert "MCP scope" in result.stdout
    assert "team" in result.stdout


def test_cli_repo_init_persists_repo_state(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.calls: list[tuple[Path, str | None, str, str | None, str | None, str | None, bool]] = []

        def initialize_repo(
            self,
            cwd: Path,
            *,
            target_name: str | None = None,
            location: str = "root",
            agent_scope: str | None = None,
            mcp_scope: str | None = None,
            mcp_profile: str | None = None,
            force: bool = False,
        ) -> tuple[ActivationTarget, Path, Path | None]:
            self.calls.append((cwd, target_name, location, agent_scope, mcp_scope, mcp_profile, force))
            profile_path = cwd / ".github" / "copilot-profile"
            config_path = cwd / ".github" / "copilot-plugin-manager.json"
            return self.catalog.resolve_target(target_name or "minimal"), profile_path, config_path

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(
        app,
        [
            "repo-init",
            "minimal",
            "--cwd",
            str(tmp_path),
            "--repo-profile-location",
            "github",
            "--agent-scope",
            "local",
            "--mcp-scope",
            "local",
            "--mcp-profile",
            "team",
        ],
    )

    assert result.exit_code == 0
    assert manager.calls == [(tmp_path.resolve(), "minimal", "github", "local", "local", "team", False)]
    assert "Initialized repo profile" in result.stdout
    assert "copilot-profile" in result.stdout
    assert "copilot-plugin-manager.json" in result.stdout


def test_cli_repo_cleanup_reconciles_managed_content(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings = ["verification: unexpected plugins still installed awesome-copilot"]
            self.calls: list[tuple[Path, str | None, str | None]] = []

        def cleanup_repo(
            self,
            cwd: Path,
            *,
            target_name: str | None = None,
            agent_scope: str | None = None,
        ) -> ActivationTarget:
            self.calls.append((cwd, target_name, agent_scope))
            return self.catalog.resolve_target(target_name or "minimal")

    manager = StubManager()
    monkeypatch.setattr("copilot_plugin_manager.cli.get_manager", lambda: manager)

    result = runner.invoke(app, ["repo-cleanup", "--cwd", str(tmp_path), "--agent-scope", "local"])

    assert result.exit_code == 0
    assert manager.calls == [(tmp_path.resolve(), None, "local")]
    assert "Reconciled repo-managed content for" in result.stdout
    assert "verification: unexpected plugins still installed awesome-copilot" in result.stdout


def test_prompt_helpers_reset_keyboard_protocol_for_tty(monkeypatch) -> None:
    class StubStdout:
        def __init__(self) -> None:
            self.writes: list[str] = []

        def isatty(self) -> bool:
            return True

        def write(self, value: str) -> None:
            self.writes.append(value)

        def flush(self) -> None:
            return None

    stdout = StubStdout()
    monkeypatch.setattr(cli.sys, "__stdout__", stdout)
    monkeypatch.setattr(cli.typer, "prompt", lambda message, default=None: default or "")
    monkeypatch.setattr(cli.typer, "confirm", lambda message, default=False: default)

    assert cli._prompt_text("Choose", default="1") == "1"
    assert cli._confirm_text("Again?", default=True) is True
    assert stdout.writes == ["\x1b[<u", "\x1b[<u"]
