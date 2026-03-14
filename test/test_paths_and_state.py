from pathlib import Path

from copilot_plugin_manager.models import ActivationTarget
from copilot_plugin_manager.paths import ManagerPaths, find_repo_profile
from copilot_plugin_manager.state import StateStore


def test_find_repo_profile_reads_github_hint(tmp_path: Path) -> None:
    project = tmp_path / "repo"
    nested = project / "src" / "module"
    nested.mkdir(parents=True)
    hint = project / ".github" / "copilot-profile"
    hint.parent.mkdir(parents=True)
    hint.write_text("python-core\n")

    assert find_repo_profile(nested, tmp_path) == "python-core"


def test_state_store_persists_repo_mapping(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    paths = ManagerPaths.from_environment()
    store = StateStore(paths)
    repo = tmp_path / "repo"
    repo.mkdir()
    target = ActivationTarget(name="docs", kind="profile", themes=["core", "docs"])

    store.write_repo_target(repo, target, "docs")
    saved = store.read_repo_state(repo)

    assert saved is not None
    assert saved.active_target == "docs"
    assert saved.active_kind == "profile"
    assert saved.repo_profile_hint == "docs"


def test_state_store_persists_source_revision_and_manifest_version(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    paths = ManagerPaths.from_environment()
    store = StateStore(paths)

    store.mark_source_revision("awesome-copilot", "abc123", manifest_version="1.2.3", source_path="pyproject.toml")
    state = store.load()

    saved = state.sources["awesome-copilot"]
    assert saved.revision == "abc123"
    assert saved.manifest_version == "1.2.3"
    assert saved.source_path == "pyproject.toml"
    assert saved.measured_at is not None
    assert saved.last_seen_at is not None
    assert saved.updated_at is not None
