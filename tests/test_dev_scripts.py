from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
PYPROJECT = REPO_ROOT / "pyproject.toml"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "cicd.yml"


def _load_ci_version_module():
    module_path = REPO_ROOT / "scripts" / "ci_version.py"
    spec = importlib.util.spec_from_file_location("ci_version_test_module", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Unable to load scripts/ci_version.py for testing")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _run_script(
    script_name: str,
    *args: str,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / script_name), *args],
        cwd=REPO_ROOT,
        env={**os.environ, **(env or {})},
        capture_output=True,
        text=True,
        check=False,
    )


def _write_rrt_fixture(root: Path) -> None:
    (root / "python" / "cpm").mkdir(parents=True)
    (root / "pyproject.toml").write_text(
        """
[project]
name = "fixture"
version = "0.2.2"

[tool.rrt]
release_branch = "release/v{version}"
changelog_file = "CHANGELOG.md"
lock_command = ["uv", "lock", "-U"]

[[tool.rrt.version_targets]]
path = "pyproject.toml"
kind = "pep621"

[[tool.rrt.version_targets]]
path = "Cargo.toml"
section = "workspace.package"
field = "version"

[[tool.rrt.version_targets]]
path = "cpm.toml"
section = "package"
field = "version"

[[tool.rrt.version_targets]]
path = "python/cpm/__init__.py"
pattern = '^(__version__\\s*=\\s*")([^"]+)(")'
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.2.2"\n', encoding="utf-8")
    (root / "cpm.toml").write_text('[package]\nversion = "0.2.2"\n', encoding="utf-8")
    (root / "python" / "cpm" / "__init__.py").write_text(
        '__version__ = "0.2.2"\n',
        encoding="utf-8",
    )


def _run_rrt(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["uv", "run", "--project", str(REPO_ROOT), "rrt", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )


def test_release_helper_tasks_are_wired_in_pyproject() -> None:
    project = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
    tasks = project["tool"]["poe"]["tasks"]

    assert tasks["bump_version"]["cmd"] == "rrt bump"
    assert tasks["bump_patch"]["cmd"] == "rrt bump patch"
    assert tasks["bump_minor"]["cmd"] == "rrt bump minor"
    assert tasks["bump_major"]["cmd"] == "rrt bump major"
    assert tasks["bump_no_changelog"]["cmd"] == "rrt bump patch --no-changelog"
    assert tasks["changelog_preview"]["cmd"] == "rrt bump patch --dry-run"

    rrt = project["tool"]["rrt"]
    assert rrt["release_branch"] == "release/v{version}"
    assert rrt["lock_command"] == ["uv", "lock", "-U"]
    assert [target["path"] for target in rrt["version_targets"]] == [
        "pyproject.toml",
        "Cargo.toml",
        "cpm.toml",
        "python/cpm/__init__.py",
    ]


def test_branch_helper_tasks_are_wired_in_pyproject() -> None:
    tasks = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["tool"]["poe"]["tasks"]

    assert tasks["branch_new"]["cmd"] == "rrt branch new"
    assert tasks["branch_feat"]["cmd"] == "rrt branch new feat"
    assert tasks["branch_fix"]["cmd"] == "rrt branch new fix"
    assert tasks["branch_chore"]["cmd"] == "rrt branch new chore"
    assert tasks["branch_docs"]["cmd"] == "rrt branch new docs"
    assert tasks["branch_refactor"]["cmd"] == "rrt branch new refactor"
    assert tasks["branch_test"]["cmd"] == "rrt branch new test"
    assert tasks["branch_ci"]["cmd"] == "rrt branch new ci"
    assert tasks["branch_perf"]["cmd"] == "rrt branch new perf"
    assert tasks["branch_style"]["cmd"] == "rrt branch new style"
    assert tasks["branch_build"]["cmd"] == "rrt branch new build"
    assert tasks["branch_rescue"]["cmd"] == "rrt branch rescue"


def test_rrt_branch_new_dry_run_reports_branch_name_and_commit_title(tmp_path: Path) -> None:
    _write_rrt_fixture(tmp_path)
    result = _run_rrt(
        tmp_path,
        "branch",
        "new",
        "feat",
        "add",
        "release",
        "helpers",
        "--scope",
        "cli",
        "--dry-run",
    )

    assert result.returncode == 0, result.stderr
    assert "feat/cli-add-release-helpers" in result.stdout
    assert "feat(cli): add release helpers" in result.stdout
    assert "[dry-run] complete" in result.stdout


def test_rrt_branch_rescue_dry_run_reports_rescue_flow(tmp_path: Path) -> None:
    _write_rrt_fixture(tmp_path)
    result = _run_rrt(
        tmp_path,
        "branch",
        "rescue",
        "fix",
        "recover",
        "release",
        "branch",
        "--since",
        "abc1234",
        "--dry-run",
    )

    assert result.returncode == 0, result.stderr
    assert "fix/recover-release-branch" in result.stdout
    assert "Would detect commits ahead of abc1234" in result.stdout
    assert "Suggested commit title:" in result.stdout


def test_rrt_bump_dry_run_reports_expected_patch_release(tmp_path: Path) -> None:
    _write_rrt_fixture(tmp_path)
    result = _run_rrt(tmp_path, "bump", "patch", "--dry-run", "--no-commit", "--no-changelog")

    assert result.returncode == 0, result.stderr
    assert "Current" in result.stdout
    assert "0.2.2 → 0.2.3" in result.stdout
    assert 'Would update' in result.stdout
    assert "Would run: uv lock -U" in result.stdout
    assert "Would run: git checkout -b release/v0.2.3" in result.stdout


def test_ci_version_compute_uses_unique_dev_release_on_main() -> None:
    current = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["project"]["version"]

    result = _run_script(
        "ci_version.py",
        "compute",
        env={
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_REF_NAME": "main",
            "GITHUB_RUN_ID": "123456",
            "GITHUB_RUN_ATTEMPT": "2",
        },
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == f"{current}.dev12345602"


def test_ci_version_compute_uses_tag_version_for_release_tags() -> None:
    result = _run_script(
        "ci_version.py",
        "compute",
        env={
            "GITHUB_REF": "refs/tags/v0.2.0",
            "GITHUB_REF_NAME": "v0.2.0",
        },
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "0.2.0"


def test_ci_version_apply_dry_run_updates_all_version_bearing_files(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    ci_version = _load_ci_version_module()

    pyproject = tmp_path / "pyproject.toml"
    cargo_toml = tmp_path / "Cargo.toml"
    cpm_manifest = tmp_path / "cpm.toml"
    init_file = tmp_path / "python" / "cpm" / "__init__.py"
    init_file.parent.mkdir(parents=True)

    pyproject.write_text('[project]\nversion = "0.2.0"\n', encoding="utf-8")
    cargo_toml.write_text('[workspace.package]\nversion = "0.2.0"\n', encoding="utf-8")
    cpm_manifest.write_text('[package]\nversion = "0.2.0"\n', encoding="utf-8")
    init_file.write_text('__version__ = "0.2.0"\n', encoding="utf-8")

    monkeypatch.setattr(ci_version, "ROOT", tmp_path)
    monkeypatch.setattr(ci_version, "PYPROJECT", pyproject)
    monkeypatch.setattr(ci_version, "CARGO_TOML", cargo_toml)
    monkeypatch.setattr(ci_version, "CPM_MANIFEST", cpm_manifest)
    monkeypatch.setattr(ci_version, "INIT_FILE", init_file)

    ci_version.apply_version("0.2.0.dev12345602", dry_run=True)
    stdout = capsys.readouterr().out

    assert 'Would update pyproject.toml: version = "0.2.0.dev12345602"' in stdout
    assert 'Would update Cargo.toml: version = "0.2.0-dev.12345602"' in stdout
    assert 'Would update cpm.toml: version = "0.2.0-dev.12345602"' in stdout
    assert 'Would update python/cpm/__init__.py: version = "0.2.0.dev12345602"' in stdout


def test_ci_version_apply_dry_run_keeps_release_versions_unchanged(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    ci_version = _load_ci_version_module()

    pyproject = tmp_path / "pyproject.toml"
    cargo_toml = tmp_path / "Cargo.toml"
    cpm_manifest = tmp_path / "cpm.toml"
    init_file = tmp_path / "python" / "cpm" / "__init__.py"
    init_file.parent.mkdir(parents=True)

    pyproject.write_text('[project]\nversion = "0.2.0"\n', encoding="utf-8")
    cargo_toml.write_text('[workspace.package]\nversion = "0.2.0"\n', encoding="utf-8")
    cpm_manifest.write_text('[package]\nversion = "0.2.0"\n', encoding="utf-8")
    init_file.write_text('__version__ = "0.2.0"\n', encoding="utf-8")

    monkeypatch.setattr(ci_version, "ROOT", tmp_path)
    monkeypatch.setattr(ci_version, "PYPROJECT", pyproject)
    monkeypatch.setattr(ci_version, "CARGO_TOML", cargo_toml)
    monkeypatch.setattr(ci_version, "CPM_MANIFEST", cpm_manifest)
    monkeypatch.setattr(ci_version, "INIT_FILE", init_file)

    ci_version.apply_version("0.2.200", dry_run=True)
    stdout = capsys.readouterr().out

    assert 'Would update pyproject.toml: version = "0.2.200"' in stdout
    assert 'Would update Cargo.toml: version = "0.2.200"' in stdout
    assert 'Would update cpm.toml: version = "0.2.200"' in stdout
    assert 'Would update python/cpm/__init__.py: version = "0.2.200"' in stdout


def test_ci_version_apply_logs_are_windows_console_safe(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    ci_version = _load_ci_version_module()

    pyproject = tmp_path / "pyproject.toml"
    cargo_toml = tmp_path / "Cargo.toml"
    cpm_manifest = tmp_path / "cpm.toml"
    init_file = tmp_path / "python" / "cpm" / "__init__.py"
    init_file.parent.mkdir(parents=True)

    pyproject.write_text('[project]\nversion = "0.2.0"\n', encoding="utf-8")
    cargo_toml.write_text('[workspace.package]\nversion = "0.2.0"\n', encoding="utf-8")
    cpm_manifest.write_text('[package]\nversion = "0.2.0"\n', encoding="utf-8")
    init_file.write_text('__version__ = "0.2.0"\n', encoding="utf-8")

    monkeypatch.setattr(ci_version, "ROOT", tmp_path)
    monkeypatch.setattr(ci_version, "PYPROJECT", pyproject)
    monkeypatch.setattr(ci_version, "CARGO_TOML", cargo_toml)
    monkeypatch.setattr(ci_version, "CPM_MANIFEST", cpm_manifest)
    monkeypatch.setattr(ci_version, "INIT_FILE", init_file)

    ci_version.apply_version("0.2.0.dev12345602", dry_run=False)
    stdout = capsys.readouterr().out

    stdout.encode("cp1252")
    assert '[updated] pyproject.toml -> version = "0.2.0.dev12345602"' in stdout
    assert '[updated] Cargo.toml -> version = "0.2.0-dev.12345602"' in stdout


def test_release_workflow_computes_and_applies_ci_candidate_versions() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")

    assert "python scripts/ci_version.py compute" in workflow
    assert (
        'shell: bash\n        run: python scripts/ci_version.py apply "$PUBLISHED_VERSION"'
        in workflow
    )
    assert 'python scripts/ci_version.py apply "$PUBLISHED_VERSION"' in workflow
    assert "copilot-plugin-manager==${PUBLISHED_VERSION}" in workflow
