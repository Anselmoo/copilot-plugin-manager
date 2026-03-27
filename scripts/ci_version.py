"""Compute and apply CI-only package versions for TestPyPI candidate builds.

The repository keeps a clean release version in source control (for example
``0.2.0``). For branch-based TestPyPI uploads we need a unique version per run,
because package indexes do not allow overwriting an existing file for the same
name/version.

This helper computes a published version from the GitHub Actions environment and
can also rewrite the version-bearing files in the checkout before packaging.
Python packaging metadata keeps the published PEP 440 form, while Cargo-facing
metadata is rewritten to a SemVer-compatible prerelease string.
"""

from __future__ import annotations

import argparse
import os
import re
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PYPROJECT = ROOT / "pyproject.toml"
CARGO_TOML = ROOT / "Cargo.toml"
CPM_MANIFEST = ROOT / "cpm.toml"
INIT_FILE = ROOT / "python" / "cpm" / "__init__.py"

PYPROJECT_VERSION_RE = re.compile(r'^(version\s*=\s*")([^"]+)(")', re.MULTILINE)
INIT_VERSION_RE = re.compile(r'^(__version__\s*=\s*")([^"]+)(")', re.MULTILINE)
WORKSPACE_SECTION_RE = re.compile(r'(?ms)(^\[workspace\.package\]\s.*?^version\s*=\s*")([^"]+)(")')
CPM_PACKAGE_SECTION_RE = re.compile(r'(?ms)(^\[package\]\s.*?^version\s*=\s*")([^"]+)(")')


@dataclass(frozen=True)
class GitHubContext:
    ref: str
    ref_name: str
    run_id: str
    run_attempt: str

    @classmethod
    def from_env(cls) -> GitHubContext:
        return cls(
            ref=os.environ.get("GITHUB_REF", ""),
            ref_name=os.environ.get("GITHUB_REF_NAME", ""),
            run_id=os.environ.get("GITHUB_RUN_ID", "0"),
            run_attempt=os.environ.get("GITHUB_RUN_ATTEMPT", "1"),
        )


@dataclass(frozen=True)
class AppliedVersions:
    python_package: str
    cargo_workspace: str
    repo_manifest: str
    python_module: str


def read_base_version() -> str:
    text = PYPROJECT.read_text(encoding="utf-8")
    match = PYPROJECT_VERSION_RE.search(text)
    if match is None:
        msg = "Could not find project version in pyproject.toml"
        raise RuntimeError(msg)
    return match.group(2)


def compute_published_version(base_version: str, context: GitHubContext) -> str:
    if context.ref.startswith("refs/tags/v"):
        tag_version = context.ref_name.removeprefix("v")
        return tag_version or base_version

    if context.ref == "refs/heads/main":
        attempt = int(context.run_attempt)
        return f"{base_version}.dev{context.run_id}{attempt:02d}"

    return base_version


def to_semver(version: str) -> str:
    """Convert a PEP 440 dev release into a Cargo-compatible SemVer prerelease."""
    return re.sub(r"\.dev(?P<build>\d+)$", r"-dev.\g<build>", version)


def build_applied_versions(version: str) -> AppliedVersions:
    cargo_version = to_semver(version)
    return AppliedVersions(
        python_package=version,
        cargo_workspace=cargo_version,
        repo_manifest=cargo_version,
        python_module=version,
    )


def _replace_once(path: Path, pattern: re.Pattern[str], new: str, *, dry_run: bool) -> None:
    text = path.read_text(encoding="utf-8")
    updated = pattern.sub(rf"\g<1>{new}\g<3>", text, count=1)
    if text == updated:
        print(f"  = {path.relative_to(ROOT)} already set to {new}")
        return
    if dry_run:
        print(f'  [dry-run] Would update {path.relative_to(ROOT)}: version = "{new}"')
        return
    path.write_text(updated, encoding="utf-8")
    print(f'  [updated] {path.relative_to(ROOT)} -> version = "{new}"')


def apply_version(version: str, *, dry_run: bool) -> None:
    applied = build_applied_versions(version)
    _replace_once(PYPROJECT, PYPROJECT_VERSION_RE, applied.python_package, dry_run=dry_run)
    _replace_once(CARGO_TOML, WORKSPACE_SECTION_RE, applied.cargo_workspace, dry_run=dry_run)
    _replace_once(CPM_MANIFEST, CPM_PACKAGE_SECTION_RE, applied.repo_manifest, dry_run=dry_run)
    _replace_once(INIT_FILE, INIT_VERSION_RE, applied.python_module, dry_run=dry_run)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Compute or apply CI-only package versions for publishing.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    compute = subparsers.add_parser("compute", help="Print the published version for this run.")
    compute.set_defaults(func=cmd_compute)

    apply = subparsers.add_parser("apply", help="Apply a concrete version to repo metadata.")
    apply.add_argument("version", help="Version string to apply to package metadata files.")
    apply.add_argument("--dry-run", action="store_true", help="Preview file edits only.")
    apply.set_defaults(func=cmd_apply)

    sync = subparsers.add_parser(
        "sync",
        help="Compute the published version from GitHub Actions env and apply it.",
    )
    sync.add_argument("--dry-run", action="store_true", help="Preview file edits only.")
    sync.set_defaults(func=cmd_sync)

    return parser


def cmd_compute(_: argparse.Namespace) -> None:
    version = compute_published_version(read_base_version(), GitHubContext.from_env())
    print(version)


def cmd_apply(args: argparse.Namespace) -> None:
    apply_version(args.version, dry_run=args.dry_run)


def cmd_sync(args: argparse.Namespace) -> None:
    version = compute_published_version(read_base_version(), GitHubContext.from_env())
    print(f"Applying published version: {version}")
    apply_version(version, dry_run=args.dry_run)


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
