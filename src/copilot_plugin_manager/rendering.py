from __future__ import annotations

import os
from dataclasses import dataclass
from typing import cast

from rich import box
from rich.console import Console
from rich.panel import Panel
from rich.table import Table
from rich.tree import Tree

from .catalog import CatalogBundle
from .models import ActivationTarget


@dataclass(frozen=True)
class Glyphs:
    branch: str
    last: str
    vertical: str
    blank: str
    pipe: str
    divider: str
    profile: str
    theme: str
    plugin: str
    skill: str
    agent: str
    install: str
    remove: str
    info: str


ASCII_GLYPHS = Glyphs(
    "|-",
    "\\-",
    "| ",
    "  ",
    "|",
    "=",
    "[profile]",
    "[theme]",
    "[plugin]",
    "[skill]",
    "[agent]",
    "[+]",
    "[-]",
    "[i]",
)
UNICODE_GLYPHS = Glyphs("├─", "└─", "│ ", "  ", "│", "═", "◉", "◆", "▣", "✦", "◎", "➜", "✖", "ℹ")


def get_glyphs() -> Glyphs:
    return ASCII_GLYPHS if os.environ.get("COPILOT_PLUGINS_ASCII") == "1" else UNICODE_GLYPHS


def console() -> Console:
    return Console(soft_wrap=True)


def _base_table(title: str, header_style: str = "bold white") -> Table:
    return Table(
        title=title,
        title_style="bold",
        header_style=header_style,
        box=box.ROUNDED,
        expand=True,
        padding=(0, 1),
        collapse_padding=True,
        show_lines=False,
    )


def build_target_tree(bundle: CatalogBundle, target: ActivationTarget) -> Panel:
    glyphs = get_glyphs()
    root_glyph = glyphs.theme if target.kind == "theme" else glyphs.profile
    tree = Tree(f"{root_glyph} {target.name}", guide_style="dim")
    for theme_name in sorted(target.themes):
        theme_node = tree.add(f"{glyphs.theme} {theme_name}")
        theme = bundle.themes[theme_name]
        for plugin in sorted(theme.plugins):
            theme_node.add(f"{glyphs.plugin} plugin  {plugin}")
        for skill in sorted(theme.skills):
            theme_node.add(f"{glyphs.skill} skill   {skill}")
        for agent in sorted(theme.agents):
            theme_node.add(f"{glyphs.agent} agent   {agent}")
    return Panel(tree, title="Active target", box=box.ROUNDED, expand=True)


def _metadata_table(title: str, rows: list[tuple[str, str]]) -> Table:
    table = Table(
        show_header=False,
        box=None,
        expand=True,
        padding=(0, 1),
        collapse_padding=True,
    )
    table.add_column("Label", style="cyan", no_wrap=True, width=18)
    table.add_column("Value", style="white", overflow="fold")
    for label, value in rows:
        if value:
            table.add_row(label, value)
    return table


def _overview_panel(title: str, rows: list[tuple[str, str]]) -> Panel:
    return Panel(_metadata_table(title, rows), box=box.ROUNDED, expand=True)


def _short_revision(revision: str | None) -> str:
    return revision[:12] if revision else ""


def _short_timestamp(value: str | None) -> str:
    return value.replace("T", " ", 1)[:19] if value else ""


_PROFILE_BASE_THEMES = {"core", "testing", "mcp", "agents", "mcp-agents"}


def _profile_focus(themes: list[str]) -> str:
    if thematic := [theme for theme in themes if theme not in _PROFILE_BASE_THEMES]:
        return ", ".join(thematic[:2])
    return ", ".join(themes[:2])


def _ordered_profiles(bundle: CatalogBundle) -> list[tuple[str, str, list[str]]]:
    ordered = []
    for name, profile in bundle.profiles.items():
        focus = _profile_focus(profile.themes)
        ordered.append((name, focus, profile.themes))
    return sorted(ordered, key=lambda item: item[0])


def render_profiles(bundle: CatalogBundle) -> Table:
    table = _base_table("Profiles", header_style="bold green")
    table.add_column("Profile", style="green", no_wrap=True, width=24)
    table.add_column("Focus", style="cyan", no_wrap=True, width=22)
    table.add_column("Themes", style="white", overflow="fold")
    for name, focus, themes in _ordered_profiles(bundle):
        table.add_row(
            name,
            "all themes" if name == "everything" else focus,
            "all themes" if name == "everything" else ", ".join(themes),
        )
    return table


def render_themes(bundle: CatalogBundle, active_name: str | None = None) -> Table:
    table = _base_table("Themes", header_style="bold yellow")
    table.add_column("Theme", style="yellow", no_wrap=True, width=24)
    table.add_column("Plugins", overflow="fold")
    table.add_column("Skills", overflow="fold")
    table.add_column("Agents", overflow="fold")
    for name in sorted(bundle.themes):
        theme = bundle.themes[name]
        active_suffix = " [active]" if active_name == name else ""
        table.add_row(
            f"{name}{active_suffix}",
            ", ".join(sorted(theme.plugins)),
            ", ".join(sorted(theme.skills)),
            ", ".join(sorted(theme.agents)),
        )
    return table


def render_repositories(bundle: CatalogBundle) -> Table:
    table = _base_table("Repository sources", header_style="bold magenta")
    table.add_column("Name", style="magenta", no_wrap=True, width=20)
    table.add_column("Owner/Repo", no_wrap=True, width=28)
    table.add_column("Revision", no_wrap=True, width=12)
    table.add_column("Commit date", no_wrap=True, width=19)
    table.add_column("Files", justify="right", no_wrap=True, width=5)
    table.add_column("Providers", justify="right", no_wrap=True, width=9)
    table.add_column("Submodule path", overflow="fold", width=22)
    table.add_column("Description", overflow="fold")
    for name in sorted(bundle.repositories):
        details = bundle.repository_metadata(name)
        summary = bundle.source_entrypoint_summary(name)
        table.add_row(
            name,
            details["owner_repo"],
            _short_revision(cast(str | None, summary["revision"])),
            _short_timestamp(cast(str | None, summary["commit_date"]) or cast(str | None, summary["measured_at"])),
            str(summary["file_count"]),
            str(summary["provider_count"]),
            details["submodule_path"],
            details["description"],
        )
    return table


def render_plugins(bundle: CatalogBundle) -> Table:
    table = _base_table(f"Plugins ({len(bundle.plugins)})", header_style="bold blue")
    table.add_column("Name", style="blue", no_wrap=True, width=28)
    table.add_column("Install source", overflow="fold", width=28)
    table.add_column("Version", no_wrap=True, width=10)
    table.add_column("Tags", overflow="fold", width=16)
    table.add_column("Source URL", overflow="fold", width=28)
    table.add_column("Description", overflow="fold")
    for name in sorted(bundle.plugins):
        details = bundle.plugin_details(name)
        table.add_row(
            name,
            details["install_source"],
            details["version"],
            details["tags"],
            details["source_url"],
            details["description"],
        )
    return table


def render_providers(bundle: CatalogBundle, kind: str) -> Table:
    registry = bundle.skill_providers if kind == "skill" else bundle.agent_providers
    title = "Skill providers" if kind == "skill" else "Agent providers"
    table = _base_table(title, header_style="bold cyan")
    table.add_column("Name", no_wrap=True, width=30)
    table.add_column("Layout", no_wrap=True, width=11)
    table.add_column("Items", justify="right", no_wrap=True, width=5)
    table.add_column("Source", no_wrap=True, width=18)
    table.add_column("Revision", no_wrap=True, width=12)
    table.add_column("Commit date", no_wrap=True, width=19)
    table.add_column("Roots", overflow="fold", width=30)
    table.add_column("Description", overflow="fold")
    ordered_names = sorted(registry)
    for name in ordered_names:
        details = bundle.provider_details(kind, name)
        summary = bundle.provider_entrypoint_summary(kind, name)
        table.add_row(
            name,
            str(summary["layout"]),
            str(summary["entrypoint_count"]),
            details["source"],
            _short_revision(cast(str | None, summary["revision"])),
            _short_timestamp(cast(str | None, summary["commit_date"]) or cast(str | None, summary["measured_at"])),
            details["roots"],
            details["description"],
        )
    return table


def render_mcps(bundle: CatalogBundle) -> Table:
    table = _base_table(f"MCP servers ({len(bundle.mcps)})", header_style="bold magenta")
    table.add_column("Name", style="magenta", no_wrap=True, width=24)
    table.add_column("Kind", no_wrap=True, width=6)
    table.add_column("Identifier", overflow="fold", width=32)
    table.add_column("Version", no_wrap=True, width=14)
    table.add_column("Tags", overflow="fold", width=18)
    table.add_column("Description", overflow="fold")
    for name in sorted(bundle.mcps):
        details = bundle.mcp_details(name)
        table.add_row(
            name,
            details["kind"],
            details["identifier"],
            details["version"],
            details["tags"],
            details["description"],
        )
    return table


def render_overview(
    bundle: CatalogBundle,
    active_target: ActivationTarget | None,
    repo_hint: str,
) -> list[object]:
    rows = [
        ("Active target", active_target.name if active_target else "none"),
        ("Active type", active_target.kind if active_target else ""),
        ("Active themes", ", ".join(active_target.themes) if active_target else ""),
        ("Repo profile", repo_hint),
        ("Profiles", str(len(bundle.profiles))),
        ("Themes", str(len(bundle.themes))),
    ]
    return [
        _overview_panel("Overview", rows),
        render_profiles(bundle),
        render_themes(bundle, active_target.name if active_target else None),
    ]


def render_repo_config(status: dict[str, object], copilot_home: str) -> Panel:
    repo_hint = str(status.get("repo_hint", "")) or "not set"
    repo_hint_kind = str(status.get("repo_hint_kind", "")) or "not resolved"
    repo_hint_themes = ", ".join(cast(list[str], status.get("repo_hint_themes", []))) or "not resolved"
    rows = [
        ("Repo target hint", repo_hint),
        ("Hint target type", repo_hint_kind),
        ("Hint themes", repo_hint_themes),
        ("Repo hint file", str(status.get("repo_profile_file", "")) or "not written"),
        ("Repo settings file", str(status.get("repo_config_file", "")) or "not written"),
        ("Project catalog file", str(status.get("project_catalog_file", "")) or "not written"),
        ("Agent scope", str(status.get("agent_scope", "global"))),
        ("Agent root", str(status.get("agent_root", ""))),
        ("MCP scope", str(status.get("mcp_scope", "global"))),
        ("MCP profile", str(status.get("mcp_profile", "")) or "none"),
        ("Copilot home", copilot_home),
    ]
    return _overview_panel("Repository config", rows)


def render_status(status: dict[str, object], copilot_home: str) -> list[object]:
    active_target = cast(ActivationTarget | None, status["active_target"])
    installed_plugins = cast(list[dict[str, str | None]], status["installed_plugins"])
    source_revisions = cast(list[dict[str, str | int | None]], status["source_revisions"])
    sync_warnings = cast(list[str], status.get("sync_warnings", []))
    verification_state = "warnings present" if sync_warnings else "verified"
    rows = [
        ("Selected target", active_target.name if active_target else "none"),
        ("Active type", active_target.kind if active_target else ""),
        ("Active themes", ", ".join(active_target.themes) if active_target else ""),
        ("Target verification", verification_state),
        ("Last verified", _short_timestamp(cast(str | None, status.get("last_verified_at")))),
        ("Skill dirs", str(status["skill_count"])),
        ("Agent files", str(status["agent_count"])),
        ("Warnings", str(len(sync_warnings))),
    ]
    plugin_table = _base_table("Installed plugins", header_style="bold blue")
    plugin_table.add_column("Name", style="blue", no_wrap=True, width=28)
    plugin_table.add_column("Source", overflow="fold", width=36)
    plugin_table.add_column("Version", no_wrap=True, width=14)
    for plugin in installed_plugins:
        plugin_table.add_row(plugin["name"], plugin["source"], plugin["version"] or "")
    source_table = _base_table("Tracked source revisions", header_style="bold magenta")
    source_table.add_column("Source", style="magenta", no_wrap=True, width=20)
    source_table.add_column("Revision", no_wrap=True, width=12)
    source_table.add_column("Commit date", no_wrap=True, width=19)
    source_table.add_column("Files", justify="right", no_wrap=True, width=5)
    source_table.add_column("Providers", justify="right", no_wrap=True, width=9)
    for source in source_revisions:
        source_table.add_row(
            str(source["name"]),
            _short_revision(cast(str | None, source["revision"])),
            _short_timestamp(cast(str | None, source["commit_date"]) or cast(str | None, source["measured_at"])),
            str(source["file_count"]),
            str(source["provider_count"]),
        )
    renderables: list[object] = [_overview_panel("Copilot Plugin Manager", rows), render_repo_config(status, copilot_home)]
    if sync_warnings:
        renderables.append(render_sync_warnings(sync_warnings))
    renderables.extend([source_table, plugin_table])
    return renderables


def render_sync_warnings(warnings: list[str]) -> Panel:
    lines = "\n".join(f"- {warning}" for warning in warnings)
    return Panel(lines, title="Sync warnings", border_style="yellow", box=box.ROUNDED, expand=True)
