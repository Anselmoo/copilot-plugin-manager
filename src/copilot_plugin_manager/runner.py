from __future__ import annotations

import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .models import InstalledPlugin


@dataclass
class CommandResult:
    args: tuple[str, ...]
    stdout: str
    stderr: str
    returncode: int


class CommandError(RuntimeError):
    def __init__(self, message: str, result: CommandResult) -> None:
        super().__init__(message)
        self.result = result


class ShellRunner:
    def which(self, name: str) -> str | None:
        return shutil.which(name)

    def require(self, name: str) -> None:
        if self.which(name) is None:
            raise RuntimeError(f"Required command not found: {name}")

    def run(self, args: list[str], cwd: Path | None = None, check: bool = True) -> CommandResult:
        proc = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
        result = CommandResult(tuple(args), proc.stdout, proc.stderr, proc.returncode)
        if check and proc.returncode != 0:
            raise CommandError(f"Command failed: {' '.join(args)}", result)
        return result


PLUGIN_LINE_RE = re.compile(r"^\s*[\u2022\-*]?\s*(?P<source>.+?)(?:\s+\((?P<version>[^)]+)\))?\s*$")


def convert_plugin_reference_to_base_name(reference: str) -> str:
    trimmed = reference.strip()
    if "@" in trimmed:
        return trimmed.split("@", 1)[0]
    return Path(trimmed).name if trimmed.count("/") >= 2 else trimmed


def parse_installed_plugins(output: str) -> list[InstalledPlugin]:
    installed: list[InstalledPlugin] = []
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("Installed plugins:"):
            continue
        match = PLUGIN_LINE_RE.match(line)
        if not match:
            continue
        source = match.group("source").strip()
        version = match.group("version")
        installed.append(
            InstalledPlugin(
                name=convert_plugin_reference_to_base_name(source),
                source=source,
                version=version,
            )
        )
    return installed
