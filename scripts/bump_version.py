"""Bump repository version metadata and prepare a release branch.

Usage examples::

    uv run python scripts/bump_version.py patch
    uv run python scripts/bump_version.py minor --dry-run
    uv run python scripts/bump_version.py 1.2.3 --no-changelog

This helper updates the version in the files that define the shipped package,
refreshes ``uv.lock``, and optionally creates a ``release/vX.Y.Z`` branch.
The implementation avoids shell-specific behavior so it remains portable across
macOS, Linux, and Windows.
"""

from __future__ import annotations

import argparse
import datetime
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PYPROJECT = ROOT / "pyproject.toml"
CARGO_TOML = ROOT / "Cargo.toml"
CPM_MANIFEST = ROOT / "cpm.toml"
INIT_FILE = ROOT / "python" / "cpm" / "__init__.py"
CHANGELOG = ROOT / "CHANGELOG.md"
UV_LOCK = ROOT / "uv.lock"

SEMVER_PARTS = 3
CHANGELOG_PREVIEW_LINES = 8

PYPROJECT_VERSION_RE = re.compile(r'^(version\s*=\s*")([^"]+)(")', re.MULTILINE)
INIT_VERSION_RE = re.compile(r'^(__version__\s*=\s*")([^"]+)(")', re.MULTILINE)
WORKSPACE_SECTION_RE = re.compile(r'(?ms)(^\[workspace\.package\]\s.*?^version\s*=\s*")([^"]+)(")')
CPM_PACKAGE_SECTION_RE = re.compile(r'(?ms)(^\[package\]\s.*?^version\s*=\s*")([^"]+)(")')
_CONV_RE = re.compile(
    r"^(?P<type>feat|fix|docs|style|refactor|perf|test|build|ci|chore|deps)"
    r"(?:\((?P<scope>[^)]+)\))?"
    r"(?P<breaking>!)?"
    r"\s*:\s*(?P<desc>.+)$",
    re.IGNORECASE,
)
_SECTION_MAP: dict[str, str] = {
    "feat": "Added",
    "fix": "Fixed",
    "refactor": "Changed",
    "perf": "Changed",
    "style": "Changed",
    "docs": "Documentation",
    "chore": "Maintenance",
    "ci": "Maintenance",
    "build": "Maintenance",
    "test": "Maintenance",
    "deps": "Maintenance",
}
_SECTION_ORDER = [
    "Breaking Changes",
    "Added",
    "Fixed",
    "Changed",
    "Documentation",
    "Maintenance",
]


@dataclass
class Version:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, raw: str) -> Version:
        parts = raw.strip().split(".")
        if len(parts) != SEMVER_PARTS or not all(part.isdigit() for part in parts):
            raise ValueError(f"Invalid semver: {raw!r}")
        return cls(int(parts[0]), int(parts[1]), int(parts[2]))

    def bump(self, kind: str) -> Version:
        if kind == "major":
            return Version(self.major + 1, 0, 0)
        if kind == "minor":
            return Version(self.major, self.minor + 1, 0)
        if kind == "patch":
            return Version(self.major, self.minor, self.patch + 1)
        raise ValueError(f"Unknown bump kind: {kind!r}")

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclass
class ParsedCommit:
    sha: str
    type: str
    scope: str | None
    description: str
    breaking: bool = False


def _replace_once(path: Path, pattern: re.Pattern[str], new: str, *, dry_run: bool) -> None:
    text = path.read_text(encoding="utf-8")
    updated = pattern.sub(rf"\g<1>{new}\g<3>", text, count=1)
    if text == updated:
        raise RuntimeError(f"{path.relative_to(ROOT)} version replacement had no effect")
    if dry_run:
        print(f'  [dry-run] Would update {path.relative_to(ROOT)}: version = "{new}"')
        return
    path.write_text(updated, encoding="utf-8")
    print(f'  ✓ {path.relative_to(ROOT)}  →  version = "{new}"')


def _update_pyproject(new: Version, *, dry_run: bool) -> None:
    _replace_once(PYPROJECT, PYPROJECT_VERSION_RE, str(new), dry_run=dry_run)


def _update_workspace_cargo(new: Version, *, dry_run: bool) -> None:
    _replace_once(CARGO_TOML, WORKSPACE_SECTION_RE, str(new), dry_run=dry_run)


def _update_cpm_manifest(new: Version, *, dry_run: bool) -> None:
    _replace_once(CPM_MANIFEST, CPM_PACKAGE_SECTION_RE, str(new), dry_run=dry_run)


def _update_init(new: Version, *, dry_run: bool) -> None:
    text = INIT_FILE.read_text(encoding="utf-8")
    updated = INIT_VERSION_RE.sub(rf"\g<1>{new}\g<3>", text, count=1)
    if text == updated:
        print(f"  ⚠ No __version__ pattern found in {INIT_FILE.relative_to(ROOT)} — skipping")
        return
    if dry_run:
        print(f'  [dry-run] Would update {INIT_FILE.relative_to(ROOT)}: __version__ = "{new}"')
        return
    INIT_FILE.write_text(updated, encoding="utf-8")
    print(f'  ✓ {INIT_FILE.relative_to(ROOT)}  →  __version__ = "{new}"')


def _read_current_version() -> Version:
    text = PYPROJECT.read_text(encoding="utf-8")
    match = PYPROJECT_VERSION_RE.search(text)
    if match is None:
        raise RuntimeError("Could not find project version in pyproject.toml")
    return Version.parse(match.group(2))


def _git_log_since_tag() -> list[tuple[str, str]]:
    tag_result = subprocess.run(
        ["git", "tag", "--sort=-v:refname"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    tags = [tag.strip() for tag in tag_result.stdout.splitlines() if tag.strip()]
    ref = f"{tags[0]}..HEAD" if tags else "HEAD"
    log_result = subprocess.run(
        ["git", "log", ref, "--pretty=format:%H\t%s"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    pairs: list[tuple[str, str]] = []
    for line in log_result.stdout.splitlines():
        if "\t" in line:
            sha, subject = line.split("\t", 1)
            pairs.append((sha.strip(), subject.strip()))
    return pairs


def _parse_conventional_commit(sha: str, subject: str) -> ParsedCommit | None:
    if subject.startswith("Merge ") or subject.lower().startswith("release:"):
        return None
    match = _CONV_RE.match(subject)
    if match is None:
        return None
    return ParsedCommit(
        sha=sha,
        type=match.group("type").lower(),
        scope=match.group("scope"),
        description=match.group("desc").strip(),
        breaking=bool(match.group("breaking")),
    )


def _build_changelog_section(
    version: Version,
    commits: list[tuple[str, str]],
    *,
    include_maintenance: bool,
) -> str:
    sections: dict[str, list[str]] = {section: [] for section in _SECTION_ORDER}

    for sha, subject in commits:
        parsed = _parse_conventional_commit(sha, subject)
        if parsed is None:
            continue
        section = "Breaking Changes" if parsed.breaking else _SECTION_MAP.get(parsed.type)
        if section is None:
            continue
        scope_part = f"**{parsed.scope}**: " if parsed.scope else ""
        sections[section].append(f"- {scope_part}{parsed.description}")

    today = datetime.datetime.now(datetime.UTC).date().isoformat()
    lines = [f"## [{version}] - {today}", ""]
    rendered_any = False
    for section_name in _SECTION_ORDER:
        entries = sections[section_name]
        if not entries:
            continue
        if section_name == "Maintenance" and not include_maintenance:
            continue
        lines.append(f"### {section_name}")
        lines.extend(entries)
        lines.append("")
        rendered_any = True

    if not rendered_any:
        lines.extend(("_No notable changes recorded._", ""))
    return "\n".join(lines)


def _update_changelog(
    new: Version,
    commits: list[tuple[str, str]],
    *,
    include_maintenance: bool,
    dry_run: bool,
) -> None:
    if not CHANGELOG.exists():
        print("  ⚠ CHANGELOG.md not found — skipping")
        return
    section = _build_changelog_section(
        new,
        commits,
        include_maintenance=include_maintenance,
    )
    if dry_run:
        print(f"  [dry-run] Would prepend to {CHANGELOG.name}:")
        for line in section.splitlines()[:CHANGELOG_PREVIEW_LINES]:
            print(f"    {line}")
        if len(section.splitlines()) > CHANGELOG_PREVIEW_LINES:
            print("    …")
        return
    existing = CHANGELOG.read_text(encoding="utf-8")
    CHANGELOG.write_text(section + "\n" + existing, encoding="utf-8")
    print(f"  ✓ {CHANGELOG.name} updated")


def _run(cmd: list[str], *, dry_run: bool, label: str) -> None:
    pretty = " ".join(cmd)
    if dry_run:
        print(f"  [dry-run] Would run: {pretty}")
        return
    print(f"  $ {pretty}")
    result = subprocess.run(
        cmd,
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        if result.stdout.strip():
            print(result.stdout.strip())
        if result.stderr.strip():
            print(result.stderr.strip(), file=sys.stderr)
        raise RuntimeError(f"{label} failed (exit {result.returncode})")
    if result.stdout.strip():
        print(result.stdout.strip())


def _branch_exists(branch: str) -> bool:
    result = subprocess.run(
        ["git", "branch", "--list", branch],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return bool(result.stdout.strip())


def _current_branch() -> str:
    result = subprocess.run(
        ["git", "branch", "--show-current"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip()


def _working_tree_clean() -> bool:
    result = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() == ""


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Bump version metadata, refresh lockfiles, and create a release branch.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "bump",
        metavar="BUMP",
        help="Bump kind: major | minor | patch, or an explicit semver like 1.2.3",
    )
    parser.add_argument("--dry-run", action="store_true", help="Preview without writing changes.")
    parser.add_argument("--no-commit", action="store_true", help="Skip the git commit step.")
    parser.add_argument(
        "--no-changelog",
        action="store_true",
        help="Skip updating CHANGELOG.md if it exists.",
    )
    parser.add_argument(
        "--include-maintenance",
        action="store_true",
        help="Include chore/ci/build/test entries in changelog output.",
    )
    parser.add_argument(
        "--base-branch",
        default=None,
        metavar="BRANCH",
        help="Branch to create the release branch from (default: current branch).",
    )
    args = parser.parse_args()

    current = _read_current_version()
    if args.bump in {"major", "minor", "patch"}:
        new = current.bump(args.bump)
    else:
        try:
            new = Version.parse(args.bump)
        except ValueError as exc:
            parser.error(str(exc))

    branch_name = f"release/v{new}"
    print(f"\n{'[DRY RUN] ' if args.dry_run else ''}Version bump: {current}  →  {new}")
    print(f"Target branch: {branch_name}\n")

    base = args.base_branch or ("<current>" if args.dry_run else _current_branch())
    if not args.dry_run:
        if not _working_tree_clean():
            print(
                (
                    "⚠  Working tree has uncommitted changes. Commit or stash them first, "
                    "or use --dry-run."
                ),
                file=sys.stderr,
            )
            sys.exit(1)
        if _branch_exists(branch_name):
            print(
                (
                    f"⚠  Branch '{branch_name}' already exists. "
                    "Delete it first or choose a different version."
                ),
                file=sys.stderr,
            )
            sys.exit(1)
        if _current_branch() != base:
            _run(["git", "checkout", base], dry_run=False, label="git checkout base")

    print("── Updating version strings ──────────────────────────────────")
    _update_pyproject(new, dry_run=args.dry_run)
    _update_workspace_cargo(new, dry_run=args.dry_run)
    _update_cpm_manifest(new, dry_run=args.dry_run)
    _update_init(new, dry_run=args.dry_run)

    if not args.no_changelog:
        print("\n── Updating CHANGELOG.md ─────────────────────────────────────")
        commits = _git_log_since_tag()
        _update_changelog(
            new,
            commits,
            include_maintenance=args.include_maintenance,
            dry_run=args.dry_run,
        )

    print("\n── Refreshing lockfiles ─────────────────────────────────────")
    _run(["uv", "lock", "-U"], dry_run=args.dry_run, label="uv lock -U")

    if not UV_LOCK.exists():
        print("  ⚠ uv.lock not found after refresh — skipping stage hint")

    print("\n── Git ───────────────────────────────────────────────────────")
    _run(["git", "checkout", "-b", branch_name], dry_run=args.dry_run, label="git checkout -b")

    files_to_stage = [
        str(PYPROJECT.relative_to(ROOT)),
        str(CARGO_TOML.relative_to(ROOT)),
        str(CPM_MANIFEST.relative_to(ROOT)),
        str(INIT_FILE.relative_to(ROOT)),
    ]
    if UV_LOCK.exists():
        files_to_stage.append(str(UV_LOCK.relative_to(ROOT)))
    if not args.no_changelog and CHANGELOG.exists():
        files_to_stage.append(str(CHANGELOG.relative_to(ROOT)))

    _run(["git", "add", *files_to_stage], dry_run=args.dry_run, label="git add")

    if not args.no_commit:
        _run(["git", "add", "-u"], dry_run=args.dry_run, label="git add -u")
        commit_msg = f"chore: bump version to v{new}"
        _run(["git", "commit", "-m", commit_msg], dry_run=args.dry_run, label="git commit")
        print(f"\n✅ Done!  Branch '{branch_name}' created with commit: {commit_msg!r}")
    else:
        print(f"\n✅ Done (no commit)!  Branch '{branch_name}' created, files staged.")

    if args.dry_run:
        print(f"Base branch: {base}")
        print("[dry-run complete — no files were modified]")


if __name__ == "__main__":
    main()
