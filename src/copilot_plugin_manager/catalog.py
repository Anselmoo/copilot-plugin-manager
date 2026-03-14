from __future__ import annotations

import re
import tomllib
from importlib import resources
from pathlib import Path

from pydantic import BaseModel, Field

from .models import ActivationTarget, EntrypointRecord, McpRecord, PluginRecord, ProfileRecord, ProviderRecord, RepositorySource, ThemeRecord


class CatalogBundle(BaseModel):
    repositories: dict[str, RepositorySource]
    plugins: dict[str, PluginRecord]
    skill_providers: dict[str, ProviderRecord]
    agent_providers: dict[str, ProviderRecord]
    mcps: dict[str, McpRecord] = Field(default_factory=dict)
    entrypoints: dict[str, EntrypointRecord] = Field(default_factory=dict)
    themes: dict[str, ThemeRecord]
    profiles: dict[str, ProfileRecord]

    def resolve_target(self, name: str) -> ActivationTarget:
        if name in self.profiles:
            return ActivationTarget(name=name, kind="profile", themes=self.profiles[name].themes)
        if name in self.themes:
            return ActivationTarget(name=name, kind="theme", themes=[name])
        raise KeyError(f"Unknown profile or theme: {name}. Profiles: {', '.join(self.profiles)}. Themes: {', '.join(self.themes)}")

    def target_items(self, theme_names: list[str], kind: str) -> list[str]:
        seen: set[str] = set()
        ordered: list[str] = []
        for theme_name in theme_names:
            theme = self.themes[theme_name]
            entries = getattr(theme, kind)
            for entry in entries:
                if entry not in seen:
                    seen.add(entry)
                    ordered.append(entry)
        return ordered

    def plugin_install_source(self, name: str) -> str:
        return self.plugins[name].install_source if name in self.plugins else f"{name}@awesome-copilot"

    def plugin_homepage(self, name: str) -> str:
        plugin = self.plugins.get(name)
        if plugin and plugin.source_url:
            return plugin.source_url
        install_source = self.plugin_install_source(name)
        if match := re.match(r"^(?P<plugin>[^@]+)@awesome-copilot$", install_source):
            return f"https://github.com/github/awesome-copilot/tree/main/plugins/{match.group('plugin')}"
        if match := re.match(r"^(?P<owner>[^/]+)/(?P<repo>[^/]+)/(?P<path>.+)$", install_source):
            return f"https://github.com/{match.group('owner')}/{match.group('repo')}/tree/main/{match.group('path')}"
        return f"https://github.com/search?q={name}&type=repositories"

    def plugin_description(self, name: str) -> str:
        plugin = self.plugins.get(name)
        if plugin and plugin.description:
            return plugin.description
        install_source = self.plugin_install_source(name)
        patterns = description_rules()
        for pattern, description in patterns.items():
            if re.search(pattern, name):
                return description
        title = slug_to_title(name)
        if install_source.endswith("@awesome-copilot"):
            return f"{title} plugin from the awesome-copilot marketplace catalog."
        return f"{title} plugin installed from a direct GitHub source."

    def plugin_use_when(self, name: str) -> str:
        plugin = self.plugins.get(name)
        if plugin and plugin.use_when:
            return plugin.use_when
        patterns = use_when_rules()
        for pattern, description in patterns.items():
            if re.search(pattern, name):
                return description
        return "Use when this plugin matches the workflow or domain you want available in Copilot."

    def plugin_version(self, name: str) -> str:
        plugin = self.plugins.get(name)
        if plugin and plugin.version_channel:
            return plugin.version_channel
        install_source = self.plugin_install_source(name)
        return "marketplace-latest" if install_source.endswith("@awesome-copilot") else "main"

    def plugin_tags(self, name: str) -> list[str]:
        plugin = self.plugins.get(name)
        if plugin and plugin.tags:
            return plugin.tags
        return classify_tags(name)

    def plugin_details(self, name: str) -> dict[str, str]:
        return {
            "name": name,
            "description": self.plugin_description(name),
            "use_when": self.plugin_use_when(name),
            "homepage": self.plugin_homepage(name),
            "source_url": self.plugin_homepage(name),
            "license": "See source repository",
            "version": self.plugin_version(name),
            "install_source": self.plugin_install_source(name),
            "tags": ", ".join(self.plugin_tags(name)),
        }

    def repository_details(self, name: str) -> RepositorySource:
        return self.repositories[name]

    def repository_tags(self, name: str) -> list[str]:
        repo = self.repositories[name]
        tags = list(repo.tags) if repo.tags else classify_tags(f"{name} {repo.description}")
        if "skills" in name or "skill" in repo.description.lower():
            tags.append("skills")
        if "agents" in name or "agent" in repo.description.lower() or "subagents" in name:
            tags.append("agents")
        if "plugin" in repo.description.lower():
            tags.append("plugins")
        return dedupe(tags)

    def repository_metadata(self, name: str) -> dict[str, str]:
        repo = self.repositories[name]
        return {
            "name": name,
            "description": repo.description,
            "use_when": repo.use_when,
            "url": repo.url,
            "version": repo.version_channel or repo.version,
            "license": repo.license,
            "owner_repo": f"{repo.owner}/{repo.repo}",
            "submodule_path": repo.submodule_path,
            "tags": ", ".join(self.repository_tags(name)),
        }

    def provider_registry(self, kind: str) -> dict[str, ProviderRecord]:
        return self.skill_providers if kind == "skill" else self.agent_providers

    def provider_details(self, kind: str, name: str) -> dict[str, str]:
        registry = self.provider_registry(kind)
        entry = registry[name]
        source = self.repositories[entry.source]
        description = entry.description or default_provider_description(name, source.owner, source.repo, kind)
        use_when = entry.use_when or default_provider_use_when(name, source.owner, source.repo, kind)
        homepage = entry.homepage or f"{source.url}/tree/main/{entry.roots[0]}" if entry.roots else source.url
        return {
            "name": name,
            "description": description,
            "use_when": use_when,
            "homepage": homepage,
            "license": entry.license or source.license,
            "version": entry.version_channel or entry.version or source.version_channel or source.version,
            "source": entry.source,
            "roots": ", ".join(entry.roots),
            "prefix": entry.prefix,
            "source_url": homepage,
            "tags": ", ".join(entry.tags or provider_tags(name, kind, entry.source)),
        }

    def provider_specificity(self, kind: str, name: str) -> tuple[int, int, int, int]:
        provider = self.provider_registry(kind)[name]
        roots = [root.replace("\\", "/") for root in provider.roots]
        entrypoint_count = len(self.entrypoint_records(kind, provider=name))
        if len(roots) == 1 and roots[0].endswith(".md"):
            layout_rank = 2
        elif len(roots) == 1:
            layout_rank = 1
        else:
            layout_rank = 0
        deepest_root = max((root.count("/") for root in roots), default=0)
        return layout_rank, deepest_root, len(roots), entrypoint_count

    def preferred_provider_order(self, kind: str, provider_names: list[str]) -> list[str]:
        unique_names = list(dict.fromkeys(provider_names))
        return sorted(
            unique_names,
            key=lambda name: (
                -self.provider_specificity(kind, name)[0],
                -self.provider_specificity(kind, name)[1],
                self.provider_specificity(kind, name)[2],
                self.provider_specificity(kind, name)[3],
                name,
            ),
        )

    def entrypoint_records(self, kind: str, source: str | None = None, provider: str | None = None) -> list[EntrypointRecord]:
        return [
            entry for entry in self.entrypoints.values() if entry.kind == kind and (source is None or entry.source == source) and (provider is None or entry.provider == provider)
        ]

    def entrypoint_for_path(
        self,
        kind: str,
        source: str,
        source_path: str,
        provider: str | None = None,
    ) -> EntrypointRecord | None:
        normalized_path = source_path.replace("\\", "/")
        for entry in self.entrypoints.values():
            if entry.kind != kind or entry.source != source:
                continue
            if provider is not None and entry.provider != provider:
                continue
            if entry.source_path == normalized_path:
                return entry
        return None

    def _matching_entrypoints(
        self,
        *,
        kind: str | None = None,
        source: str | None = None,
        provider: str | None = None,
    ) -> list[EntrypointRecord]:
        return [
            entry
            for entry in self.entrypoints.values()
            if (kind is None or entry.kind == kind) and (source is None or entry.source == source) and (provider is None or entry.provider == provider)
        ]

    def source_entrypoint_summary(self, source: str) -> dict[str, int | str | None]:
        matches = self._matching_entrypoints(source=source)
        latest = max(matches, key=lambda entry: entry.measured_at or "") if matches else None
        return {
            "revision": ((latest.commit_revision or latest.measured_revision) if latest is not None else None),
            "commit_date": latest.commit_date if latest is not None else None,
            "measured_at": latest.measured_at if latest is not None else None,
            "file_count": len({entry.source_path for entry in matches}),
            "provider_count": len({entry.provider for entry in matches if entry.provider}),
        }

    def provider_entrypoint_summary(self, kind: str, name: str) -> dict[str, int | str | None]:
        registry = self.provider_registry(kind)
        roots = registry[name].roots
        matches = self._matching_entrypoints(kind=kind, provider=name)
        latest = max(matches, key=lambda entry: entry.measured_at or "") if matches else None
        if len(matches) == 1 or (len(roots) == 1 and roots[0].endswith(".md")):
            layout = "single-file"
        else:
            layout = "collection"
        return {
            "layout": layout,
            "entrypoint_count": len(matches),
            "revision": ((latest.commit_revision or latest.measured_revision) if latest is not None else None),
            "commit_date": latest.commit_date if latest is not None else None,
            "measured_at": latest.measured_at if latest is not None else None,
        }


    def mcp_details(self, name: str) -> dict[str, str]:
        record = self.mcps[name]
        identifier = record.package or record.url or record.local_path or name
        version = record.pinned_tag or record.version_channel or "latest"
        if record.kind == "http":
            version = record.url or "http"
        return {
            "name": name,
            "kind": record.kind,
            "identifier": identifier,
            "description": record.description or f"MCP server {name}.",
            "use_when": record.use_when or "Use when this MCP server matches the workflow you want.",
            "source_url": record.source_url or "",
            "version": version,
            "tags": ", ".join(record.tags),
        }


def slug_to_title(value: str) -> str:
    parts = [part for part in re.split(r"[-_/]", value) if part]
    return " ".join(part.upper() if len(part) <= 3 else part[:1].upper() + part[1:] for part in parts)


def dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            ordered.append(value)
    return ordered


def description_rules() -> dict[str, str]:
    return {
        r"security|audit|protect": "Security review, secure coding, and hardening workflows.",
        r"planning|spike|autonomy|context": "Planning, architecture, and task orchestration support.",
        r"python|java|ruby|rust|swift|go|php|typescript|kotlin|csharp|clojure": ("Language-specific development helpers and coding guidance."),
        r"openapi|mcp": "Helpers for API, contract-first, and MCP server/application workflows.",
        r"frontend|ui|winui|react|angular|vue|design": ("Frontend, design, and user-experience focused tooling."),
        r"data|database|postgres|oracle|bi|science": ("Data engineering, analytics, and scientific workflow support."),
        r"doc|pdf|ppt|xlsx|theme|canvas|brand": ("Documentation, publishing, and artifact production support."),
    }


def use_when_rules() -> dict[str, str]:
    return {
        r"security|audit|protect": "Use when you are reviewing risk, auditing code, or tightening security controls.",
        r"planning|spike|autonomy|context": ("Use when you need planning, decomposition, or structured execution help."),
        r"python|java|ruby|rust|swift|go|php|typescript|kotlin|csharp|clojure": ("Use when the repo or task is centered on this language or runtime."),
        r"openapi|mcp": "Use when building MCP servers, SDKs, or OpenAPI-driven applications.",
        r"frontend|ui|winui|react|angular|vue|design": ("Use when building interfaces, components, or visual experiences."),
        r"data|database|postgres|oracle|bi|science": ("Use when working on databases, analytics, research, or data-heavy tasks."),
        r"doc|pdf|ppt|xlsx|theme|canvas|brand": ("Use when producing documents, presentations, themed assets, or polished artifacts."),
    }


def classify_tags(text: str) -> list[str]:
    tags: list[str] = []
    rules = [
        (r"security|audit|protect", ["security"]),
        (r"planning|spike|autonomy|context", ["planning", "orchestration"]),
        (r"python", ["python", "language"]),
        (r"typescript|javascript|react|frontend|ui|design|winui|vue|angular", ["frontend", "ui"]),
        (r"java|ruby|rust|swift|go|php|kotlin|csharp|clojure", ["language"]),
        (r"openapi|mcp", ["api", "mcp"]),
        (r"database|postgres|oracle|data|science|research|bi", ["data"]),
        (r"doc|pdf|ppt|xlsx|canvas|brand|theme", ["docs", "artifacts"]),
        (r"github|git", ["github"]),
        (r"devops|docker|kubernetes|platform", ["devops"]),
        (r"agent|subagent", ["agents"]),
        (r"skill", ["skills"]),
    ]
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


def default_provider_description(name: str, owner: str, repo: str, kind: str) -> str:
    patterns = {
        r"^voltagent-": "VoltAgent category or specialist subagents synced into the local agent catalog.",
        r"^agency-": "Agency role-based agents synced into the local agent catalog.",
        r"^anthropic-": "Focused Anthropic skill pack synced into the local skills catalog.",
        r"^mskills-": "Microsoft language- or topic-specific skill bundle.",
        r"^kdense": "K-Dense scientific or research-oriented skill pack.",
    }
    for pattern, description in patterns.items():
        if re.search(pattern, name):
            return description
    return f"Local {kind} provider backed by {owner}/{repo}."


def default_provider_use_when(name: str, owner: str, repo: str, kind: str) -> str:
    patterns = {
        r"^voltagent-language$": "Use when you want language-specialist agents from the VoltAgent catalog.",
        r"^voltagent-(infra|quality|data-ai|devex|domains|product|orchestration|research)$": "Use when you want a focused VoltAgent category synced into local agents.",
        r"^voltagent-": "Use when you want a specific VoltAgent specialist or workflow agent locally available.",
        r"^agency-": "Use when you want role-based or division-based agents installed locally.",
        r"^anthropic-": "Use when you want one Anthropic skill rather than the full upstream collection.",
        r"^mskills-": "Use when you want Microsoft skills for a specific language or topic area.",
        r"^kdense": "Use when you need scientific research, literature, or data-analysis focused skills.",
    }
    for pattern, description in patterns.items():
        if re.search(pattern, name):
            return description
    return f"Use when you want this {kind} provider synced from {owner}/{repo}."


def _read_toml(path: str, root: Path | None = None) -> dict[str, object]:
    if root is not None:
        return tomllib.loads((root / path).read_text())
    resource = resources.files("copilot_plugin_manager.catalog_data").joinpath(path)
    return tomllib.loads(resource.read_text())


def load_catalog_bundle(root: Path | None = None) -> CatalogBundle:
    try:
        entrypoints = _read_toml("entrypoints.toml", root)["entrypoints"]
    except FileNotFoundError:
        entrypoints = {}
    try:
        mcps = _read_toml("mcps.toml", root)["mcps"]
    except (FileNotFoundError, KeyError):
        mcps = {}
    return CatalogBundle.model_validate(
        {
            "repositories": _read_toml("repositories.toml", root)["repositories"],
            "plugins": _read_toml("plugins.toml", root)["plugins"],
            "skill_providers": _read_toml("skills.toml", root)["providers"],
            "agent_providers": _read_toml("agents.toml", root)["providers"],
            "mcps": mcps,
            "entrypoints": entrypoints,
            "themes": _read_toml("themes.toml", root)["themes"],
            "profiles": _read_toml("profiles.toml", root)["profiles"],
        }
    )
