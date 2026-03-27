from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path

PathLookup = Callable[[str], str | None]


@dataclass(frozen=True)
class DelegateSpec:
    command: list[str]
    cwd: Path | None = None


def _cargo_delegate(repo_root: Path, args: Sequence[str]) -> DelegateSpec:
    return DelegateSpec(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(repo_root / "Cargo.toml"),
            "-p",
            "cpm-cli",
            "--",
            *args,
        ]
    )


def find_repo_root(start: Path | None = None) -> Path | None:
    candidates: list[Path] = []
    if start is not None:
        candidates.append(start)
    candidates.extend((Path.cwd(), Path(__file__).resolve()))

    for candidate in candidates:
        current = candidate if candidate.is_dir() else candidate.parent
        for parent in (current, *current.parents):
            cli_manifest = parent / "crates/cpm-cli/Cargo.toml"
            if (parent / "Cargo.toml").is_file() and cli_manifest.is_file():
                return parent
    return None


def _resolve_path_binary(
    *,
    current_executable: Path | None,
    path_lookup: PathLookup,
) -> Path | None:
    binary = path_lookup("cpm")
    if binary is None:
        return None

    resolved = Path(binary).resolve()
    if current_executable is not None and resolved == current_executable.resolve():
        return None
    return resolved


def resolve_delegate(
    args: Sequence[str],
    *,
    prefer_cargo: bool = False,
    repo_root: Path | None = None,
    current_executable: Path | None = None,
    path_lookup: PathLookup = shutil.which,
) -> DelegateSpec:
    workspace_root = repo_root or find_repo_root()
    if prefer_cargo:
        if workspace_root is not None:
            return _cargo_delegate(workspace_root, args)
        raise RuntimeError(
            "Cargo delegation was requested, but no cpm source checkout was found. "
            "Run from a repository checkout containing crates/cpm-cli or unset --cargo."
        )

    configured_binary = os.environ.get("CPM_BIN")
    if configured_binary:
        return DelegateSpec([configured_binary, *args])

    if workspace_root is not None:
        return _cargo_delegate(workspace_root, args)

    binary = _resolve_path_binary(
        current_executable=current_executable,
        path_lookup=path_lookup,
    )
    if binary is not None:
        return DelegateSpec([str(binary), *args])

    raise RuntimeError(
        "Unable to locate the Rust `cpm` CLI. Run `cargo build -p cpm-cli` from a source checkout "
        "or set the CPM_BIN environment variable."
    )


def invoke_delegate(args: Sequence[str], *, prefer_cargo: bool = False) -> int:
    current_executable = Path(sys.argv[0]).resolve() if sys.argv and sys.argv[0] else None
    delegate = resolve_delegate(
        args,
        prefer_cargo=prefer_cargo,
        current_executable=current_executable,
    )
    completed = subprocess.run(delegate.command, cwd=delegate.cwd, check=False)
    return completed.returncode
