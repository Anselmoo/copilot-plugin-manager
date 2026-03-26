from __future__ import annotations

import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

import cpm._cli
from cpm._cli import find_repo_root, resolve_delegate

# ── Helpers ──────────────────────────────────────────────────────────────────

REPO_ROOT = Path(__file__).resolve().parents[1]


def _write_workspace(repo_root: Path) -> None:
    (repo_root / "crates/cpm-cli").mkdir(parents=True)
    (repo_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    (repo_root / "crates/cpm-cli/Cargo.toml").write_text(
        '[package]\nname = "cpm-cli"\nversion = "0.0.0"\n',
        encoding="utf-8",
    )


def _write_binary(binary: Path) -> None:
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    binary.chmod(0o755)


def _run_cpm(
    *args: str,
    cwd: Path,
    env: dict | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run cpm via python -m cpm with no CPM_BIN override."""
    e = (env or os.environ).copy()
    e.pop("CPM_BIN", None)
    return subprocess.run(
        [sys.executable, "-m", "cpm", *args],
        cwd=cwd,
        env=e,
        capture_output=True,
        text=True,
        check=False,
    )


def _uv_run_project(
    repo_root: Path,
    *args: str,
    cwd: Path,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.pop("CPM_BIN", None)
    return subprocess.run(
        ["uv", "run", "--project", str(repo_root), *args],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def _make_skill_dir(parent: Path, name: str, extra_files: dict[str, str] | None = None) -> Path:
    """Create a minimal skill directory under *parent*."""
    skill_dir = parent / name
    skill_dir.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_text(f"# {name}\n", encoding="utf-8")
    for fname, content in (extra_files or {}).items():
        (skill_dir / fname).write_text(content, encoding="utf-8")
    return skill_dir


def _make_hook_dir(parent: Path, name: str) -> Path:
    hook_dir = parent / name
    hook_dir.mkdir(parents=True, exist_ok=True)
    (hook_dir / "pre-commit.sh").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    return hook_dir


def _make_workflow_file(parent: Path, name: str) -> Path:
    parent.mkdir(parents=True, exist_ok=True)
    workflow = parent / f"{name}.md"
    workflow.write_text("# Workflow\n\n- step: review\n", encoding="utf-8")
    return workflow


# ── Delegation layer unit tests ───────────────────────────────────────────────


def test_find_repo_root_discovers_workspace() -> None:
    repo_root = find_repo_root()

    assert repo_root is not None
    assert (repo_root / "Cargo.toml").is_file()
    assert (repo_root / "crates/cpm-cli/Cargo.toml").is_file()


def test_resolve_delegate_prefers_cargo_in_source_checkout(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    _write_workspace(repo_root)

    delegate = resolve_delegate(
        ["--help"],
        repo_root=repo_root,
        current_executable=repo_root / "python/bin/cpm",
        path_lookup=lambda _: None,
    )

    assert delegate.command == [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(repo_root / "Cargo.toml"),
        "-p",
        "cpm-cli",
        "--",
        "--help",
    ]
    assert delegate.cwd is None


def test_resolve_delegate_falls_back_to_cargo(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    _write_workspace(repo_root)

    delegate = resolve_delegate(
        ["status"],
        repo_root=repo_root,
        current_executable=repo_root / "python/bin/cpm",
        path_lookup=lambda _: None,
    )

    assert delegate.command == [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(repo_root / "Cargo.toml"),
        "-p",
        "cpm-cli",
        "--",
        "status",
    ]
    assert delegate.cwd is None


def test_resolve_delegate_honors_force_cargo_over_configured_binary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo_root = tmp_path / "repo"
    _write_workspace(repo_root)

    configured_binary = tmp_path / "bin/cpm"
    _write_binary(configured_binary)
    monkeypatch.setenv("CPM_BIN", str(configured_binary))

    delegate = resolve_delegate(
        ["--help"],
        prefer_cargo=True,
        repo_root=repo_root,
        current_executable=repo_root / "python/bin/cpm",
        path_lookup=lambda _: str(configured_binary),
    )

    assert delegate.command == [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(repo_root / "Cargo.toml"),
        "-p",
        "cpm-cli",
        "--",
        "--help",
    ]


def test_resolve_delegate_uses_cpmbın_env_var(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    configured_binary = tmp_path / "bin/cpm"
    _write_binary(configured_binary)
    monkeypatch.setenv("CPM_BIN", str(configured_binary))

    delegate = resolve_delegate(
        ["list"],
        current_executable=tmp_path / "other/bin/cpm",
        path_lookup=lambda _: None,
    )
    assert delegate.command == [str(configured_binary), "list"]


def test_resolve_delegate_requires_source_checkout_when_force_cargo_requested(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    configured_binary = tmp_path / "bin/cpm"
    _write_binary(configured_binary)
    monkeypatch.setattr(cpm._cli, "find_repo_root", lambda start=None: None)

    with pytest.raises(RuntimeError, match="source checkout"):
        resolve_delegate(
            ["--help"],
            prefer_cargo=True,
            current_executable=tmp_path / "python/bin/cpm",
            path_lookup=lambda _: str(configured_binary),
        )


def test_resolve_delegate_raises_when_no_binary_available(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cpm._cli, "find_repo_root", lambda start=None: None)
    monkeypatch.delenv("CPM_BIN", raising=False)

    with pytest.raises(RuntimeError, match="Unable to locate"):
        resolve_delegate(
            ["--help"],
            current_executable=tmp_path / "python/bin/cpm",
            path_lookup=lambda _: None,
        )


# ── Runtime integration tests (python -m cpm) ────────────────────────────────


def test_python_module_help_exits_zero(tmp_path: Path) -> None:
    result = _run_cpm("--help", cwd=tmp_path)
    assert result.returncode == 0
    assert "cpm" in result.stdout.lower()


def test_python_module_version_exits_zero(tmp_path: Path) -> None:
    result = _run_cpm("--version", cwd=tmp_path)
    assert result.returncode == 0


def test_uv_run_legacy_console_script_exits_zero(tmp_path: Path) -> None:
    result = _uv_run_project(REPO_ROOT, "copilot-plugin-manager", "--help", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert "cpm" in result.stdout.lower()


def test_python_module_init_creates_manifest_and_lockfile(tmp_path: Path) -> None:
    result = _run_cpm("init", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert (tmp_path / "cpm.toml").is_file()
    assert (tmp_path / "cpm.lock").is_file()
    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert "package" in manifest
    assert manifest["package"]["name"] == tmp_path.name


def test_python_module_init_with_explicit_name(tmp_path: Path) -> None:
    result = _run_cpm("init", "--name", "my-copilot-project", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert manifest["package"]["name"] == "my-copilot-project"


def test_python_module_init_does_not_overwrite_existing(tmp_path: Path) -> None:
    (tmp_path / "cpm.toml").write_text("[package]\nname = 'original'\n", encoding="utf-8")
    result = _run_cpm("init", cwd=tmp_path)
    assert result.returncode == 0
    # The existing manifest must remain unchanged.
    text = (tmp_path / "cpm.toml").read_text(encoding="utf-8")
    assert "original" in text


def test_python_module_delegates_to_rust_add(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "delegated-pdf")

    result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    assert result.returncode == 0, result.stderr
    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert manifest["skills"]["delegated-pdf"]["path"] == str(Path("skills/delegated-pdf"))


def test_python_module_materializes_local_skill_and_lockfile(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(
        tmp_path / "skills",
        "local-pdf",
        extra_files={"helper.txt": "helper\n"},
    )

    result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    assert result.returncode == 0, result.stderr

    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    lockfile = tomllib.loads((tmp_path / "cpm.lock").read_text(encoding="utf-8"))

    assert manifest["skills"]["local-pdf"]["path"] == str(Path("skills/local-pdf"))
    assert (tmp_path / ".github" / "skills" / "local-pdf" / "SKILL.md").is_file()
    assert (tmp_path / ".github" / "skills" / "local-pdf" / "helper.txt").is_file()
    assert lockfile["version"] == 1
    assert len(lockfile["skill"]) == 1
    assert lockfile["skill"][0]["name"] == "local-pdf"


def test_python_module_add_multiple_skills(tmp_path: Path) -> None:
    """Adding two skills in sequence should accumulate both in the manifest."""
    for name in ("alpha", "beta"):
        skill_dir = _make_skill_dir(tmp_path / "skills", name)
        result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)
        assert result.returncode == 0, result.stderr

    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert "alpha" in manifest["skills"]
    assert "beta" in manifest["skills"]

    lockfile = tomllib.loads((tmp_path / "cpm.lock").read_text(encoding="utf-8"))
    names = [e["name"] for e in lockfile["skill"]]
    assert "alpha" in names
    assert "beta" in names


def test_python_module_remove_skill(tmp_path: Path) -> None:
    """A skill added then removed should not appear in the manifest."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "to-remove")
    _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    result = _run_cpm("remove", "to-remove", "--skill", cwd=tmp_path)
    assert result.returncode == 0, result.stderr

    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert "to-remove" not in manifest.get("skills", {})


def test_python_module_doctor_clean_project(tmp_path: Path) -> None:
    """doctor on a freshly synced project must exit 0."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "healthy")
    _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    result = _run_cpm("doctor", cwd=tmp_path)
    assert result.returncode == 0, result.stderr


def test_python_module_status_after_add(tmp_path: Path) -> None:
    """status on a freshly synced project should report nothing unexpected."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "status-skill")
    _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    result = _run_cpm("status", cwd=tmp_path)
    assert result.returncode == 0, result.stderr


def test_python_module_list_after_add(tmp_path: Path) -> None:
    """list should mention the installed asset after add."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "listed-skill")
    _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    result = _run_cpm("list", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert "listed-skill" in result.stdout


def test_python_module_materializes_hook_bundle(tmp_path: Path) -> None:
    hook_dir = _make_hook_dir(tmp_path / "hooks", "guardrails")

    result = _run_cpm("add", str(hook_dir), "--hook", cwd=tmp_path)

    assert result.returncode == 0, result.stderr
    assert (tmp_path / ".github" / "hooks" / "guardrails" / "pre-commit.sh").is_file()


def test_python_module_materializes_workflow_and_reset_removes_sidecar(tmp_path: Path) -> None:
    workflow = _make_workflow_file(tmp_path / "workflows", "review")

    add_result = _run_cpm("add", str(workflow), "--workflow", cwd=tmp_path)
    assert add_result.returncode == 0, add_result.stderr

    compiled_sidecar = tmp_path / ".github" / "workflows" / "review.lock.yml"
    compiled_sidecar.write_text("name: review\n", encoding="utf-8")

    reset_result = _run_cpm("reset", "--workflow", "--force", cwd=tmp_path)
    assert reset_result.returncode == 0, reset_result.stderr
    assert not (tmp_path / ".github" / "workflows" / "review.md").exists()
    assert not compiled_sidecar.exists()


def test_python_module_rejects_global_workflow_scope(tmp_path: Path) -> None:
    workflow = _make_workflow_file(tmp_path / "workflows", "global-review")

    result = _run_cpm("add", str(workflow), "--workflow", "--scope", "global", cwd=tmp_path)

    assert result.returncode != 0
    assert "local-only" in result.stderr or "local-only" in result.stdout


def test_python_module_show_returns_details(tmp_path: Path) -> None:
    """show should print details for a known asset."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "detail-skill")
    _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)

    result = _run_cpm("show", "detail-skill", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert "detail-skill" in result.stdout


def test_python_module_lock_check_exits_nonzero_when_manifest_needs_lock(tmp_path: Path) -> None:
    """lock --check should fail if manifest entries exist but cpm.lock is absent."""
    skill_dir = _make_skill_dir(tmp_path / "skills", "needs-lock")
    (tmp_path / "cpm.toml").write_text(
        f'[skills]\nneeds-lock = {{ path = "{skill_dir.relative_to(tmp_path).as_posix()}" }}\n',
        encoding="utf-8",
    )
    result = _run_cpm("lock", "--check", cwd=tmp_path)
    assert result.returncode != 0
    assert "lock out of date" in (result.stderr or result.stdout)


def test_python_module_cache_dir_prints_path(tmp_path: Path) -> None:
    """cache dir should print a non-empty path."""
    result = _run_cpm("cache", "dir", cwd=tmp_path)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() != ""


def test_python_module_auth_status_exits_zero(tmp_path: Path) -> None:
    """auth status should always exit 0 (authenticated or not)."""
    result = _run_cpm("auth", "status", cwd=tmp_path)
    assert result.returncode == 0, result.stderr


def test_python_module_subcommand_help_exits_zero(tmp_path: Path) -> None:
    """Every subcommand should have --help that exits 0."""
    for subcmd in (
        "init",
        "add",
        "sync",
        "remove",
        "update",
        "lock",
        "overview",
        "list",
        "show",
        "doctor",
        "status",
        "tree",
        "reset",
        "cache",
        "auth",
        "scope",
    ):
        result = _run_cpm(subcmd, "--help", cwd=tmp_path)
        assert result.returncode == 0, f"{subcmd} --help failed: {result.stderr}"


def test_python_module_overview_reports_unmanaged_files(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "tracked-skill")
    add_result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)
    assert add_result.returncode == 0, add_result.stderr

    unmanaged_dir = tmp_path / ".github" / "skills" / "manual" / "docs"
    unmanaged_dir.mkdir(parents=True, exist_ok=True)
    ((tmp_path / ".github" / "skills" / "manual") / "SKILL.md").write_text(
        "# manual\n", encoding="utf-8"
    )
    (unmanaged_dir / "guide.md").write_text("# guide\n", encoding="utf-8")

    overview = _run_cpm("overview", "--json", cwd=tmp_path)
    assert overview.returncode == 0, overview.stderr
    payload = json.loads(overview.stdout)
    assert payload["unmanaged_count"] >= 1
    assert any(
        item["path"] == "manual/" and item["entry_type"] == "bundle" and item["file_count"] == 2
        for item in payload["unmanaged"]
    )


def test_python_module_overview_groups_unmanaged_bundle_directories(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "tracked-skill")
    add_result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)
    assert add_result.returncode == 0, add_result.stderr

    unmanaged_dir = tmp_path / ".github" / "skills" / "manual" / "docs"
    unmanaged_dir.mkdir(parents=True, exist_ok=True)
    ((tmp_path / ".github" / "skills" / "manual") / "SKILL.md").write_text(
        "# manual\n", encoding="utf-8"
    )
    (unmanaged_dir / "guide.md").write_text("# guide\n", encoding="utf-8")

    overview = _run_cpm("overview", cwd=tmp_path)
    assert overview.returncode == 0, overview.stderr
    assert "manual/ (bundle directory, 2 file(s))" in overview.stdout
    assert "manual/docs/guide.md" not in overview.stdout


def test_python_module_reset_removes_managed_skill_from_disk_and_manifest(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "reset-skill")
    add_result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)
    assert add_result.returncode == 0, add_result.stderr

    reset_result = _run_cpm("reset", "--skill", "--force", cwd=tmp_path)
    assert reset_result.returncode == 0, reset_result.stderr

    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    lockfile = tomllib.loads((tmp_path / "cpm.lock").read_text(encoding="utf-8"))
    assert "reset-skill" not in manifest.get("skills", {})
    assert lockfile.get("skill", []) == []
    assert not (tmp_path / ".github" / "skills" / "reset-skill" / "SKILL.md").exists()


def test_python_module_reset_hard_removes_unmanaged_bundles_and_empty_roots(
    tmp_path: Path,
) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "tracked-skill")
    add_result = _run_cpm("add", str(skill_dir), "--skill", cwd=tmp_path)
    assert add_result.returncode == 0, add_result.stderr

    unmanaged_dir = tmp_path / ".github" / "skills" / "manual" / "docs"
    unmanaged_dir.mkdir(parents=True, exist_ok=True)
    ((tmp_path / ".github" / "skills" / "manual") / "SKILL.md").write_text(
        "# manual\n", encoding="utf-8"
    )
    (unmanaged_dir / "guide.md").write_text("# guide\n", encoding="utf-8")

    reset_result = _run_cpm("reset", "--skill", "--hard", "--force", cwd=tmp_path)
    assert reset_result.returncode == 0, reset_result.stderr
    assert "unmanaged install(s)" in reset_result.stdout
    assert not (tmp_path / ".github" / "skills").exists()


# ── uv run integration tests ──────────────────────────────────────────────────


def test_uv_run_materializes_local_skill_and_lockfile(tmp_path: Path) -> None:
    skill_dir = _make_skill_dir(tmp_path / "skills", "uv-pdf")

    result = _uv_run_project(REPO_ROOT, "cpm", "add", str(skill_dir), "--skill", cwd=tmp_path)

    assert result.returncode == 0, result.stderr
    assert (tmp_path / "cpm.lock").is_file()
    assert (tmp_path / ".github" / "skills" / "uv-pdf" / "SKILL.md").is_file()
    manifest_text = (tmp_path / "cpm.toml").read_text(encoding="utf-8")
    assert "[skills]" in manifest_text
    assert 'uv-pdf = { path = "skills/uv-pdf" }' in manifest_text
    assert "[skills.uv-pdf]" not in manifest_text


def test_uv_run_init_creates_manifest(tmp_path: Path) -> None:
    result = _uv_run_project(REPO_ROOT, "cpm", "init", "--name", "uv-test-project", cwd=tmp_path)

    assert result.returncode == 0, result.stderr
    assert (tmp_path / "cpm.toml").is_file()
    manifest = tomllib.loads((tmp_path / "cpm.toml").read_text(encoding="utf-8"))
    assert manifest["package"]["name"] == "uv-test-project"


def test_uv_run_console_script_matches_module_entrypoint_help(tmp_path: Path) -> None:
    console_result = _uv_run_project(REPO_ROOT, "cpm", "--help", cwd=tmp_path)
    module_result = _uv_run_project(REPO_ROOT, "python", "-m", "cpm", "--help", cwd=tmp_path)

    assert console_result.returncode == 0, console_result.stderr
    assert module_result.returncode == 0, module_result.stderr
    assert console_result.stdout == module_result.stdout
