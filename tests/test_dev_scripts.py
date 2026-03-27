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


def test_release_helper_tasks_are_wired_in_pyproject() -> None:
    tasks = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["tool"]["poe"]["tasks"]

    assert tasks["bump_version"]["cmd"] == "python scripts/bump_version.py"
    assert tasks["bump_patch"]["cmd"] == "python scripts/bump_version.py patch"
    assert tasks["bump_minor"]["cmd"] == "python scripts/bump_version.py minor"
    assert tasks["bump_major"]["cmd"] == "python scripts/bump_version.py major"
    assert (
        tasks["bump_no_changelog"]["cmd"] == "python scripts/bump_version.py patch --no-changelog"
    )
    assert tasks["changelog_preview"]["cmd"] == "python scripts/bump_version.py patch --dry-run"


def test_branch_helper_tasks_are_wired_in_pyproject() -> None:
    tasks = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["tool"]["poe"]["tasks"]

    assert tasks["branch_new"]["cmd"] == "python scripts/branch.py new"
    assert tasks["branch_feat"]["cmd"] == "python scripts/branch.py new feat"
    assert tasks["branch_fix"]["cmd"] == "python scripts/branch.py new fix"
    assert tasks["branch_chore"]["cmd"] == "python scripts/branch.py new chore"
    assert tasks["branch_docs"]["cmd"] == "python scripts/branch.py new docs"
    assert tasks["branch_refactor"]["cmd"] == "python scripts/branch.py new refactor"
    assert tasks["branch_test"]["cmd"] == "python scripts/branch.py new test"
    assert tasks["branch_ci"]["cmd"] == "python scripts/branch.py new ci"
    assert tasks["branch_perf"]["cmd"] == "python scripts/branch.py new perf"
    assert tasks["branch_style"]["cmd"] == "python scripts/branch.py new style"
    assert tasks["branch_build"]["cmd"] == "python scripts/branch.py new build"
    assert tasks["branch_rescue"]["cmd"] == "python scripts/branch.py rescue"


def test_branch_script_new_dry_run_reports_branch_name_and_commit_title() -> None:
    result = _run_script(
        "branch.py",
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
    assert "Branch : feat/cli-add-release-helpers" in result.stdout
    assert "Title  : feat(cli): add release helpers" in result.stdout
    assert "[dry-run complete" in result.stdout


def test_branch_script_rescue_dry_run_reports_rescue_flow() -> None:
    result = _run_script(
        "branch.py",
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
    assert "Rescue commits from 'main' → 'fix/recover-release-branch'" in result.stdout
    assert "Would detect commits ahead of abc1234" in result.stdout
    assert "Suggested commit title:" in result.stdout


def test_bump_version_dry_run_reports_expected_patch_release() -> None:
    project = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
    current = project["project"]["version"]
    major, minor, patch = (int(part) for part in current.split("."))
    expected = f"{major}.{minor}.{patch + 1}"

    result = _run_script("bump_version.py", "patch", "--dry-run", "--no-changelog")

    assert result.returncode == 0, result.stderr
    assert f"Version bump: {current}  →  {expected}" in result.stdout
    assert 'Would update pyproject.toml: version = "' in result.stdout
    assert "Would run: uv lock -U" in result.stdout
    assert f"Would run: git checkout -b release/v{expected}" in result.stdout


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


def test_ci_version_apply_dry_run_updates_all_version_bearing_files() -> None:
    result = _run_script("ci_version.py", "apply", "0.2.0.dev12345602", "--dry-run")

    assert result.returncode == 0, result.stderr
    assert 'Would update pyproject.toml: version = "0.2.0.dev12345602"' in result.stdout
    assert 'Would update Cargo.toml: version = "0.2.0-dev.12345602"' in result.stdout
    assert 'Would update cpm.toml: version = "0.2.0-dev.12345602"' in result.stdout
    assert 'Would update python/cpm/__init__.py: version = "0.2.0.dev12345602"' in result.stdout


def test_ci_version_apply_dry_run_keeps_release_versions_unchanged() -> None:
    result = _run_script("ci_version.py", "apply", "0.2.200", "--dry-run")

    assert result.returncode == 0, result.stderr
    assert 'Would update pyproject.toml: version = "0.2.200"' in result.stdout
    assert 'Would update Cargo.toml: version = "0.2.200"' in result.stdout
    assert 'Would update cpm.toml: version = "0.2.200"' in result.stdout
    assert 'Would update python/cpm/__init__.py: version = "0.2.200"' in result.stdout


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
