"""Conventional-commit branch helper for this repository.

Usage examples::

    uv run python scripts/branch.py new feat "add multi-group membership"
    uv run python scripts/branch.py new fix "auth guidance" --scope cli
    uv run python scripts/branch.py rescue fix "recover release work"
    uv run python scripts/branch.py rescue feat "recover partial work" --since abc1234

The script is intentionally shell-free so it behaves consistently on macOS,
Linux, and Windows.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONVENTIONAL_TYPES = (
    "feat",
    "fix",
    "chore",
    "docs",
    "refactor",
    "test",
    "ci",
    "perf",
    "style",
    "build",
)
_SLUG_MAX = 60


@dataclass
class BranchName:
    type: str
    description: str
    scope: str | None = None

    def slug(self) -> str:
        raw = f"{self.scope}-{self.description}" if self.scope else self.description
        slug = re.sub(r"[^a-z0-9]+", "-", raw.lower()).strip("-")
        slug = slug[:_SLUG_MAX].rstrip("-")
        return f"{self.type}/{slug}"

    def commit_title(self) -> str:
        scope_part = f"({self.scope})" if self.scope else ""
        return f"{self.type}{scope_part}: {self.description}"


def _run(cmd: list[str], *, dry_run: bool, label: str) -> str:
    pretty = " ".join(cmd)
    if dry_run:
        print(f"  [dry-run] Would run: {pretty}")
        return ""
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
    return result.stdout.strip()


def _capture(cmd: list[str]) -> str:
    return subprocess.run(
        cmd,
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()


def _branch_exists(branch: str) -> bool:
    return bool(_capture(["git", "branch", "--list", branch]))


def _current_branch() -> str:
    return _capture(["git", "branch", "--show-current"])


def _working_tree_clean() -> bool:
    return _capture(["git", "status", "--porcelain"]) == ""


def _commits_ahead(base_ref: str) -> list[str]:
    out = _capture(["git", "log", f"{base_ref}..HEAD", "--pretty=format:%h %s"])
    return [line for line in out.splitlines() if line]


def _normalize_commit_type(value: str) -> str:
    normalized = value.lower()
    if normalized not in CONVENTIONAL_TYPES:
        allowed = ", ".join(CONVENTIONAL_TYPES)
        raise argparse.ArgumentTypeError(
            f"invalid conventional type: {value!r} (choose one of: {allowed})"
        )
    return normalized


def _join_description(parts: list[str]) -> str:
    if not (description := " ".join(parts).strip()):
        raise argparse.ArgumentTypeError("description must not be empty")
    return description


def _add_common_branch_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("type", type=_normalize_commit_type, metavar="TYPE")
    parser.add_argument("description", nargs="+", help="Short branch description.")
    parser.add_argument("--scope", metavar="SCOPE", default=None, help="Optional scope.")
    parser.add_argument("--dry-run", action="store_true", help="Preview without touching git.")


def cmd_new(args: argparse.Namespace) -> None:
    description = _join_description(args.description)
    branch = BranchName(type=args.type, description=description, scope=args.scope)
    branch_name = branch.slug()
    commit_title = branch.commit_title()

    base = "<current>" if args.dry_run else _current_branch()
    print(f"\n{'[DRY RUN] ' if args.dry_run else ''}New branch from '{base}'")
    print(f"  Branch : {branch_name}")
    print(f"  Title  : {commit_title}\n")

    if not args.dry_run and _branch_exists(branch_name):
        print(
            (
                f"⚠  Branch '{branch_name}' already exists. "
                "Delete it first or choose a different description."
            ),
            file=sys.stderr,
        )
        sys.exit(1)

    has_changes = not args.dry_run and not _working_tree_clean()

    print("── Creating branch ───────────────────────────────────────────────")
    _run(["git", "checkout", "-b", branch_name], dry_run=args.dry_run, label="git checkout -b")

    if has_changes:
        print("→  Uncommitted changes moved to the new branch.\n")

    print(f"✅ Done!  Suggested commit title:\n\n    {commit_title}\n")
    if args.dry_run:
        print("[dry-run complete — no changes made]")


def cmd_rescue(args: argparse.Namespace) -> None:
    description = _join_description(args.description)
    branch = BranchName(type=args.type, description=description, scope=args.scope)
    branch_name = branch.slug()
    commit_title = branch.commit_title()

    origin_branch = "main" if args.dry_run else _current_branch()
    print(
        f"\n{'[DRY RUN] ' if args.dry_run else ''}"
        f"Rescue commits from '{origin_branch}' → '{branch_name}'\n"
    )

    if args.since:
        log_lines = [] if args.dry_run else _commits_ahead(args.since)
        reset_target = args.since
    else:
        remote_ref = f"origin/{origin_branch}"
        log_lines = [] if args.dry_run else _commits_ahead(remote_ref)
        reset_target = remote_ref

    if not log_lines and not args.dry_run:
        ref_label = args.since or f"origin/{origin_branch}"
        print(
            (
                f"⚠  No commits found ahead of '{ref_label}'. "
                "Nothing to rescue. Use --since <sha> to override."
            ),
            file=sys.stderr,
        )
        sys.exit(1)

    print("── Commits to rescue ─────────────────────────────────────────────")
    if log_lines:
        for line in log_lines:
            print(f"  {line}")
    else:
        ahead = args.since or f"origin/{origin_branch}"
        print(f"  [dry-run] Would detect commits ahead of {ahead}")

    if not args.dry_run and _branch_exists(branch_name):
        print(
            (
                f"⚠  Branch '{branch_name}' already exists. "
                "Delete it first or choose a different description."
            ),
            file=sys.stderr,
        )
        sys.exit(1)

    print("\n── Creating rescue branch ───────────────────────────────────────")
    _run(
        ["git", "checkout", "-b", branch_name],
        dry_run=args.dry_run,
        label="git checkout -b rescue",
    )

    print("\n── Resetting origin branch ──────────────────────────────────────")
    _run(["git", "checkout", origin_branch], dry_run=args.dry_run, label="git checkout origin")
    _run(["git", "reset", "--hard", reset_target], dry_run=args.dry_run, label="git reset --hard")

    print("\n── Switching back to rescue branch ──────────────────────────────")
    _run(["git", "checkout", branch_name], dry_run=args.dry_run, label="git checkout rescue")

    rescued_count = "Selected" if args.dry_run else str(len(log_lines))
    print(
        f"\n✅ Done!  {rescued_count} commit(s) rescued into '{branch_name}'.\n"
        f"   '{origin_branch}' has been reset to '{reset_target}'.\n"
        f"   Suggested commit title:\n\n    {commit_title}\n"
    )
    if args.dry_run:
        print("[dry-run complete — no changes made]")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Conventional commits branch helper.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    sub = parser.add_subparsers(dest="command", required=True)

    new_p = sub.add_parser(
        "new",
        help="Create a new conventionally named branch from the current HEAD.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    _add_common_branch_arguments(new_p)
    new_p.set_defaults(func=cmd_new)

    rescue_p = sub.add_parser(
        "rescue",
        help="Move commits to a new branch and reset the current branch.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    _add_common_branch_arguments(rescue_p)
    rescue_p.add_argument(
        "--since",
        metavar="SHA",
        default=None,
        help="Rescue commits since this SHA instead of origin/<branch>.",
    )
    rescue_p.set_defaults(func=cmd_rescue)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
