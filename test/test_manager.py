from pathlib import Path

from copilot_plugin_manager.catalog import load_catalog_bundle
from copilot_plugin_manager.manager import PluginManager
from copilot_plugin_manager.models import SourceState
from copilot_plugin_manager.paths import ManagerPaths
from copilot_plugin_manager.runner import CommandResult, ShellRunner


class FakeRunner(ShellRunner):
    def __init__(self) -> None:
        self.calls: list[tuple[str, ...]] = []

    def require(self, name: str) -> None:
        return None

    def run(
        self,
        args: list[str],
        cwd: Path | None = None,
        check: bool = True,
    ) -> CommandResult:
        self.calls.append(tuple(args))
        return CommandResult(
            tuple(args),
            "Installed plugins:\n  • awesome-copilot@awesome-copilot (v1.0.0)\n",
            "",
            0,
        )


class GitCloneRunner(FakeRunner):
    def __init__(self, clone_layouts: dict[str, dict[str, str]] | None = None) -> None:
        super().__init__()
        self.clone_layouts = clone_layouts or {}

    def run(
        self,
        args: list[str],
        cwd: Path | None = None,
        check: bool = True,
    ) -> CommandResult:
        self.calls.append(tuple(args))
        if args[:4] == ["git", "clone", "--depth", "1"]:
            destination = Path(args[-1])
            destination.mkdir(parents=True, exist_ok=True)
            for relative_path, content in self.clone_layouts.get(destination.name, {}).items():
                target = destination / relative_path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content)
            return CommandResult(tuple(args), "", "", 0)
        if args[:3] == ["git", "pull", "--ff-only"]:
            return CommandResult(tuple(args), "", "", 0)
        if args[:3] == ["git", "rev-parse", "HEAD"]:
            return CommandResult(tuple(args), "abc123\n", "", 0)
        return CommandResult(
            tuple(args),
            "Installed plugins:\n  • awesome-copilot@awesome-copilot (v1.0.0)\n",
            "",
            0,
        )


def test_plugin_actions_for_switch_non_exclusive() -> None:
    manager = PluginManager(
        load_catalog_bundle(),
        ManagerPaths(
            Path("/tmp/.copilot"),
            Path("/tmp/.copilot/copilot-plugin-manager"),
            Path("/tmp/.copilot/skills"),
            Path("/tmp/.copilot/agents"),
            Path("/tmp/.copilot/active-profile"),
            Path("/tmp/.copilot/copilot-plugin-manager/state.json"),
            Path("/tmp/.copilot/copilot-plugin-manager/sources"),
        ),
        runner=FakeRunner(),
    )
    actions = manager.plugin_actions_for_switch("minimal", ["awesome-copilot", "partners"], exclusive=False)
    commands = [action.command for action in actions]
    assert ("copilot", "plugin", "uninstall", "partners") in commands
    assert ("copilot", "plugin", "install", "context-engineering@awesome-copilot") in commands


def test_sync_skill_provider_from_submodule_layout(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    runner = GitCloneRunner()
    manager = PluginManager(bundle, paths, runner=runner)
    source = bundle.repositories["anthropics-skills"]
    clone_call = ("git", "clone", "--depth", "1", f"https://github.com/{source.owner}/{source.repo}.git", str(paths.sources_dir / "anthropics-skills"))

    project = tmp_path / "repo"
    source_root = project / "external" / "anthropics-skills" / "skills" / "pdf" / "sample-skill"
    source_root.mkdir(parents=True)
    (source_root / "README.md").write_text("sample")

    manager.sync_skill_provider("anthropic-pdf", project)

    assert (paths.skills_dir / "anthropic-pdf__sample-skill").exists()
    assert clone_call not in runner.calls


def test_sync_skill_provider_bootstraps_missing_cached_checkout(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    source = bundle.repositories["anthropics-skills"]
    clone_call = ("git", "clone", "--depth", "1", f"https://github.com/{source.owner}/{source.repo}.git", str(paths.sources_dir / "anthropics-skills"))
    runner = GitCloneRunner(
        {
            "anthropics-skills": {
                "skills/pdf/sample-skill/README.md": "sample",
            }
        }
    )
    manager = PluginManager(bundle, paths, runner=runner)

    project = tmp_path / "repo"
    project.mkdir()

    manager.sync_skill_provider("anthropic-pdf", project)

    assert (paths.skills_dir / "anthropic-pdf__sample-skill").exists()
    assert clone_call in runner.calls


def test_sync_missing_skill_providers_reuses_cached_outputs(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    runner = GitCloneRunner(
        {
            "anthropics-skills": {
                "skills/pdf/sample-skill/README.md": "sample",
            }
        }
    )
    manager = PluginManager(bundle, paths, runner=runner)
    project = tmp_path / "repo"
    project.mkdir()

    manager.sync_skill_provider("anthropic-pdf", project)
    calls: list[str] = []
    original = manager.sync_skill_provider

    def tracked(provider_name: str, cwd: Path, *, source_root: Path | None = None, observed: SourceState | None = None) -> list[str]:
        calls.append(provider_name)
        return original(provider_name, cwd, source_root=source_root, observed=observed)

    monkeypatch.setattr(manager, "sync_skill_provider", tracked)
    manager._sync_missing_skill_providers(["anthropic-pdf"], project)

    assert calls == []


def test_sync_skill_provider_skips_dangling_symlinks_and_records_warning(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())

    project = tmp_path / "repo"
    compute_root = project / "external" / "microsoft-skills" / "skills" / "typescript" / "compute"
    good_skill = compute_root / "calculator"
    good_skill.mkdir(parents=True)
    (good_skill / "README.md").write_text("sample")
    (compute_root / "playwright").symlink_to(compute_root / "missing-playwright")

    outputs = manager.sync_skill_provider("mskills-typescript", project)

    assert "mskills-typescript__compute" in outputs
    assert (paths.skills_dir / "mskills-typescript__compute").exists()
    assert not (paths.skills_dir / "mskills-typescript__compute" / "playwright").exists()
    assert manager.sync_warnings == ["mskills-typescript: skipped skills/typescript/compute/playwright (dangling symlink)"]
    saved = manager.state_store.read_provider_state("skill", "mskills-typescript")
    assert saved is not None
    assert saved.warnings == manager.sync_warnings


def test_sync_missing_skill_providers_retries_when_previous_sync_had_warnings(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())
    project = tmp_path / "repo"
    compute_root = project / "external" / "microsoft-skills" / "skills" / "typescript" / "compute"
    good_skill = compute_root / "calculator"
    good_skill.mkdir(parents=True)
    (good_skill / "README.md").write_text("sample")
    (compute_root / "playwright").symlink_to(compute_root / "missing-playwright")

    manager.sync_skill_provider("mskills-typescript", project)
    calls: list[str] = []
    original = manager.sync_skill_provider

    def tracked(provider_name: str, cwd: Path, *, source_root: Path | None = None, observed: SourceState | None = None) -> list[str]:
        calls.append(provider_name)
        return original(provider_name, cwd, source_root=source_root, observed=observed)

    monkeypatch.setattr(manager, "sync_skill_provider", tracked)
    manager._sync_missing_skill_providers(["mskills-typescript"], project)

    assert calls == ["mskills-typescript"]


def test_write_repo_profile_creates_hint_in_repo_root(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())
    project = tmp_path / "repo"
    nested = project / "src" / "feature"
    nested.mkdir(parents=True)
    (project / ".git").mkdir()

    saved = manager.write_repo_profile(nested, "python-core", "github")

    assert saved == project / ".github" / "copilot-profile"
    assert saved.read_text() == "python-core\n"


def test_sync_agent_provider_normalizes_output_name_and_metadata(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())

    project = tmp_path / "repo"
    source_file = project / "external" / "agency-agents" / "design" / "design-brand-guardian.md"
    source_file.parent.mkdir(parents=True)
    source_file.write_text("# Design Brand Guardian\\n\\nHelps with brand systems.\\n")

    manager.sync_agent_provider("agency-design-brand-guardian", project)

    generated = paths.agents_dir / "agency-design-brand-guardian__design__design-brand-guardian.agent.md"
    assert generated.exists()
    content = generated.read_text()
    assert "Generated by copilot-plugin-manager." in content
    assert content.startswith("<!--")
    assert "commit_revision:" in content
    assert "# Design Brand Guardian" in content


def test_sync_missing_agent_providers_dedupes_overlapping_sources(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())

    project = tmp_path / "repo"
    source_file = project / "external" / "agency-agents" / "design" / "design-brand-guardian.md"
    source_file.parent.mkdir(parents=True)
    source_file.write_text("# Design Brand Guardian\n\nHelps with brand systems.\n")

    manager.sync_agent_provider("agency", project)
    broad_output = paths.agents_dir / "agency__design__design-brand-guardian.agent.md"
    assert broad_output.exists()

    manager._sync_missing_agent_providers(["agency", "agency-design-brand-guardian"], project)

    leaf_output = paths.agents_dir / "agency-design-brand-guardian__design__design-brand-guardian.agent.md"
    assert leaf_output.exists()
    assert not broad_output.exists()


def test_sync_missing_agent_providers_reuses_cached_outputs(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())

    project = tmp_path / "repo"
    source_file = project / "external" / "agency-agents" / "design" / "design-brand-guardian.md"
    source_file.parent.mkdir(parents=True)
    source_file.write_text("# Design Brand Guardian\n\nHelps with brand systems.\n")

    manager.sync_agent_provider("agency-design-brand-guardian", project)
    calls: list[str] = []
    original = manager.sync_agent_provider

    def tracked(
        provider_name: str,
        cwd: Path,
        *,
        claimed_source_paths: set[tuple[str, str]] | None = None,
        source_root: Path | None = None,
        observed: SourceState | None = None,
    ) -> list[str]:
        calls.append(provider_name)
        return original(
            provider_name,
            cwd,
            claimed_source_paths=claimed_source_paths,
            source_root=source_root,
            observed=observed,
        )

    monkeypatch.setattr(manager, "sync_agent_provider", tracked)
    manager._sync_missing_agent_providers(["agency-design-brand-guardian"], project)

    assert calls == []


def test_probe_manifest_version_supports_generic_manifests(tmp_path: Path) -> None:
    manager = PluginManager(
        load_catalog_bundle(),
        ManagerPaths(
            Path("/tmp/.copilot"),
            Path("/tmp/.copilot/copilot-plugin-manager"),
            Path("/tmp/.copilot/skills"),
            Path("/tmp/.copilot/agents"),
            Path("/tmp/.copilot/active-profile"),
            Path("/tmp/.copilot/copilot-plugin-manager/state.json"),
            Path("/tmp/.copilot/copilot-plugin-manager/sources"),
        ),
        runner=FakeRunner(),
    )

    package_root = tmp_path / "package"
    package_root.mkdir()
    (package_root / "package.json").write_text('{"name":"demo","version":"1.2.3"}')

    pyproject_root = tmp_path / "pyproject"
    pyproject_root.mkdir()
    (pyproject_root / "pyproject.toml").write_text('[project]\nname = "demo"\nversion = "2.0.0"\n')

    assert manager.probe_manifest_version(package_root) == "1.2.3"
    assert manager.probe_manifest_version(pyproject_root) == "2.0.0"


def test_source_state_compares_by_revision_before_manifest_version() -> None:
    previous = SourceState(revision="abc123", manifest_version="1.0.0")
    same_revision_newer_manifest = SourceState(revision="abc123", manifest_version="2.0.0")
    changed_revision = SourceState(revision="def456", manifest_version="1.0.0")
    manifest_only_change = SourceState(manifest_version="2.0.0")

    assert same_revision_newer_manifest.has_comparable_change(previous) is False
    assert changed_revision.has_comparable_change(previous) is True
    assert manifest_only_change.has_comparable_change(SourceState(manifest_version="1.0.0")) is True


def test_repo_update_clones_cache_inside_regular_git_repository(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    runner = GitCloneRunner()
    manager = PluginManager(bundle, paths, runner=runner)

    project = tmp_path / "repo"
    project.mkdir()
    (project / ".git").mkdir()

    revisions = manager.repo_update(project, remote=False)

    clone_calls = [call for call in runner.calls if call[:4] == ("git", "clone", "--depth", "1")]
    assert len(clone_calls) == len(bundle.repositories)
    assert revisions["anthropics-skills"] == "abc123"
    assert (paths.sources_dir / "anthropics-skills").exists()


def test_status_snapshot_includes_repo_profile_file_and_sync_warnings(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("COPILOT_HOME", str(tmp_path / ".copilot"))
    bundle = load_catalog_bundle()
    paths = ManagerPaths.from_environment()
    manager = PluginManager(bundle, paths, runner=FakeRunner())
    project = tmp_path / "repo"
    project.mkdir()
    (project / ".git").mkdir()
    (project / ".copilot-profile").write_text("ts\n")
    compute_root = project / "external" / "microsoft-skills" / "skills" / "typescript" / "compute"
    good_skill = compute_root / "calculator"
    good_skill.mkdir(parents=True)
    (good_skill / "README.md").write_text("sample")
    (compute_root / "playwright").symlink_to(compute_root / "missing-playwright")

    manager.sync_skill_provider("mskills-typescript", project)
    snapshot = manager.status_snapshot(project)

    assert snapshot["repo_profile_file"] == str(project / ".copilot-profile")
    assert snapshot["sync_warnings"] == ["mskills-typescript: skipped skills/typescript/compute/playwright (dangling symlink)"]
