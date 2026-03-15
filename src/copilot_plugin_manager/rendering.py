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
    return Console(soft_wrap=False)


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
    for theme_name in target.themes:
        theme_node = tree.add(f"{glyphs.theme} {theme_name}")
        theme = bundle.themes[theme_name]
        for plugin in theme.plugins:
            theme_node.add(f"{glyphs.plugin} plugin  {plugin}")
        for skill in theme.skills:
            theme_node.add(f"{glyphs.skill} skill   {skill}")
        for agent in theme.agents:
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
    if not revision:
        return ""
    return revision[:12]


def _short_timestamp(value: str | None) -> str:
    if not value:
        return ""
    return value.replace("T", " ", 1)[:19]


def render_profiles(bundle: CatalogBundle) -> Table:
    table = _base_table("Profiles", header_style="bold green")
    table.add_column("Profile", style="green", no_wrap=True, width=24)
    table.add_column("Themes", style="white", overflow="fold")
    for name, profile in bundle.profiles.items():
        table.add_row(name, "all themes" if name == "everything" else ", ".join(profile.themes))
    return table


def render_themes(bundle: CatalogBundle, active_name: str | None = None) -> Table:
    table = _base_table("Themes", header_style="bold yellow")
    table.add_column("Theme", style="yellow", no_wrap=True, width=24)
    table.add_column("Plugins", overflow="fold")
    table.add_column("Skills", overflow="fold")
    table.add_column("Agents", overflow="fold")
    for name, theme in bundle.themes.items():
        active_suffix = " [active]" if active_name == name else ""
        table.add_row(
            f"{name}{active_suffix}",
            ", ".join(theme.plugins),
            ", ".join(theme.skills),
            ", ".join(theme.agents),
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
    for name in bundle.repositories:
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
    for name in bundle.plugins:
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
    ordered_names = sorted(
        registry,
        key=lambda name: (
            bundle.provider_entrypoint_summary(kind, name)["layout"] != "single-file",
            name,
        ),
    )
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
    for name in bundle.mcps:
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


def render_status(status: dict[str, object], copilot_home: str) -> list[object]:
    active_target = cast(ActivationTarget | None, status["active_target"])
    installed_plugins = cast(list[dict[str, str | None]], status["installed_plugins"])
    source_revisions = cast(list[dict[str, str | int | None]], status["source_revisions"])
    sync_warnings = cast(list[str], status.get("sync_warnings", []))
    rows = [
        ("Active target", active_target.name if active_target else "none"),
        ("Active type", active_target.kind if active_target else ""),
        ("Active themes", ", ".join(active_target.themes) if active_target else ""),
        ("Repo profile", str(status["repo_hint"])),
        ("Repo profile file", str(status.get("repo_profile_file", ""))),
        ("Copilot home", copilot_home),
        ("Skill dirs", str(status["skill_count"])),
        ("Agent files", str(status["agent_count"])),
        ("Sync warnings", str(len(sync_warnings))),
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
    renderables: list[object] = [_overview_panel("Copilot Plugin Manager", rows)]
    if sync_warnings:
        renderables.append(render_sync_warnings(sync_warnings))
    renderables.extend([source_table, plugin_table])
    return renderables


def render_sync_warnings(warnings: list[str]) -> Panel:
    table = Table(show_header=False, box=None, expand=True, padding=(0, 1), collapse_padding=True)
    table.add_column("Warning", style="yellow", overflow="fold")
    for warning in warnings:
        table.add_row(warning)
    return Panel(table, title="Sync warnings", border_style="yellow", box=box.ROUNDED, expand=True)
