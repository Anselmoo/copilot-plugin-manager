from __future__ import annotations

import importlib.util
import tomllib
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = ("cpm.toml", "cpm-reference.toml")


def _load_check_committed_toml_module():
    script_path = REPO_ROOT / "scripts/check_committed_toml.py"
    spec = importlib.util.spec_from_file_location("check_committed_toml", script_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _find_absolute_paths(value: object, location: str = "") -> list[str]:
    errors: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            child_location = f"{location}.{key}" if location else str(key)
            if key == "path" and isinstance(child, str) and Path(child).is_absolute():
                errors.append(f"{child_location} -> {child}")
            errors.extend(_find_absolute_paths(child, child_location))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            errors.extend(_find_absolute_paths(child, f"{location}[{index}]"))
    return errors


@pytest.mark.parametrize("manifest_name", MANIFESTS)
def test_checked_in_manifests_are_valid_and_repo_relative(manifest_name: str) -> None:
    parsed = tomllib.loads((REPO_ROOT / manifest_name).read_text(encoding="utf-8"))

    assert _find_absolute_paths(parsed) == []


def test_manifest_guardrail_script_reports_invalid_toml_and_absolute_paths(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    module = _load_check_committed_toml_module()

    (tmp_path / "cpm.toml").write_text(
        """
[skills.demo]
path = "/tmp/generated-skill"
""".strip(),
        encoding="utf-8",
    )
    (tmp_path / "cpm-reference.toml").write_text(
        """
[package]
name = "broken
""".strip(),
        encoding="utf-8",
    )

    monkeypatch.setattr(module, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(module, "MANIFESTS", MANIFESTS)

    assert module.main() == 1

    captured = capsys.readouterr()
    assert "cpm.toml: skills.demo.path must be repo-relative" in captured.err
    assert "cpm-reference.toml: invalid TOML" in captured.err


def test_manifest_guardrail_is_wired_into_local_and_ci_validation() -> None:
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    tasks = pyproject["tool"]["poe"]["tasks"]

    assert tasks["manifests-check"]["cmd"] == "python scripts/check_committed_toml.py"
    assert "manifests-check" in tasks["lint"]["sequence"]
    assert "lint" in tasks["ci"]["sequence"]
    assert "lint" in tasks["ci-full"]["sequence"]

    lefthook = (REPO_ROOT / "lefthook.yml").read_text(encoding="utf-8")
    assert "uv run --group dev python -m poethepoet manifests-check" in lefthook

    cicd_workflow = (REPO_ROOT / ".github/workflows/cicd.yml").read_text(encoding="utf-8")
    assert "uv run --group dev python -m poethepoet ci-full" in cicd_workflow
