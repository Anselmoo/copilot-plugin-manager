import subprocess
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

from typer.testing import CliRunner

runner = CliRunner()


def load_check_broken_links_module():
    module_path = Path(__file__).resolve().parents[1] / "scripts" / "check_broken_links.py"
    spec = spec_from_file_location("check_broken_links", module_path)
    assert spec is not None and spec.loader is not None
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_find_dangling_symlinks_reports_only_broken_links(tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    target = tmp_path / "target.txt"
    target.write_text("ok", encoding="utf-8")
    (tmp_path / "valid-link").symlink_to(target.name)
    (tmp_path / "broken-link").symlink_to("missing.txt")
    (tmp_path / "plain.txt").write_text("plain", encoding="utf-8")

    broken = checker.find_dangling_symlinks(tmp_path, [Path("valid-link"), Path("broken-link"), Path("plain.txt")])

    assert broken == [Path("broken-link")]


def test_find_dangling_symlinks_accepts_resolvable_microsoft_wrapper_symlinks(tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    wrapper = tmp_path / "external" / "microsoft-skills" / "skills" / "python" / "data"
    wrapper.mkdir(parents=True)
    (wrapper / "blob").symlink_to("../../../.github/skills/azure-storage-blob-py")
    real_skill = tmp_path / "external" / "microsoft-skills" / ".github" / "plugins" / "azure-sdk-python" / "skills" / "azure-storage-blob-py"
    real_skill.mkdir(parents=True)

    broken = checker.find_dangling_symlinks(tmp_path, [Path("external/microsoft-skills/skills/python/data/blob")])

    assert broken == []


def test_list_repo_paths_includes_initialized_submodules(monkeypatch, tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    submodule = tmp_path / "external" / "sample"

    monkeypatch.setattr(checker, "list_initialized_submodules", lambda root: [submodule])
    monkeypatch.setattr(
        checker,
        "list_git_paths",
        lambda root, git_dir, prefix: [Path("root-link")] if prefix == Path() else [prefix / "nested-link"],
    )

    candidates = checker.list_repo_paths(tmp_path)

    assert candidates == [Path("root-link"), Path("external/sample/nested-link")]


def test_list_git_paths_uses_sanitized_git_env(monkeypatch, tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    captured_env: dict[str, str] | None = None

    def fake_run(*_args, **kwargs):
        nonlocal captured_env
        captured_env = kwargs.get("env")
        return subprocess.CompletedProcess(args=[], returncode=0, stdout=b"tracked\0", stderr=b"")

    monkeypatch.setattr(checker.subprocess, "run", fake_run)
    monkeypatch.setenv("GIT_DIR", "/tmp/fake-dot-git")
    monkeypatch.setenv("GIT_WORK_TREE", "/tmp/fake-work-tree")

    result = checker.list_git_paths(tmp_path, tmp_path, Path())

    assert result == [Path("tracked")]
    assert captured_env is not None
    assert "GIT_DIR" not in captured_env
    assert "GIT_WORK_TREE" not in captured_env


def test_cli_exits_non_zero_when_broken_links_are_found(monkeypatch, tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    (tmp_path / "broken-link").symlink_to("missing.txt")
    monkeypatch.setattr(checker, "list_repo_paths", lambda root: [Path("broken-link")])

    result = runner.invoke(checker.app, ["--root", str(tmp_path)])

    assert result.exit_code == 1
    assert "Found 1 broken symlink" in result.stdout
    assert "broken-link" in result.stdout


def test_cli_succeeds_when_no_broken_links_exist(monkeypatch, tmp_path: Path) -> None:
    checker = load_check_broken_links_module()
    target = tmp_path / "target.txt"
    target.write_text("ok", encoding="utf-8")
    (tmp_path / "valid-link").symlink_to(target.name)
    monkeypatch.setattr(checker, "list_repo_paths", lambda root: [Path("valid-link")])

    result = runner.invoke(checker.app, ["--root", str(tmp_path)])

    assert result.exit_code == 0
    assert "No broken symlinks found." in result.stdout
