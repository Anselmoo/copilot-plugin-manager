from __future__ import annotations

import os
import subprocess
from pathlib import Path

import typer

from copilot_plugin_manager.rendering import console

ROOT = Path(__file__).resolve().parent.parent
app = typer.Typer(add_completion=False, help="Detect dangling symlinks in the repository and initialized submodules.")
GIT_ENV_DENYLIST = (
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
)


def sanitized_git_env() -> dict[str, str]:
    return {key: value for key, value in os.environ.items() if key not in GIT_ENV_DENYLIST}


def list_repo_paths(root: Path) -> list[Path]:
    candidates = list_git_paths(root, root, Path())
    for submodule in list_initialized_submodules(root):
        candidates.extend(list_git_paths(root, submodule, submodule.relative_to(root)))
    return candidates


def list_git_paths(root: Path, git_dir: Path, prefix: Path) -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(git_dir),
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        check=True,
        capture_output=True,
        env=sanitized_git_env(),
    )
    return [prefix / item.decode("utf-8") for item in result.stdout.split(b"\0") if item]


def list_initialized_submodules(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "-C", str(root), "submodule", "status", "--recursive"],
        check=True,
        capture_output=True,
        env=sanitized_git_env(),
    )
    submodules: list[Path] = []
    for line in result.stdout.decode("utf-8", errors="replace").splitlines():
        parts = line.strip().split()
        if len(parts) < 2:
            continue
        submodule = root / parts[1]
        if submodule.is_dir():
            submodules.append(submodule)
    return submodules


def resolve_microsoft_wrapper_target(root: Path, full_path: Path) -> Path | None:
    relative = full_path.relative_to(root)
    parts = relative.parts
    if len(parts) < 4 or parts[:2] != ("external", "microsoft-skills") or parts[2] != "skills" or not full_path.is_symlink():
        return None
    microsoft_root = root / "external" / "microsoft-skills"
    target_name = full_path.readlink().name
    matches: list[Path] = []
    direct_skill = microsoft_root / ".github" / "skills" / target_name
    if direct_skill.is_dir():
        matches.append(direct_skill)
    plugin_root = microsoft_root / ".github" / "plugins"
    if plugin_root.exists():
        matches.extend(sorted(path for path in plugin_root.glob(f"*/skills/{target_name}") if path.is_dir()))
    if len(matches) != 1:
        return None
    return matches[0]


def symlink_is_effectively_resolved(root: Path, full_path: Path) -> bool:
    if full_path.exists():
        return True
    return resolve_microsoft_wrapper_target(root, full_path) is not None


def find_dangling_symlinks(root: Path, candidates: list[Path]) -> list[Path]:
    broken: list[Path] = []
    seen: set[Path] = set()
    for candidate in candidates:
        if candidate in seen:
            continue
        seen.add(candidate)
        full_path = root / candidate
        if full_path.is_symlink() and not symlink_is_effectively_resolved(root, full_path):
            broken.append(candidate)
    return sorted(broken, key=lambda path: path.as_posix())


@app.command()
def main(
    root: Path = typer.Option(
        ROOT,
        "--root",
        file_okay=False,
        dir_okay=True,
        exists=True,
        resolve_path=True,
        help="Repository root to inspect.",
    ),
) -> None:
    term = console()
    term.print(f"[bold]Checking for broken symlinks[/bold] in {root}")
    try:
        candidates = list_repo_paths(root)
    except subprocess.CalledProcessError as exc:
        stderr = exc.stderr.decode("utf-8", errors="replace").strip()
        if stderr:
            term.print(f"[bold red]git ls-files failed:[/bold red] {stderr}")
        raise typer.Exit(code=2) from exc

    broken = find_dangling_symlinks(root, candidates)
    if broken:
        term.print(f"[bold red]Found {len(broken)} broken symlink(s):[/bold red]")
        for path in broken:
            term.print(f" - {path.as_posix()}")
        raise typer.Exit(code=1)

    term.print("[bold green]No broken symlinks found.[/bold green]")


if __name__ == "__main__":
    app()
