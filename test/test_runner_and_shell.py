from pathlib import Path

from typer.main import get_command
from typer.testing import CliRunner

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
    assert "completion-script bash" in shell_init_snippet("bash")
    assert "completion-script zsh" in shell_init_snippet("zsh")
    assert "completion-script fish" in shell_init_snippet("fish")
    assert "completion-script powershell" in shell_init_snippet("powershell")
    assert "completion-install nushell" in shell_init_snippet("nushell")
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


def test_cli_shell_init_command() -> None:
    result = runner.invoke(app, ["shell-init", "bash"])
    assert result.exit_code == 0
    assert "completion-script bash" in result.stdout


def test_cli_completion_script_command() -> None:
    result = runner.invoke(app, ["completion-script", "powershell"])
    assert result.exit_code == 0
    assert "Register-ArgumentCompleter -Native -CommandName copilot-plugin-manager" in result.stdout


def test_cli_completion_install_command(tmp_path) -> None:
    target = tmp_path / "copilot-plugin-manager.nu"
    result = runner.invoke(app, ["completion-install", "nushell", "--path", str(target)])
    assert result.exit_code == 0
    assert target.read_text(encoding="utf-8").startswith("def _copilot_plugin_manager_completion")
    assert f"Installed nushell completion to {target}" in result.stdout


def test_default_completion_paths_cover_supported_shells(monkeypatch, tmp_path) -> None:
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))

    assert default_completion_path("bash").name == "copilot-plugin-manager"
    assert default_completion_path("zsh").name == "_copilot-plugin-manager"
    assert default_completion_path("fish").suffix == ".fish"
    assert default_completion_path("powershell").suffix == ".ps1"
    assert default_completion_path("nushell").suffix == ".nu"


def test_cli_switch_can_save_repo_profile(monkeypatch, tmp_path) -> None:
    class StubManager:
        def __init__(self) -> None:
            self.catalog = load_catalog_bundle()
            self.sync_warnings: list[str] = []
            self.saved_paths: list[Path] = []

        def switch_target(self, target: str, cwd: Path, exclusive_plugins: bool = False) -> ActivationTarget:
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
    assert "Saved repo profile hint to" in result.stdout
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
    assert "Do you want to choose another action?" in result.stdout
