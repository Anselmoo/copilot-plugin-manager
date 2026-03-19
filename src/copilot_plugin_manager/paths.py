from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

PROFILE_HINT_PATHS = (Path(".copilot-profile"), Path(".github/copilot-profile"))
REPO_CONFIG_PATH = Path(".github/copilot-plugin-manager.json")
PROJECT_CATALOG_PATH = Path(".github/copilot-project-catalog.toml")
LOCAL_AGENTS_PATH = Path(".github/agents")


@dataclass(frozen=True)
class ManagerPaths:
    copilot_home: Path
    manager_home: Path
    skills_dir: Path
    agents_dir: Path
    legacy_active_target_file: Path
    state_file: Path
    sources_dir: Path
    mcp_config_file: Path | None = None

    @classmethod
    def from_environment(cls) -> "ManagerPaths":
        copilot_home = Path(os.environ.get("COPILOT_HOME", Path.home() / ".copilot")).expanduser()
        manager_home = Path(os.environ.get("COPILOT_PLUGIN_MANAGER_HOME", copilot_home / "copilot-plugin-manager")).expanduser()
        return cls(
            copilot_home=copilot_home,
            manager_home=manager_home,
            skills_dir=copilot_home / "skills",
            agents_dir=copilot_home / "agents",
            legacy_active_target_file=copilot_home / "active-profile",
            state_file=manager_home / "state.json",
            sources_dir=manager_home / "sources",
            mcp_config_file=copilot_home / "mcp-config.json",
        )

    def ensure_directories(self) -> None:
        self.manager_home.mkdir(parents=True, exist_ok=True)
        self.sources_dir.mkdir(parents=True, exist_ok=True)

    def repo_root(self, cwd: Path) -> Path:
        return find_project_root(cwd) or cwd.resolve()

    def repo_config_file(self, cwd: Path) -> Path:
        return self.repo_root(cwd) / REPO_CONFIG_PATH

    def project_catalog_file(self, cwd: Path) -> Path:
        return self.repo_root(cwd) / PROJECT_CATALOG_PATH

    def local_agents_dir(self, cwd: Path) -> Path:
        return self.repo_root(cwd) / LOCAL_AGENTS_PATH


def repo_key(path: Path) -> str:
    return str(path.resolve())


def find_repo_profile(start: Path, home: Path | None = None) -> str:
    current = start.resolve()
    stop = (home or Path.home()).resolve()
    while current not in [current.parent, stop]:
        for hint in PROFILE_HINT_PATHS:
            candidate = current / hint
            if candidate.exists():
                return candidate.read_text().strip()
        current = current.parent
    return ""


def find_project_root(start: Path) -> Path | None:
    current = start.resolve()
    while current != current.parent:
        if (current / ".git").exists() or (current / ".gitmodules").exists():
            return current
        current = current.parent
    return None
