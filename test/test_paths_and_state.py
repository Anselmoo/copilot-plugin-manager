from pathlib import Path

from copilot_plugin_manager.models import ActivationTarget, SourceState
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


def test_state_store_normalizes_repo_mapping_to_project_root(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    paths = ManagerPaths.from_environment()
    store = StateStore(paths)
    repo = tmp_path / "repo"
    nested = repo / "src" / "module"
    nested.mkdir(parents=True)
    (repo / ".git").mkdir()
    target = ActivationTarget(name="ts", kind="profile", themes=["core", "typescript"])

    store.write_repo_target(nested, target, "ts")

    repo_state = store.read_repo_state(repo)
    nested_state = store.read_repo_state(nested)

    assert repo_state is not None
    assert nested_state is not None
    assert repo_state.active_target == "ts"


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


def test_state_store_persists_provider_sync_state(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    paths = ManagerPaths.from_environment()
    store = StateStore(paths)

    store.write_provider_state(
        "skill",
        "anthropic-pdf",
        "anthropics-skills",
        SourceState(revision="abc123", manifest_version="1.2.3", source_path="pyproject.toml"),
        ["anthropic-pdf__sample-skill"],
        ["anthropic-pdf: skipped skills/pdf/broken-link (dangling symlink)"],
        "sig-123",
    )
    saved = store.read_provider_state("skill", "anthropic-pdf")

    assert saved is not None
    assert saved.source == "anthropics-skills"
    assert saved.revision == "abc123"
    assert saved.outputs == ["anthropic-pdf__sample-skill"]
    assert saved.warnings == ["anthropic-pdf: skipped skills/pdf/broken-link (dangling symlink)"]
    assert saved.definition_signature == "sig-123"

    store.clear_provider_state("skill", "anthropic-pdf")

    assert store.read_provider_state("skill", "anthropic-pdf") is None
