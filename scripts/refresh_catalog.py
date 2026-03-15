from __future__ import annotations

import re
import subprocess
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath

import typer
from rich.progress import BarColumn, Progress, SpinnerColumn, TaskProgressColumn, TextColumn, TimeElapsedColumn

from copilot_plugin_manager.rendering import console

ROOT = Path(__file__).resolve().parent.parent
CATALOG_DIR = ROOT / "src" / "copilot_plugin_manager" / "catalog_data"
PLUGINS_PATH = CATALOG_DIR / "plugins.toml"
ENTRYPOINTS_PATH = CATALOG_DIR / "entrypoints.toml"
REPOSITORIES_PATH = CATALOG_DIR / "repositories.toml"
SKILLS_PATH = CATALOG_DIR / "skills.toml"
AGENTS_PATH = CATALOG_DIR / "agents.toml"
app = typer.Typer(add_completion=False, help="Refresh bundled catalog metadata from the tracked upstream sources.")


def load_toml(path: Path) -> dict[str, object]:
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        return {}
    return tomllib.loads(text)


def quote_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def quote_key(value: str) -> str:
    return quote_string(value)


def format_array(values: list[object]) -> str:
    return "[" + ", ".join(quote_string(str(value)) for value in values) + "]"


def dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            ordered.append(value)
    return ordered


def slug_to_title(value: str) -> str:
    parts = [part for part in re.split(r"[-_/]", value) if part]
    return " ".join(part.upper() if len(part) <= 3 else part[:1].upper() + part[1:] for part in parts)


def classify_tags(text: str) -> list[str]:
    rules = [
        (r"plugin|awesome-copilot", ["plugins"]),
        (r"security|audit|protect", ["security"]),
        (r"planning|spike|autonomy|context", ["planning", "orchestration"]),
        (r"python", ["python", "language"]),
        (r"typescript|javascript|react|frontend|ui|design|winui|vue|angular", ["frontend", "ui"]),
        (r"java|ruby|rust|swift|go|php|kotlin|csharp|clojure", ["language"]),
        (r"openapi|mcp", ["api", "mcp"]),
        (r"database|postgres|oracle|data|science|research|bi", ["data"]),
        (r"doc|pdf|ppt|xlsx|canvas|brand|theme", ["docs", "artifacts"]),
        (r"github|git", ["github"]),
        (r"devops|docker|kubernetes|platform|infra", ["devops"]),
        (r"agent|subagent", ["agents"]),
        (r"skill", ["skills"]),
    ]
    tags: list[str] = []
    for pattern, additions in rules:
        if re.search(pattern, text, re.IGNORECASE):
            tags.extend(additions)
    return dedupe(tags or ["general"])


def provider_tags(name: str, kind: str, source_name: str) -> list[str]:
    tags = [kind]
    tags.extend(classify_tags(name))
    if source_name.startswith("anthropic"):
        tags.append("anthropic")
    if source_name.startswith("microsoft"):
        tags.append("microsoft")
    if "voltagent" in source_name:
        tags.append("voltagent")
    if "agency" in source_name:
        tags.append("agency")
    return dedupe(tags)


def clean_markdown_line(line: str) -> str:
    line = re.sub(r"`([^`]+)`", r"\1", line)
    line = re.sub(r"\[([^\]]+)\]\([^\)]+\)", r"\1", line)
    line = re.sub(r"\*\*([^*]+)\*\*", r"\1", line)
    line = re.sub(r"\*([^*]+)\*", r"\1", line)
    return " ".join(line.split()).strip()


def frontmatter_fields(lines: list[str]) -> dict[str, str]:
    if not lines or lines[0].strip() != "---":
        return {}
    fields: dict[str, str] = {}
    for line in lines[1:]:
        stripped = line.strip()
        if stripped == "---":
            break
        if ":" not in stripped:
            continue
        key, value = stripped.split(":", 1)
        fields[key.strip().lower()] = value.strip()
    return fields


def _looks_like_date(value: str) -> bool:
    candidate = value.strip()
    return bool(re.search(r"\d{4}[-/]\d{2}[-/]\d{2}|\d{2}[-/]\d{2}[-/]\d{4}", candidate))


def approval_date_from_lines(lines: list[str], frontmatter: dict[str, str]) -> str | None:
    for key in (
        "approval_date",
        "approved_at",
        "approved_on",
        "date_approved",
        "last_reviewed",
        "reviewed_at",
    ):
        value = frontmatter.get(key)
        if value and _looks_like_date(value):
            return value

    patterns = [
        r"^approval date:\s*(.+)$",
        r"^approved at:\s*(.+)$",
        r"^approved on:\s*(.+)$",
        r"^last reviewed:\s*(.+)$",
        r"^reviewed at:\s*(.+)$",
    ]
    for raw_line in lines[:40]:
        line = raw_line.strip()
        for pattern in patterns:
            match = re.match(pattern, line, re.IGNORECASE)
            if match and _looks_like_date(match.group(1)):
                return match.group(1).strip()
    return None


def markdown_metadata(path: Path, fallback_name: str) -> tuple[str, str, str | None]:
    if path.is_dir():
        skill_md = path / "SKILL.md"
        readme = path / "README.md"
        if skill_md.exists():
            path = skill_md
        elif readme.exists():
            path = readme
        else:
            fallback_title = slug_to_title(fallback_name)
            return fallback_title, fallback_title, None
    if not path.exists():
        fallback_title = slug_to_title(fallback_name)
        return fallback_title, fallback_title, None

    lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    frontmatter = frontmatter_fields(lines)
    title = frontmatter.get("name") or frontmatter.get("title") or ""
    description = frontmatter.get("description") or ""
    approval_date = approval_date_from_lines(lines, frontmatter)
    in_code_block = False
    in_frontmatter = bool(frontmatter)
    delimiters = 0

    for raw_line in lines:
        line = raw_line.strip()
        if in_frontmatter:
            if line == "---":
                delimiters += 1
                if delimiters >= 2:
                    in_frontmatter = False
            continue
        if line.startswith("```"):
            in_code_block = not in_code_block
            continue
        if in_code_block or not line:
            continue
        if not title and line.startswith("#"):
            title = clean_markdown_line(line.lstrip("#").strip())
            continue
        if line.startswith("#"):
            continue
        if re.fullmatch(r"[-|: ]+", line):
            continue
        if line.startswith("|") or line.startswith("!["):
            continue
        description = clean_markdown_line(line)
        if description:
            break

    if not title:
        title = slug_to_title(fallback_name)
    if not description:
        description = title
    return title, description, approval_date


def git_revision(path: Path) -> str | None:
    if not path.exists():
        return None
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=path,
            capture_output=True,
            text=True,
            check=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None
    return result.stdout.strip() or None


def git_commit_metadata(repo_root: Path, pathspec: str) -> tuple[str | None, str | None]:
    if not repo_root.exists():
        return None, None
    try:
        result = subprocess.run(
            ["git", "log", "-1", "--format=%H%x1f%cI", "--", pathspec],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None, None
    output = result.stdout.strip()
    if not output:
        return None, None
    revision, _, commit_date = output.partition("\x1f")
    return revision or None, commit_date or None


def git_commit_metadata_map(repo_root: Path, pathspecs: list[str]) -> dict[str, tuple[str | None, str | None]]:
    if not repo_root.exists():
        return {}

    wanted = set(dedupe([path for path in pathspecs if path]))
    if not wanted:
        return {}

    try:
        result = subprocess.run(
            ["git", "log", "--format=%H%x1f%cI", "--name-only", "--", *sorted(wanted)],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return {}

    metadata: dict[str, tuple[str | None, str | None]] = {}
    commit_revision: str | None = None
    commit_date: str | None = None
    for raw_line in result.stdout.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if "\x1f" in line:
            commit_revision, _, commit_date = line.partition("\x1f")
            continue
        if line in wanted and line not in metadata:
            metadata[line] = (commit_revision or None, commit_date or None)
            if len(metadata) == len(wanted):
                break
    return metadata


def provider_family_name(source_name: str) -> str:
    if "voltagent" in source_name:
        return "VoltAgent"
    if "agency" in source_name:
        return "Agency"
    if "microsoft" in source_name:
        return "Microsoft"
    if "anthropic" in source_name:
        return "Anthropic"
    return slug_to_title(source_name)


def category_label(roots: list[str]) -> str:
    if not roots:
        return "general"
    first = roots[0].rstrip("/")
    parts = [part for part in first.split("/") if part]
    label = parts[-2] if first.endswith(".md") and len(parts) > 1 else parts[-1]
    label = re.sub(r"^\d+-", "", label)
    return label.replace("-", " ").replace("_", " ").strip() or "general"


def shorten_sentence(text: str, limit: int = 180) -> str:
    compact = clean_markdown_line(text).strip()
    if len(compact) <= limit:
        return compact
    truncated = compact[: limit - 1].rsplit(" ", 1)[0].rstrip(" ,;:")
    return truncated + "…"


def inferred_use_when(title: str, description: str) -> str:
    summary = clean_markdown_line(description)
    summary = re.sub(r"^\-?\s*Role:\s*", "", summary, flags=re.IGNORECASE)
    had_role_prefix = bool(re.match(r"^You are\s+", summary, re.IGNORECASE))
    summary = re.sub(r"^You are\s+", "", summary, flags=re.IGNORECASE)
    if had_role_prefix and "," in summary:
        summary = summary.split(",", 1)[1].strip()
    summary = summary.rstrip(".")
    if not summary:
        summary = f"{title} support"
    if summary[:1].isalpha():
        summary = summary[:1].lower() + summary[1:]
    return shorten_sentence(f"Use when you need {summary}.")


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


def progress_line(step: int, total: int, label: str, width: int = 24) -> str:
    filled = min(width, round((step / total) * width)) if total else width
    return f"[{'#' * filled}{'-' * (width - filled)}] {step}/{total} {label}"


def load_previous_entrypoints() -> dict[str, dict[str, object]]:
    if not ENTRYPOINTS_PATH.exists():
        return {}
    data = load_toml(ENTRYPOINTS_PATH)
    entrypoints = data.get("entrypoints")
    return entrypoints if isinstance(entrypoints, dict) else {}


def load_provider_catalog(path: Path) -> dict[str, dict[str, object]]:
    data = load_toml(path)
    providers = data.get("providers")
    return providers if isinstance(providers, dict) else {}


def common_posix_path(paths: list[str]) -> str:
    normalized = [[part for part in path.split("/") if part] for path in paths if path]
    if not normalized:
        return ""
    prefix: list[str] = []
    for parts in zip(*normalized):
        if len(set(parts)) != 1:
            break
        prefix.append(parts[0])
    return "/".join(prefix)


def parent_dir(path: str) -> str:
    parent = PurePosixPath(path).parent.as_posix()
    return "" if parent == "." else parent


def infer_skill_roots(source_name: str, provider_name: str, source_paths: list[str]) -> list[str]:
    normalized = dedupe([path.replace("\\", "/") for path in source_paths])
    if not normalized:
        return []
    if source_name == "microsoft-skills" and provider_name.count("-") >= 2 and len(normalized) == 1:
        return normalized
    parents = dedupe([parent_dir(path) for path in normalized if parent_dir(path)])
    common = common_posix_path(parents)
    if common:
        return [common]
    return parents or normalized


def infer_agent_roots(source_paths: list[str]) -> list[str]:
    normalized = dedupe([path.replace("\\", "/") for path in source_paths])
    if not normalized:
        return []
    if len(normalized) == 1 and normalized[0].endswith(".md"):
        return normalized
    common = common_posix_path(normalized)
    if common and common != "categories":
        return [common]
    first_segments = dedupe([path.split("/", 1)[0] for path in normalized])
    if len(first_segments) == 1 and first_segments[0] == "categories":
        category_roots = dedupe(["/".join(path.split("/")[:2]) for path in normalized if len(path.split("/")) >= 2])
        return category_roots or ["categories"]
    return first_segments


def infer_provider_roots(kind: str, source_name: str, provider_name: str, source_paths: list[str]) -> list[str]:
    if kind == "skill":
        return infer_skill_roots(source_name, provider_name, source_paths)
    return infer_agent_roots(source_paths)


def infer_provider_homepage(repo: dict[str, object], roots: list[str], entries: list[dict[str, object]]) -> str:
    if len(entries) == 1:
        source_url = str(entries[0].get("source_url") or "")
        if source_url:
            return source_url
    if len(roots) == 1:
        return f"https://github.com/{repo['owner']}/{repo['repo']}/tree/main/{roots[0]}"
    return str(repo.get("url") or "")


def bootstrap_provider_catalog(
    kind: str,
    repositories: dict[str, object],
    previous: dict[str, dict[str, object]],
) -> dict[str, dict[str, object]]:
    by_provider: dict[str, list[dict[str, object]]] = {}
    for entry in previous.values():
        provider_name = entry.get("provider")
        if entry.get("kind") != kind or not isinstance(provider_name, str):
            continue
        by_provider.setdefault(provider_name, []).append(entry)

    records: dict[str, dict[str, object]] = {}
    for provider_name, entries in by_provider.items():
        ordered_entries = sorted(entries, key=lambda entry: str(entry.get("source_path") or ""))
        source_name = str(ordered_entries[0]["source"])
        repo = repositories[source_name]
        source_paths = [str(entry["source_path"]) for entry in ordered_entries]
        roots = infer_provider_roots(kind, source_name, provider_name, source_paths)
        tags = dedupe([str(tag) for entry in ordered_entries for tag in entry.get("tags", []) if isinstance(tag, str)])
        records[provider_name] = {
            "source": source_name,
            "prefix": provider_name,
            "roots": roots,
            "homepage": infer_provider_homepage(repo, roots, ordered_entries),
            "version_channel": str(repo.get("version_channel") or "main"),
            "tags": tags,
        }
    return records


def entrypoint_key(kind: str, provider: str | None, source: str, source_path: str) -> str:
    owner = provider or source
    return f"{kind}:{owner}:{source_path}"


def entrypoint_record(
    *,
    kind: str,
    source: str,
    provider: str | None,
    source_path: str,
    local_name: str,
    local_output: str,
    title: str,
    description: str,
    source_url: str,
    tags: list[str],
    commit_revision: str | None,
    commit_date: str | None,
    approval_date: str | None,
    measured_revision: str | None,
    measured_at: str,
    previous: dict[str, object] | None,
) -> dict[str, object]:
    first_seen_at = measured_at
    if previous is not None:
        first_seen_at = str(previous.get("first_seen_at") or previous.get("measured_at") or measured_at)
    return {
        "kind": kind,
        "source": source,
        "provider": provider,
        "source_path": source_path,
        "local_name": local_name,
        "local_output": local_output,
        "title": title,
        "description": description,
        "source_url": source_url,
        "tags": dedupe(tags),
        "commit_revision": commit_revision,
        "commit_date": commit_date,
        "approval_date": approval_date,
        "measured_revision": measured_revision,
        "measured_at": measured_at,
        "first_seen_at": first_seen_at,
        "last_seen_at": measured_at,
    }


def build_plugin_records(existing_plugins: dict[str, object], previous: dict[str, dict[str, object]]) -> tuple[dict[str, dict[str, object]], list[dict[str, object]]]:
    repo_root = ROOT / "external" / "awesome-copilot"
    plugin_root = repo_root / "plugins"
    measured_at = now_iso()
    revision = git_revision(repo_root)
    records: dict[str, dict[str, object]] = {}
    entrypoints: list[dict[str, object]] = []

    if plugin_root.exists():
        plugin_names = sorted(path.name for path in plugin_root.iterdir() if path.is_dir() and (path / "README.md").exists())
    else:
        plugin_names = sorted(existing_plugins)

    commit_metadata = git_commit_metadata_map(repo_root, [f"plugins/{name}/README.md" for name in plugin_names])

    for name in plugin_names:
        install_source = f"{name}@awesome-copilot"
        readme = plugin_root / name / "README.md"
        title, description, approval_date = markdown_metadata(readme, name)
        source_url = f"https://github.com/github/awesome-copilot/tree/main/plugins/{name}"
        records[name] = {
            "install_source": install_source,
            "description": description,
            "use_when": existing_plugins.get(name, {}).get("use_when") if isinstance(existing_plugins.get(name), dict) else None,
            "source_url": source_url,
            "version_channel": "marketplace-latest",
            "tags": dedupe(classify_tags(f"{name} {description}")),
        }
        if records[name]["use_when"] is None:
            records[name]["use_when"] = inferred_use_when(title, description)
        source_path = f"plugins/{name}/README.md"
        commit_revision, commit_date = commit_metadata.get(source_path, (None, None))
        key = entrypoint_key("plugin", None, "awesome-copilot", source_path)
        entrypoints.append(
            entrypoint_record(
                kind="plugin",
                source="awesome-copilot",
                provider=None,
                source_path=source_path,
                local_name=name,
                local_output=install_source,
                title=title,
                description=description,
                source_url=f"https://github.com/github/awesome-copilot/blob/main/{source_path}",
                tags=["plugin", *classify_tags(f"{name} {description}")],
                commit_revision=commit_revision,
                commit_date=commit_date,
                approval_date=approval_date,
                measured_revision=revision,
                measured_at=measured_at,
                previous=previous.get(key),
            )
        )
    return records, entrypoints


def github_source_url(owner: str, repo: str, relative_path: str, *, is_tree: bool) -> str:
    view = "tree" if is_tree else "blob"
    return f"https://github.com/{owner}/{repo}/{view}/main/{relative_path}"


def resolve_microsoft_skill_directory(source_root: Path, candidate: Path) -> tuple[Path, str] | None:
    if not candidate.is_symlink():
        return None
    target_name = candidate.readlink().name
    matches: list[Path] = []
    direct_skill = source_root / ".github" / "skills" / target_name
    if direct_skill.is_dir():
        matches.append(direct_skill)
    plugin_skills_root = source_root / ".github" / "plugins"
    if plugin_skills_root.exists():
        matches.extend(sorted(path for path in plugin_skills_root.glob(f"*/skills/{target_name}") if path.is_dir()))
    if len(matches) != 1:
        return None
    resolved = matches[0]
    return resolved, resolved.relative_to(source_root).as_posix()


def directory_entry_candidates(path: Path) -> list[Path]:
    subdirs = sorted(item for item in path.iterdir() if item.is_dir() or item.is_symlink())
    return subdirs or [path]


def build_provider_entrypoints_for_source(
    kind: str,
    source_name: str,
    source_root: Path,
    candidates: list[dict[str, object]],
    measured_at: str,
    revision: str | None,
    previous: dict[str, dict[str, object]],
) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    commit_metadata = git_commit_metadata_map(source_root, [str(candidate.get("commit_path") or candidate["source_path"]) for candidate in candidates])
    for candidate in candidates:
        provider_name = str(candidate["provider_name"])
        source_path = str(candidate["source_path"])
        local_name = str(candidate["local_name"])
        title, description, approval_date = markdown_metadata(Path(candidate["metadata_path"]), local_name)
        commit_path = str(candidate.get("commit_path") or source_path)
        commit_revision, commit_date = commit_metadata.get(commit_path, (None, None))
        key = entrypoint_key(kind, provider_name, source_name, source_path)
        records.append(
            entrypoint_record(
                kind=kind,
                source=source_name,
                provider=provider_name,
                source_path=source_path,
                local_name=local_name,
                local_output=str(candidate["local_output"]),
                title=title,
                description=description,
                source_url=str(candidate["source_url"]),
                tags=[kind, *provider_tags(provider_name, kind, source_name), *classify_tags(f"{title} {description}")],
                commit_revision=commit_revision,
                commit_date=commit_date,
                approval_date=approval_date,
                measured_revision=revision,
                measured_at=measured_at,
                previous=previous.get(key),
            )
        )
    return records


def build_skill_entrypoints(skills: dict[str, object], repositories: dict[str, object], previous: dict[str, dict[str, object]]) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    repo_contexts: dict[str, dict[str, object]] = {}
    candidates_by_source: dict[str, list[dict[str, object]]] = {}
    for provider_name, raw in skills.items():
        provider = raw if isinstance(raw, dict) else {}
        source_name = str(provider["source"])
        repo = repositories[source_name]
        source_root = ROOT / repo["submodule_path"]
        repo_contexts.setdefault(
            source_name,
            {
                "source_root": source_root,
                "measured_at": now_iso(),
                "revision": git_revision(source_root),
            },
        )
        candidates = candidates_by_source.setdefault(source_name, [])
        for root in provider["roots"]:
            base = source_root / str(root)
            if not base.exists():
                continue
            if base.is_file():
                relative = base.relative_to(source_root).as_posix()
                candidates.append(
                    {
                        "provider_name": provider_name,
                        "source_name": source_name,
                        "source_path": relative,
                        "commit_path": relative,
                        "metadata_path": base,
                        "local_name": base.stem,
                        "local_output": f"{provider['prefix']}__{base.stem}",
                        "source_url": github_source_url(str(repo["owner"]), str(repo["repo"]), relative, is_tree=False),
                    }
                )
                continue
            for candidate in directory_entry_candidates(base):
                relative_root = candidate.relative_to(source_root).as_posix()
                if candidate.is_symlink():
                    source_path = relative_root
                    resolved_dir = resolve_microsoft_skill_directory(source_root, candidate) if source_name == "microsoft-skills" else None
                    if resolved_dir is not None:
                        metadata_path, commit_path = resolved_dir
                        source_url = github_source_url(str(repo["owner"]), str(repo["repo"]), commit_path, is_tree=True)
                    else:
                        metadata_path = candidate
                        commit_path = source_path
                        source_url = github_source_url(str(repo["owner"]), str(repo["repo"]), relative_root, is_tree=False)
                else:
                    entry_file = candidate / "SKILL.md"
                    if not entry_file.exists():
                        entry_file = candidate / "README.md"
                    source_path = entry_file.relative_to(source_root).as_posix() if entry_file.exists() else relative_root
                    metadata_path = entry_file if entry_file.exists() else candidate
                    commit_path = source_path
                    source_url = github_source_url(str(repo["owner"]), str(repo["repo"]), relative_root, is_tree=True)
                candidates.append(
                    {
                        "provider_name": provider_name,
                        "source_name": source_name,
                        "source_path": source_path,
                        "commit_path": commit_path,
                        "metadata_path": metadata_path,
                        "local_name": candidate.name,
                        "local_output": f"{provider['prefix']}__{candidate.name}",
                        "source_url": source_url,
                    }
                )

    with ThreadPoolExecutor(max_workers=max(1, len(candidates_by_source))) as executor:
        futures = [
            executor.submit(
                build_provider_entrypoints_for_source,
                "skill",
                source_name,
                Path(repo_contexts[source_name]["source_root"]),
                candidates,
                str(repo_contexts[source_name]["measured_at"]),
                str(repo_contexts[source_name]["revision"]) if repo_contexts[source_name]["revision"] is not None else None,
                previous,
            )
            for source_name, candidates in candidates_by_source.items()
        ]
        for future in futures:
            records.extend(future.result())
    return records


def build_agent_entrypoints(agents: dict[str, object], repositories: dict[str, object], previous: dict[str, dict[str, object]]) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    repo_contexts: dict[str, dict[str, object]] = {}
    candidates_by_source: dict[str, list[dict[str, object]]] = {}
    for provider_name, raw in agents.items():
        provider = raw if isinstance(raw, dict) else {}
        source_name = str(provider["source"])
        repo = repositories[source_name]
        source_root = ROOT / repo["submodule_path"]
        repo_contexts.setdefault(
            source_name,
            {
                "source_root": source_root,
                "measured_at": now_iso(),
                "revision": git_revision(source_root),
            },
        )
        candidates = candidates_by_source.setdefault(source_name, [])
        for root in provider["roots"]:
            base = source_root / str(root)
            if not base.exists():
                continue
            files = [base] if base.is_file() else sorted(path for path in base.rglob("*.md") if path.name != "README.md")
            for file_path in files:
                relative = file_path.relative_to(source_root).as_posix()
                flat = relative.replace("/", "__").replace("\\", "__")
                if flat.endswith(".md"):
                    flat = flat[:-3]
                candidates.append(
                    {
                        "provider_name": provider_name,
                        "source_name": source_name,
                        "source_path": relative,
                        "metadata_path": file_path,
                        "local_name": file_path.stem,
                        "local_output": f"{provider['prefix']}__{flat}.agent.md",
                        "source_url": f"https://github.com/{repo['owner']}/{repo['repo']}/blob/main/{relative}",
                    }
                )

    with ThreadPoolExecutor(max_workers=max(1, len(candidates_by_source))) as executor:
        futures = [
            executor.submit(
                build_provider_entrypoints_for_source,
                "agent",
                source_name,
                Path(repo_contexts[source_name]["source_root"]),
                candidates,
                str(repo_contexts[source_name]["measured_at"]),
                str(repo_contexts[source_name]["revision"]) if repo_contexts[source_name]["revision"] is not None else None,
                previous,
            )
            for source_name, candidates in candidates_by_source.items()
        ]
        for future in futures:
            records.extend(future.result())
    return records


def provider_root_descriptor(kind: str, source_name: str, roots: list[str]) -> str:
    if not roots:
        return provider_family_name(source_name)
    primary = roots[0].rstrip("/")
    parts = [part for part in primary.split("/") if part]
    ignored = {"categories"} if kind == "agent" else {"skills", "scientific-skills"}
    filtered = [re.sub(r"^\d+-", "", part) for part in parts if part not in ignored]
    if primary.endswith(".md") and filtered:
        filtered[-1] = PurePosixPath(filtered[-1]).stem
    return slug_to_title("/".join(filtered[-2:] if len(filtered) >= 2 else filtered))


def build_provider_records(
    kind: str,
    providers: dict[str, object],
    repositories: dict[str, object],
    provider_entrypoints: list[dict[str, object]],
) -> dict[str, dict[str, object]]:
    entrypoints_by_provider: dict[str, list[dict[str, object]]] = {}
    for entry in provider_entrypoints:
        provider_name = entry.get("provider")
        if isinstance(provider_name, str):
            entrypoints_by_provider.setdefault(provider_name, []).append(entry)

    records: dict[str, dict[str, object]] = {}
    for provider_name, raw in providers.items():
        provider = dict(raw) if isinstance(raw, dict) else {}
        roots = [str(root) for root in provider.get("roots", [])]
        entries = sorted(
            entrypoints_by_provider.get(provider_name, []),
            key=lambda entry: str(entry.get("title") or entry.get("local_name") or ""),
        )
        source_name = str(provider.get("source", provider_name))
        family = provider_family_name(source_name)
        repo = repositories[source_name]
        descriptor = provider_root_descriptor(kind, source_name, roots)
        if len(entries) == 1:
            entry = entries[0]
            title = str(entry["title"])
            entry_description = clean_markdown_line(str(entry["description"]))
            if kind == "skill" and len(entry_description.split()) < 3:
                description = shorten_sentence(f"{family} {descriptor} skill pack synced into the local skills catalog.")
                use_when = shorten_sentence(f"Use when you want the {family} {descriptor} skill pack available locally.")
            else:
                description = shorten_sentence(entry_description or title)
                use_when = inferred_use_when(title, entry_description or title)
            homepage = str(entry["source_url"])
            tags = dedupe([*provider.get("tags", []), *entry.get("tags", [])])
        elif entries:
            label = category_label(roots)
            examples = ", ".join(str(entry["title"]) for entry in entries[:3])
            if kind == "skill":
                description = shorten_sentence(f"{family} {label} skills such as {examples}.")
                use_when = shorten_sentence(f"Use when you want local {family} {label} skills such as {examples}.")
            else:
                description = shorten_sentence(f"{family} {label} specialists such as {examples}.")
                use_when = shorten_sentence(f"Use when you want local {family} {label} agents such as {examples}.")
            homepage = str(provider.get("homepage") or "")
            if not homepage and roots:
                homepage = f"https://github.com/{repo['owner']}/{repo['repo']}/tree/main/{roots[0]}"
            if not homepage:
                homepage = str(repo.get("url") or "")
            tags = dedupe([*provider.get("tags", []), *(tag for entry in entries for tag in entry.get("tags", []))])
        else:
            description = str(provider.get("description") or "")
            use_when = str(provider.get("use_when") or "")
            homepage = str(provider.get("homepage") or "")
            tags = list(provider.get("tags", []))

        records[provider_name] = {
            "source": str(provider["source"]),
            "prefix": str(provider["prefix"]),
            "roots": roots,
            "description": description,
            "use_when": use_when,
            "homepage": homepage,
            "version_channel": str(provider.get("version_channel") or "main"),
            "tags": tags,
        }
    return records


def write_plugins(records: dict[str, dict[str, object]]) -> None:
    lines: list[str] = []
    for name in sorted(records):
        record = records[name]
        lines.append(f"[plugins.{quote_key(name)}]")
        lines.append(f"install_source = {quote_string(str(record['install_source']))}")
        lines.append(f"description = {quote_string(str(record['description']))}")
        lines.append(f"use_when = {quote_string(str(record['use_when']))}")
        lines.append(f"source_url = {quote_string(str(record['source_url']))}")
        lines.append(f"version_channel = {quote_string(str(record['version_channel']))}")
        lines.append(f"tags = {format_array(list(record['tags']))}")
        lines.append("")
    PLUGINS_PATH.write_text("\n".join(lines) + "\n")


def write_providers(path: Path, records: dict[str, dict[str, object]]) -> None:
    lines: list[str] = []
    for name in sorted(records):
        record = records[name]
        lines.append(f"[providers.{quote_key(name)}]")
        lines.append(f"source = {quote_string(str(record['source']))}")
        lines.append(f"prefix = {quote_string(str(record['prefix']))}")
        lines.append(f"roots = {format_array(list(record['roots']))}")
        lines.append(f"description = {quote_string(str(record['description']))}")
        lines.append(f"use_when = {quote_string(str(record['use_when']))}")
        lines.append(f"homepage = {quote_string(str(record['homepage']))}")
        lines.append(f"version_channel = {quote_string(str(record['version_channel']))}")
        lines.append(f"tags = {format_array(list(record['tags']))}")
        lines.append("")
    path.write_text("\n".join(lines) + "\n")


def write_entrypoints(records: list[dict[str, object]]) -> None:
    keyed = {
        entrypoint_key(
            str(record["kind"]),
            str(record["provider"]) if record.get("provider") is not None else None,
            str(record["source"]),
            str(record["source_path"]),
        ): record
        for record in records
    }
    lines: list[str] = []
    for key in sorted(keyed):
        record = keyed[key]
        lines.append(f"[entrypoints.{quote_key(key)}]")
        for field in [
            "kind",
            "source",
            "source_path",
            "local_name",
            "local_output",
            "title",
            "description",
            "source_url",
            "commit_revision",
            "commit_date",
            "approval_date",
            "measured_revision",
            "measured_at",
            "first_seen_at",
            "last_seen_at",
        ]:
            value = record.get(field)
            if value is not None:
                lines.append(f"{field} = {quote_string(str(value))}")
        if record.get("provider") is not None:
            lines.append(f"provider = {quote_string(str(record['provider']))}")
        lines.append(f"tags = {format_array(list(record['tags']))}")
        lines.append("")
    ENTRYPOINTS_PATH.write_text("\n".join(lines) + "\n")


def new_progress() -> Progress:
    return Progress(
        SpinnerColumn(),
        TextColumn("[bold blue]{task.description}"),
        BarColumn(),
        TaskProgressColumn(),
        TimeElapsedColumn(),
        console=console(),
        transient=False,
    )


@app.command()
def main(
    hard_reset: bool = typer.Option(
        False,
        "--hard-reset",
        help="Rebuild entrypoint history from scratch instead of preserving first-seen metadata.",
    ),
) -> None:
    total_steps = 5
    total_start = time.perf_counter()
    term = console()
    repositories = load_toml(REPOSITORIES_PATH).get("repositories", {})
    bootstrap_entrypoints = load_previous_entrypoints()
    skills = load_provider_catalog(SKILLS_PATH) or bootstrap_provider_catalog("skill", repositories, bootstrap_entrypoints)
    agents = load_provider_catalog(AGENTS_PATH) or bootstrap_provider_catalog("agent", repositories, bootstrap_entrypoints)
    existing_plugins = load_toml(PLUGINS_PATH).get("plugins", {})
    previous = {} if hard_reset else bootstrap_entrypoints

    term.print("[bold]Refreshing bundled catalog metadata[/bold]")
    with new_progress() as progress:
        task_id = progress.add_task("Refreshing plugin catalog...", total=total_steps)

        phase_start = time.perf_counter()
        plugin_records, plugin_entrypoints = build_plugin_records(existing_plugins, previous)
        progress.advance(task_id)
        term.print(f"[green]plugins[/green]={len(plugin_records)} entrypoints={len(plugin_entrypoints)} duration={time.perf_counter() - phase_start:.2f}s")

        progress.update(task_id, description="Refreshing skill entrypoints...")
        phase_start = time.perf_counter()
        skill_entrypoints = build_skill_entrypoints(skills, repositories, previous)
        progress.advance(task_id)
        term.print(f"[green]skills[/green]={len(skill_entrypoints)} duration={time.perf_counter() - phase_start:.2f}s")

        progress.update(task_id, description="Refreshing agent entrypoints...")
        phase_start = time.perf_counter()
        agent_entrypoints = build_agent_entrypoints(agents, repositories, previous)
        progress.advance(task_id)
        term.print(f"[green]agents[/green]={len(agent_entrypoints)} duration={time.perf_counter() - phase_start:.2f}s")

        progress.update(task_id, description="Rebuilding provider catalogs...")
        phase_start = time.perf_counter()
        skill_provider_records = build_provider_records("skill", skills, repositories, skill_entrypoints)
        agent_provider_records = build_provider_records("agent", agents, repositories, agent_entrypoints)
        progress.advance(task_id)
        term.print(
            f"[green]skill_providers[/green]={len(skill_provider_records)} "
            f"[green]agent_providers[/green]={len(agent_provider_records)} duration={time.perf_counter() - phase_start:.2f}s"
        )

        progress.update(task_id, description="Writing refreshed catalog files...")
        phase_start = time.perf_counter()
        write_plugins(plugin_records)
        write_providers(SKILLS_PATH, skill_provider_records)
        write_providers(AGENTS_PATH, agent_provider_records)
        write_entrypoints([*plugin_entrypoints, *skill_entrypoints, *agent_entrypoints])
        progress.advance(task_id)
        term.print(f"[green]wrote[/green]=4 files duration={time.perf_counter() - phase_start:.2f}s")

    term.print(
        "[bold green]Refreshed catalogs[/bold green]: "
        f"plugins={len(plugin_records)} "
        f"skills={len(skill_entrypoints)} "
        f"agents={len(agent_entrypoints)} "
        f"entrypoints={len(plugin_entrypoints) + len(skill_entrypoints) + len(agent_entrypoints)} "
        f"total={time.perf_counter() - total_start:.2f}s"
        f"{' (hard-reset)' if hard_reset else ''}"
    )


if __name__ == "__main__":
    app()
