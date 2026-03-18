from __future__ import annotations

import sys
from enum import StrEnum
from pathlib import Path
from typing import Annotated, Literal, cast

import typer
from rich import box
from rich.table import Table
from typer.main import get_command

from . import __version__
from .catalog import load_catalog_bundle
from .completion import completion_source, install_completion_message, install_completion_script, shell_init_snippet
from .manager import PluginManager
from .paths import ManagerPaths
from .rendering import (
    build_target_tree,
    console,
    render_mcps,
    render_overview,
    render_plugins,
    render_profiles,
    render_providers,
    render_repo_config,
    render_repositories,
    render_status,
    render_sync_warnings,
    render_themes,
)

APP_HELP = """[bold]Copilot Plugin Manager[/bold]

Install, update, inspect, and switch GitHub Copilot plugins, local skills, and local agents
from one production-focused Python CLI.

[bold]What happens by default?[/bold]
Running [cyan]copilot-plugin-manager[/cyan] with no subcommand opens a guided interactive menu
when the terminal is interactive. Non-interactive sessions fall back to a compact status view.

[bold]Main workflows[/bold]
• [cyan]list[/cyan] to explore bundled catalogs and active state
• [cyan]install[/cyan], [cyan]update[/cyan], or [cyan]delete[/cyan] to manage plugins / skills / agents
• [cyan]switch[/cyan] or [cyan]switch-exclusive[/cyan] to activate a profile or theme
• [cyan]repo-init[/cyan] to write repo-local target state explicitly
• [cyan]repo-cleanup[/cyan] to reconcile stale managed repo content explicitly
• [cyan]repo-update[/cyan] to refresh upstream sources before syncing third-party content
"""

APP_EPILOG = """\b
Quick start:
  copilot-plugin-manager repo-update --remote
  copilot-plugin-manager list overview
  copilot-plugin-manager repo-init minimal --agent-scope local
  copilot-plugin-manager repo-config --agent-scope local
  copilot-plugin-manager repo-cleanup
  copilot-plugin-manager completion init bash

Tip:
  Use copilot-plugin-manager COMMAND -h for examples and command-specific guidance.
"""


class ListSection(StrEnum):
    overview = "overview"
    all = "all"
    sources = "sources"
    profiles = "profiles"
    themes = "themes"
    plugins = "plugins"
    skills = "skills"
    agents = "agents"
    mcps = "mcps"


class ManagedTarget(StrEnum):
    all = "all"
    plugins = "plugins"
    skills = "skills"
    agents = "agents"
    mcps = "mcps"
    thirdparty = "thirdparty"


class ShellName(StrEnum):
    bash = "bash"
    zsh = "zsh"
    fish = "fish"
    powershell = "powershell"
    nushell = "nushell"


class RepoProfileLocation(StrEnum):
    root = "root"
    github = "github"


class MenuAction(StrEnum):
    status = "1"
    profiles = "2"
    themes = "3"
    switch = "4"
    switch_exclusive = "5"
    update = "6"
    repo_update = "7"
    quit = "q"


class ListMenuAction(StrEnum):
    overview = "1"
    profiles = "2"
    themes = "3"
    sources = "4"
    plugins = "5"
    skills = "6"
    agents = "7"
    mcps = "8"
    all = "9"
    quit = "q"


app = typer.Typer(
    name="copilot-plugin-manager",
    no_args_is_help=False,
    add_completion=False,
    invoke_without_command=True,
    rich_markup_mode="rich",
    help=APP_HELP,
    epilog=APP_EPILOG,
    context_settings={"help_option_names": ["-h", "--help"]},
)
completion_app = typer.Typer(
    no_args_is_help=True,
    rich_markup_mode="rich",
    help="Manage shell completion snippets, scripts, and installed completion files.",
)
app.add_typer(completion_app, name="completion", short_help="Manage shell completion.")


def get_manager() -> PluginManager:
    return PluginManager(load_catalog_bundle(), ManagerPaths.from_environment())


def _cwd(path: Path | None) -> Path:
    return (path or Path.cwd()).resolve()


def _active_target(manager: PluginManager, current: Path):
    active_name = manager.read_active_target(current)
    active_target = manager.catalog.resolve_target(active_name) if active_name in {*manager.catalog.profiles, *manager.catalog.themes} else None
    return active_name, active_target


def _revision_table(title: str, revisions: dict[str, str | None]) -> Table:
    table = Table(title=title, box=box.ROUNDED, expand=True)
    table.add_column("Source")
    table.add_column("Revision")
    for name, revision in revisions.items():
        table.add_row(name, revision or "unknown")
    return table


def _print_sync_warnings(manager: PluginManager) -> None:
    if manager.sync_warnings:
        console().print(render_sync_warnings(manager.sync_warnings))


def _reset_terminal_keyboard_protocol() -> None:
    stream = getattr(sys, "__stdout__", None)
    if stream is None or not hasattr(stream, "isatty") or not stream.isatty():
        return
    try:
        stream.write("\x1b[<u")
        stream.flush()
    except OSError:
        return


def _prompt_text(message: str, default: str | None = None) -> str:
    _reset_terminal_keyboard_protocol()
    return typer.prompt(message, default=default).strip()


def _confirm_text(message: str, default: bool = False) -> bool:
    _reset_terminal_keyboard_protocol()
    return typer.confirm(message, default=default)


def _supports_interactive_menu() -> bool:
    return sys.stdin.isatty() and sys.stdout.isatty()


def _render_status_snapshot(manager: PluginManager, current: Path) -> None:
    snapshot = manager.status_snapshot(current)
    for renderable in render_status(snapshot, str(manager.paths.copilot_home)):
        console().print(renderable)


def _menu_table(manager: PluginManager, current: Path) -> Table:
    active_name, _ = _active_target(manager, current)
    repo_hint = manager.repo_profile_hint(current)
    table = Table(title="Copilot Plugin Manager", box=box.ROUNDED, expand=True)
    table.add_column("Key", style="cyan", no_wrap=True, width=4)
    table.add_column("Action", style="bold white", no_wrap=True, width=20)
    table.add_column("What it does", style="white", overflow="fold")
    table.add_row("1", "status", "Show the current repo-aware Copilot state.")
    table.add_row("2", "profiles", "Browse bundled profiles.")
    table.add_row("3", "themes", "Browse bundled themes.")
    table.add_row("4", "switch", "Activate a profile or theme.")
    table.add_row("5", "switch-exclusive", "Activate a target and prune managed plugins not in it.")
    table.add_row("6", "update", "Update managed content for this repository context.")
    table.add_row("7", "repo-update", "Refresh tracked upstream source checkouts.")
    table.add_row("q", "quit", "Exit the menu.")
    subtitle = [
        f"cwd: {current}",
        f"active: {active_name or 'none'}",
    ]
    if repo_hint:
        subtitle.append(f"repo hint: {repo_hint}")
    subtitle.append(_scope_caption(manager, current))
    table.caption = " | ".join(subtitle)
    return table


def _prompt_menu_action() -> MenuAction:
    while True:
        raw = _prompt_text("Choose an action", default=MenuAction.status.value).lower()
        if raw in {action.value for action in MenuAction}:
            return MenuAction(raw)
        console().print("[red]Unknown choice. Pick 1-7 or q.[/red]")


def _prompt_managed_target(default: ManagedTarget = ManagedTarget.all) -> ManagedTarget:
    choices = ", ".join(target.value for target in ManagedTarget)
    while True:
        raw = _prompt_text(f"Managed target [{choices}]", default=default.value).lower()
        try:
            return ManagedTarget(raw)
        except ValueError:
            console().print(f"[red]Unknown target '{raw}'. Choose one of: {choices}.[/red]")


def _scope_caption(manager: PluginManager, current: Path) -> str:
    agent_scope_getter = getattr(manager, "agent_scope", None)
    mcp_scope_getter = getattr(manager, "mcp_scope", None)
    mcp_profile_getter = getattr(manager, "mcp_profile", None)
    return (
        f"agent scope: {agent_scope_getter(current) if callable(agent_scope_getter) else 'global'}"
        f" | mcp scope: {mcp_scope_getter(current) if callable(mcp_scope_getter) else 'global'}"
        f" | mcp profile: {(mcp_profile_getter(current) if callable(mcp_profile_getter) else None) or 'none'}"
    )


def _maybe_save_repo_profile(manager: PluginManager, current: Path, target_name: str) -> None:
    if not _confirm_text("Save this target as the repo-local target hint?", default=False):
        return
    while True:
        location = _prompt_text(
            "Repo target hint location [root/github]",
            default=RepoProfileLocation.root.value,
        ).lower()
        try:
            repo_location = RepoProfileLocation(location)
        except ValueError:
            console().print("[red]Unknown repo target hint location. Choose 'root' or 'github'.[/red]")
            continue
        break
    profile_path = manager.write_repo_profile(current, target_name, repo_location.value)
    console().print(f"Saved repo target hint to {profile_path}")


def _run_interactive_menu(manager: PluginManager, current: Path) -> None:
    while True:
        console().print(_menu_table(manager, current))
        action = _prompt_menu_action()
        if action is MenuAction.quit:
            return
        match action:
            case MenuAction.status:
                _render_status_snapshot(manager, current)
            case MenuAction.profiles:
                console().print(render_profiles(manager.catalog))
            case MenuAction.themes:
                active_name, _ = _active_target(manager, current)
                console().print(render_themes(manager.catalog, active_name or None))
            case MenuAction.switch | MenuAction.switch_exclusive:
                active_name, _ = _active_target(manager, current)
                default_target = manager.repo_profile_hint(current) or active_name or "minimal"
                target_name = _prompt_text("Target to activate", default=default_target)
                activation = manager.switch_target(
                    target_name,
                    current,
                    exclusive_plugins=action is MenuAction.switch_exclusive,
                )
                console().print(build_target_tree(manager.catalog, activation))
                _print_sync_warnings(manager)
                _maybe_save_repo_profile(manager, current, activation.name)
            case MenuAction.update:
                manager.manage_target("update", _prompt_managed_target().value, current)
                _print_sync_warnings(manager)
            case MenuAction.repo_update:
                revisions = manager.repo_update(current, remote=_confirm_text("Refresh remote refs too?", default=True))
                console().print(_revision_table("Source revisions", revisions))
        if not _confirm_text("Do you want to choose another action?", default=False):
            return


def _render_list_section(manager: PluginManager, current: Path, section: ListSection) -> None:
    active_name, active_target = _active_target(manager, current)
    repo_hint = manager.repo_profile_hint(current)
    term = console()
    match section:
        case ListSection.overview:
            for renderable in render_overview(manager.catalog, active_target, repo_hint):
                term.print(renderable)
        case ListSection.profiles:
            term.print(render_overview(manager.catalog, active_target, repo_hint)[1])
        case ListSection.themes:
            term.print(render_themes(manager.catalog, active_name or None))
        case ListSection.sources:
            term.print(render_repositories(manager.catalog))
        case ListSection.plugins:
            term.print(render_plugins(manager.catalog))
        case ListSection.skills:
            term.print(render_providers(manager.catalog, "skill"))
        case ListSection.agents:
            term.print(render_providers(manager.catalog, "agent"))
        case ListSection.mcps:
            term.print(render_mcps(manager.catalog))
        case ListSection.all:
            for renderable in render_overview(manager.catalog, active_target, repo_hint):
                term.print(renderable)
            term.print(render_repositories(manager.catalog))
            term.print(render_plugins(manager.catalog))
            term.print(render_providers(manager.catalog, "skill"))
            term.print(render_providers(manager.catalog, "agent"))
            term.print(render_mcps(manager.catalog))


def _list_menu_table(manager: PluginManager, current: Path) -> Table:
    active_name, _ = _active_target(manager, current)
    table = Table(title="Catalog browser", box=box.ROUNDED, expand=True)
    table.add_column("Key", style="cyan", no_wrap=True, width=4)
    table.add_column("View", style="bold white", no_wrap=True, width=16)
    table.add_column("What it shows", style="white", overflow="fold")
    table.add_row("1", "overview", "Compact overview with active state, profiles, and themes.")
    table.add_row("2", "profiles", "Bundled profile catalog.")
    table.add_row("3", "themes", "Theme bundles across plugins, skills, and agents.")
    table.add_row("4", "sources", "Tracked upstream repositories and revisions.")
    table.add_row("5", "plugins", "Managed plugin catalog.")
    table.add_row("6", "skills", "Skill provider catalog.")
    table.add_row("7", "agents", "Agent provider catalog.")
    table.add_row("8", "mcps", "Managed MCP catalog.")
    table.add_row("9", "all", "Full catalog dump.")
    table.add_row("q", "quit", "Exit the catalog browser.")
    table.caption = f"cwd: {current} | active: {active_name or 'none'} | {_scope_caption(manager, current)}"
    return table


def _prompt_list_action() -> ListMenuAction:
    while True:
        raw = _prompt_text("Choose a catalog view", default=ListMenuAction.overview.value).lower()
        if raw in {action.value for action in ListMenuAction}:
            return ListMenuAction(raw)
        console().print("[red]Unknown choice. Pick 1-9 or q.[/red]")


def _run_list_menu(manager: PluginManager, current: Path) -> None:
    section_map = {
        ListMenuAction.overview: ListSection.overview,
        ListMenuAction.profiles: ListSection.profiles,
        ListMenuAction.themes: ListSection.themes,
        ListMenuAction.sources: ListSection.sources,
        ListMenuAction.plugins: ListSection.plugins,
        ListMenuAction.skills: ListSection.skills,
        ListMenuAction.agents: ListSection.agents,
        ListMenuAction.mcps: ListSection.mcps,
        ListMenuAction.all: ListSection.all,
    }
    while True:
        console().print(_list_menu_table(manager, current))
        action = _prompt_list_action()
        if action is ListMenuAction.quit:
            return
        _render_list_section(manager, current, section_map[action])
        if not _confirm_text("Browse another catalog view?", default=False):
            return


def _completion_command():
    return get_command(app)


@app.callback()
def callback(
    ctx: typer.Context,
    version: Annotated[
        bool,
        typer.Option(
            "--version",
            help="Show the installed copilot-plugin-manager version and exit.",
            is_eager=True,
            rich_help_panel="Global options",
        ),
    ] = False,
) -> None:
    """Open the guided menu or fall back to a compact status view."""
    if version:
        typer.echo(__version__)
        raise typer.Exit()
    if ctx.invoked_subcommand is None:
        manager = get_manager()
        current = _cwd(None)
        if _supports_interactive_menu():
            _run_interactive_menu(manager, current)
            raise typer.Exit()
        _render_status_snapshot(manager, current)
        raise typer.Exit()


@app.command(
    "list",
    short_help="Browse bundled catalogs and active views.",
    help=(
        "Render overview data or drill into bundled repository sources, themes, profiles, plugins, skills, "
        "and agent catalogs. Use [cyan]overview[/cyan] for the default summary or [cyan]all[/cyan] for a full dump."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager list overview\n"
        "  copilot-plugin-manager list all\n"
        "  copilot-plugin-manager list plugins\n"
        "  copilot-plugin-manager list skills --cwd /path/to/repo"
    ),
)
def list_command(
    ctx: typer.Context,
    section: Annotated[
        ListSection | None,
        typer.Argument(help="Which catalog view to render. Omit in an interactive terminal to open the catalog browser."),
    ] = None,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository detection and profile hint resolution.",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Render a specific catalog section for the current or supplied repository context."""
    manager = get_manager()
    current = _cwd(cwd)
    del ctx
    if section is None:
        if _supports_interactive_menu():
            _run_list_menu(manager, current)
            return
        section = ListSection.overview
    _render_list_section(manager, current, section)


@app.command(
    "status",
    short_help="Show active state and installed content.",
    help=("Inspect the active target, repository hinting, Copilot home, and counts of installed local skills, agents, and plugins for the selected repository context."),
    epilog="Example:\n  copilot-plugin-manager status --cwd /path/to/repo",
)
def status_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository detection and state lookup.",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Render a status snapshot for the current repository context."""
    manager = get_manager()
    snapshot = manager.status_snapshot(_cwd(cwd))
    for renderable in render_status(snapshot, str(manager.paths.copilot_home)):
        console().print(renderable)


@app.command(
    "menu",
    short_help="Open the guided interactive menu.",
    help="Open the guided interactive menu used by the default no-subcommand experience.",
    epilog="Example:\n  copilot-plugin-manager menu",
)
def menu_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository detection and menu actions.",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Open the guided interactive menu."""
    manager = get_manager()
    current = _cwd(cwd)
    if not _supports_interactive_menu():
        _render_status_snapshot(manager, current)
        return
    _run_interactive_menu(manager, current)


@app.command(
    "install",
    short_help="Install managed content.",
    help=(
        "Install plugins, local skills, local agents, or all managed content for the selected scope. "
        "Use [cyan]thirdparty[/cyan] to sync skills and agents without touching plugins."
    ),
    epilog="\b\nExamples:\n  copilot-plugin-manager install all\n  copilot-plugin-manager install plugins\n  copilot-plugin-manager install thirdparty --cwd /path/to/repo",
)
def install_command(
    target: Annotated[
        ManagedTarget,
        typer.Argument(help="Which managed content scope to install."),
    ] = ManagedTarget.all,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-aware provider syncing.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
    mcp_scope: Annotated[
        str | None,
        typer.Option(
            "--mcp-scope",
            help="Override the effective MCP scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Install the selected managed content scope."""
    manager = get_manager()
    manager.manage_target(
        "install",
        target.value,
        _cwd(cwd),
        agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
        mcp_scope=_parse_scope(mcp_scope) if mcp_scope is not None else None,
    )
    _print_sync_warnings(manager)


@app.command(
    "update",
    short_help="Update managed content.",
    help=("Update plugins, local skills, local agents, or all managed content for the selected scope. This is the preferred command after a [cyan]repo-update[/cyan] refresh."),
    epilog="\b\nExamples:\n  copilot-plugin-manager update all\n  copilot-plugin-manager update thirdparty\n  copilot-plugin-manager update agents --cwd /path/to/repo",
)
def update_command(
    target: Annotated[
        ManagedTarget,
        typer.Argument(help="Which managed content scope to update."),
    ] = ManagedTarget.all,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-aware provider syncing.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
    mcp_scope: Annotated[
        str | None,
        typer.Option(
            "--mcp-scope",
            help="Override the effective MCP scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Update the selected managed content scope."""
    manager = get_manager()
    manager.manage_target(
        "update",
        target.value,
        _cwd(cwd),
        agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
        mcp_scope=_parse_scope(mcp_scope) if mcp_scope is not None else None,
    )
    _print_sync_warnings(manager)


@app.command(
    "delete",
    short_help="Remove managed content.",
    help=("Delete plugins, local skills, local agents, or all managed content for the selected scope. Use with care when removing [cyan]all[/cyan]."),
    epilog="\b\nExamples:\n  copilot-plugin-manager delete plugins\n  copilot-plugin-manager delete thirdparty\n  copilot-plugin-manager delete all --cwd /path/to/repo",
)
def delete_command(
    target: Annotated[
        ManagedTarget,
        typer.Argument(help="Which managed content scope to delete."),
    ] = ManagedTarget.all,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-aware provider syncing.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
    mcp_scope: Annotated[
        str | None,
        typer.Option(
            "--mcp-scope",
            help="Override the effective MCP scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Delete the selected managed content scope."""
    get_manager().manage_target(
        "delete",
        target.value,
        _cwd(cwd),
        agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
        mcp_scope=_parse_scope(mcp_scope) if mcp_scope is not None else None,
    )


@app.command(
    "switch",
    short_help="Activate a profile or theme.",
    help=(
        "Switch to a profile or theme while preserving unrelated installed plugins when possible. "
        "Use [cyan]list profiles[/cyan], [cyan]list themes[/cyan], or [cyan]docs/THEMES.md[/cyan] "
        "to inspect the current composition before saving a repo-local target hint."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager list profiles\n"
        "  copilot-plugin-manager switch minimal\n"
        "  copilot-plugin-manager switch minimal --cwd /path/to/repo --save-repo-profile"
    ),
)
def switch_command(
    target: Annotated[
        str,
        typer.Argument(help="Profile or theme name to activate."),
    ],
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-specific activation state.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    save_repo_profile: Annotated[
        bool,
        typer.Option(
            "--save-repo-profile",
            help="Write the selected profile or theme into a repo-local target hint file for easier future activation.",
            rich_help_panel="Repository context",
        ),
    ] = False,
    repo_profile_location: Annotated[
        RepoProfileLocation,
        typer.Option(
            "--repo-profile-location",
            help="Where to write the repo-local target hint when --save-repo-profile is used.",
            rich_help_panel="Repository context",
        ),
    ] = RepoProfileLocation.root,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Switch to a profile or theme without aggressively pruning unrelated plugins."""
    manager = get_manager()
    current = _cwd(cwd)
    activation = manager.switch_target(
        target,
        current,
        exclusive_plugins=False,
        agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
    )
    console().print(build_target_tree(manager.catalog, activation))
    if save_repo_profile:
        profile_path = manager.write_repo_profile(current, activation.name, repo_profile_location.value)
        console().print(f"Saved repo target hint to {profile_path}")
    _print_sync_warnings(manager)


@app.command(
    "switch-exclusive",
    short_help="Activate a profile or theme exclusively.",
    help=(
        "Switch to a profile or theme and prune managed plugins that are not part of the target. "
        "Use [cyan]list profiles[/cyan], [cyan]list themes[/cyan], or [cyan]docs/THEMES.md[/cyan] "
        "to inspect the current composition before saving a repo-local target hint."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager list themes\n"
        "  copilot-plugin-manager switch-exclusive minimal\n"
        "  copilot-plugin-manager switch-exclusive minimal --cwd /path/to/repo --save-repo-profile"
    ),
)
def switch_exclusive_command(
    target: Annotated[
        str,
        typer.Argument(help="Profile or theme name to activate."),
    ],
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-specific activation state.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    save_repo_profile: Annotated[
        bool,
        typer.Option(
            "--save-repo-profile",
            help="Write the selected profile or theme into a repo-local target hint file for easier future activation.",
            rich_help_panel="Repository context",
        ),
    ] = False,
    repo_profile_location: Annotated[
        RepoProfileLocation,
        typer.Option(
            "--repo-profile-location",
            help="Where to write the repo-local target hint when --save-repo-profile is used.",
            rich_help_panel="Repository context",
        ),
    ] = RepoProfileLocation.root,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this command. Defaults to the repo config when omitted.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Switch to a profile or theme and keep managed plugins aligned exactly to the target."""
    manager = get_manager()
    current = _cwd(cwd)
    activation = manager.switch_target(
        target,
        current,
        exclusive_plugins=True,
        agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
    )
    console().print(build_target_tree(manager.catalog, activation))
    if save_repo_profile:
        profile_path = manager.write_repo_profile(current, activation.name, repo_profile_location.value)
        console().print(f"Saved repo target hint to {profile_path}")
    _print_sync_warnings(manager)


@app.command(
    "repo-update",
    short_help="Refresh upstream source checkouts.",
    help=("Refresh configured source repositories from submodules or cached clones and print observed revisions. Run this before syncing third-party skills or agents."),
    epilog="\b\nExamples:\n  copilot-plugin-manager repo-update\n  copilot-plugin-manager repo-update --remote\n  copilot-plugin-manager repo-update --cwd /path/to/repo",
)
def repo_update_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used to locate the project root and configured submodules.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    remote: Annotated[
        bool,
        typer.Option(
            "--remote/--no-remote",
            help="Refresh remote refs when updating submodules or cache clones.",
            rich_help_panel="Sync behavior",
        ),
    ] = True,
) -> None:
    """Update tracked upstream repositories and report their current revisions."""
    revisions = get_manager().repo_update(_cwd(cwd), remote=remote)
    console().print(_revision_table("Source revisions", revisions))


@app.command(
    "self-update",
    short_help="Pull this checkout forward.",
    help=("Update the current git checkout and then refresh the tracked upstream source repositories. This command requires running inside a git checkout of the manager itself."),
    epilog="Example:\n  copilot-plugin-manager self-update",
)
def self_update_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used to locate the managed git checkout.",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Update this checkout from git and then run a repository refresh."""
    revisions = get_manager().self_update(_cwd(cwd))
    console().print(_revision_table("Updated revisions", revisions))


def _mcp_results_table(title: str, results: dict[str, str]) -> None:
    from rich import box
    from rich.table import Table

    table = Table(title=title, box=box.ROUNDED, expand=True)
    table.add_column("MCP")
    table.add_column("Action")
    for name, action in results.items():
        style = {"added": "green", "updated": "yellow", "removed": "red", "skipped": "dim"}.get(action, "")
        table.add_row(name, action, style=style)
    console().print(table)


def _parse_scope(scope: str) -> Literal["global", "local"]:
    """Validate and return a typed scope literal, exiting on unknown values."""
    if scope not in {"global", "local"}:
        console().print(f"[red]Unknown scope '{scope}'. Use 'global' or 'local'.[/red]")
        raise typer.Exit(1)
    return cast(Literal["global", "local"], scope)


@app.command(
    "mcp-sync",
    short_help="Sync default and local MCP servers.",
    help=(
        "Reconcile the MCP server catalog with [cyan]~/.copilot/mcp-config.json[/cyan]. "
        "Adds missing entries, updates outdated ones, and merges local MCP definitions "
        "from [cyan].vscode/mcp.json[/cyan] in the current repository. "
        "Uses npm version tags where available; falls back to pinned SHA if none exists."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager mcp-sync\n"
        "  copilot-plugin-manager mcp-sync --cwd /path/to/repo\n"
        "  copilot-plugin-manager mcp-sync --no-probe-version\n"
        "  copilot-plugin-manager mcp-sync --remove-unlisted"
    ),
)
def mcp_sync_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for local MCP discovery (.vscode/mcp.json).",
            rich_help_panel="Repository context",
        ),
    ] = None,
    probe_version: Annotated[
        bool,
        typer.Option(
            "--probe-version/--no-probe-version",
            help="Probe npm registry for the latest version tag before writing the config.",
            rich_help_panel="Sync behaviour",
        ),
    ] = True,
    remove_unlisted: Annotated[
        bool,
        typer.Option(
            "--remove-unlisted/--keep-unlisted",
            help="Remove MCP entries from the config that are not in the catalog or local definitions.",
            rich_help_panel="Sync behaviour",
        ),
    ] = False,
) -> None:
    """Sync catalog and local MCP servers into ~/.copilot/mcp-config.json."""
    manager = get_manager()
    results = manager.reconcile_mcps(_cwd(cwd), probe_version=probe_version, remove_unlisted=remove_unlisted)
    _mcp_results_table("MCP sync results", results)


@app.command(
    "mcp-add",
    short_help="Add a single MCP server entry.",
    help=(
        "Add or overwrite a named MCP server entry in [cyan]~/.copilot/mcp-config.json[/cyan]. "
        "For catalog MCPs, version is probed from npm automatically. "
        "Pass [cyan]--no-probe-version[/cyan] to skip version probing."
    ),
    epilog=("\b\nExamples:\n  copilot-plugin-manager mcp-add playwright\n  copilot-plugin-manager mcp-add context7\n  copilot-plugin-manager mcp-add my-custom --no-probe-version"),
)
def mcp_add_command(
    name: Annotated[
        str,
        typer.Argument(help="Catalog MCP name to add (see 'list mcps')."),
    ],
    probe_version: Annotated[
        bool,
        typer.Option(
            "--probe-version/--no-probe-version",
            help="Probe package registry for the latest version tag.",
            rich_help_panel="Sync behaviour",
        ),
    ] = True,
    scope: Annotated[
        str,
        typer.Option(
            "--scope",
            help="Where to write the entry: 'global' (~/.copilot/mcp-config.json) or 'local' (.vscode/mcp.json).",
            rich_help_panel="Scope",
        ),
    ] = "global",
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory (required when --scope local).",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Add or refresh a single MCP server entry from the catalog."""
    resolved_scope = _parse_scope(scope)
    current = _cwd(cwd)
    manager = get_manager()
    if name not in manager.catalog.mcps:
        console().print(f"[red]Unknown MCP '{name}'. Run 'list mcps' to see available entries.[/red]")
        raise typer.Exit(1)
    record = manager.catalog.mcps[name]
    state = manager.sync_mcp(name, record, probe_version=probe_version, scope=resolved_scope, cwd=current)
    version_info = state.installed_version or state.installed_sha or "latest"
    dest = ".vscode/mcp.json" if resolved_scope == "local" else "~/.copilot/mcp-config.json"
    console().print(f"[green]Added MCP '{name}' ({record.kind}, {version_info}) → {dest}[/green]")


@app.command(
    "mcp-remove",
    short_help="Remove a single MCP server entry.",
    help=("Remove a named MCP server entry from [cyan]~/.copilot/mcp-config.json[/cyan] and/or [cyan].vscode/mcp.json[/cyan]."),
    epilog=("\b\nExamples:\n  copilot-plugin-manager mcp-remove playwright\n  copilot-plugin-manager mcp-remove context7 --cwd /path/to/repo"),
)
def mcp_remove_command(
    name: Annotated[
        str,
        typer.Argument(help="MCP server name to remove."),
    ],
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory; when provided, also removes from .vscode/mcp.json.",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Remove a named MCP server entry from global and/or local config."""
    current = _cwd(cwd) if cwd is not None else None
    manager = get_manager()
    if removed := manager.remove_mcp(name, current):  # noqa: F841
        console().print(f"[yellow]Removed MCP '{name}' from config.[/yellow]")
    else:
        console().print(f"[dim]MCP '{name}' was not present in config.[/dim]")


@app.command(
    "mcp-move",
    short_help="Move an MCP entry between global and local scope.",
    help=(
        "Move a named MCP server entry between the global config "
        "([cyan]~/.copilot/mcp-config.json[/cyan]) and the local repo config "
        "([cyan].vscode/mcp.json[/cyan]).\n\n"
        "Use [cyan]--to local[/cyan] to restrict an MCP to the current repository, "
        "and [cyan]--to global[/cyan] to promote it back to the user-wide config.\n\n"
        "After moving to local scope, [cyan]mcp-sync[/cyan] will not overwrite the "
        "entry in the global config until you move it back with [cyan]--to global[/cyan]."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager mcp-move playwright --to local\n"
        "  copilot-plugin-manager mcp-move playwright --to global\n"
        "  copilot-plugin-manager mcp-move context7 --to local --cwd /path/to/repo"
    ),
)
def mcp_move_command(
    name: Annotated[
        str,
        typer.Argument(help="MCP server name to move."),
    ],
    to: Annotated[
        str,
        typer.Option(
            "--to",
            help="Target scope: 'global' or 'local'.",
            rich_help_panel="Scope",
        ),
    ],
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory (used to locate .vscode/mcp.json).",
            rich_help_panel="Repository context",
        ),
    ] = None,
) -> None:
    """Move an MCP entry between global (~/.copilot/mcp-config.json) and local (.vscode/mcp.json) scope."""
    resolved_scope = _parse_scope(to)
    current = _cwd(cwd)
    manager = get_manager()
    try:
        manager.move_mcp_to_scope(name, resolved_scope, current)
    except KeyError as exc:
        console().print(f"[red]{exc}[/red]")
        raise typer.Exit(1) from exc
    src = ".vscode/mcp.json" if resolved_scope == "global" else "~/.copilot/mcp-config.json"
    dst = "~/.copilot/mcp-config.json" if resolved_scope == "global" else ".vscode/mcp.json"
    console().print(f"[green]Moved MCP '{name}' from {src} → {dst}[/green]")


@app.command(
    "repo-init",
    short_help="Initialize repo-local target state.",
    help=(
        "Explicitly write a repo-local target hint file and optional repo config for the selected repository. "
        "Use this when [cyan]status[/cyan] shows no repo hint yet and you want a safe, explicit initialization step."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager repo-init minimal\n"
        "  copilot-plugin-manager repo-init --cwd /path/to/repo --repo-profile-location github\n"
        "  copilot-plugin-manager repo-init minimal --agent-scope local --mcp-scope local --mcp-profile team"
    ),
)
def repo_init_command(
    target: Annotated[
        str | None,
        typer.Argument(help="Optional profile or theme name to persist. Defaults to the current active target when omitted."),
    ] = None,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used to locate the repository root and repo-local config files.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    repo_profile_location: Annotated[
        RepoProfileLocation,
        typer.Option(
            "--repo-profile-location",
            help="Where to write the repo-local target hint file.",
            rich_help_panel="Repository context",
        ),
    ] = RepoProfileLocation.root,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Persist the default repo agent scope: 'global' or 'local'.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    mcp_scope: Annotated[
        str | None,
        typer.Option(
            "--mcp-scope",
            help="Persist the default repo MCP scope: 'global' or 'local'.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    mcp_profile: Annotated[
        str | None,
        typer.Option(
            "--mcp-profile",
            help="Persist the preferred repo MCP profile name.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    force: Annotated[
        bool,
        typer.Option(
            "--force",
            help="Replace an existing repo target hint when it points at a different target.",
            rich_help_panel="Safety",
        ),
    ] = False,
) -> None:
    """Initialize repo-local target state without applying plugins, skills, or agents."""
    manager = get_manager()
    current = _cwd(cwd)
    try:
        activation, profile_path, config_path = manager.initialize_repo(
            current,
            target_name=target,
            location=repo_profile_location.value,
            agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
            mcp_scope=_parse_scope(mcp_scope) if mcp_scope is not None else None,
            mcp_profile=mcp_profile,
            force=force,
        )
    except (KeyError, RuntimeError) as exc:
        console().print(f"[red]{exc}[/red]")
        raise typer.Exit(1) from exc
    console().print(build_target_tree(manager.catalog, activation))
    console().print(f"[green]Initialized repo profile at {profile_path}[/green]")
    if config_path is not None:
        console().print(f"[green]Updated repo settings at {config_path}[/green]")


@app.command(
    "repo-cleanup",
    short_help="Reconcile repo-managed content.",
    help=(
        "Explicitly clean up repo-managed plugins, skills, and agents for the selected target. "
        "This runs an exclusive reconciliation pass to remove stale managed content and reinstall missing managed content."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager repo-cleanup\n"
        "  copilot-plugin-manager repo-cleanup minimal\n"
        "  copilot-plugin-manager repo-cleanup --cwd /path/to/repo --agent-scope local"
    ),
)
def repo_cleanup_command(
    target: Annotated[
        str | None,
        typer.Argument(help="Optional profile or theme name to reconcile. Defaults to the repo hint or active target."),
    ] = None,
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used for repository-specific cleanup.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Override the effective agent scope for this cleanup pass.",
            rich_help_panel="Scope",
        ),
    ] = None,
) -> None:
    """Reconcile managed repo content explicitly when warnings show stale or missing state."""
    manager = get_manager()
    current = _cwd(cwd)
    try:
        activation = manager.cleanup_repo(
            current,
            target_name=target,
            agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
        )
    except (KeyError, RuntimeError) as exc:
        console().print(f"[red]{exc}[/red]")
        raise typer.Exit(1) from exc
    console().print(build_target_tree(manager.catalog, activation))
    console().print(f"[green]Reconciled repo-managed content for {activation.name}[/green]")
    _print_sync_warnings(manager)


@app.command(
    "repo-config",
    short_help="View or update repo-local settings.",
    help=(
        "Inspect or write [cyan].github/copilot-plugin-manager.json[/cyan] for the selected repository. "
        "Use this alongside the repo target hint files ([cyan].copilot-profile[/cyan] or "
        "[cyan].github/copilot-profile[/cyan]) to persist repo-local agent scope, MCP scope, "
        "and the preferred MCP profile."
    ),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager repo-config\n"
        "  copilot-plugin-manager repo-config --agent-scope local --cwd /path/to/repo\n"
        "  copilot-plugin-manager switch minimal --cwd /path/to/repo --save-repo-profile\n"
        "  copilot-plugin-manager repo-config --mcp-scope local --mcp-profile team"
    ),
)
def repo_config_command(
    cwd: Annotated[
        Path | None,
        typer.Option(
            "--cwd",
            help="Working directory used to locate the repository root and repo config file.",
            rich_help_panel="Repository context",
        ),
    ] = None,
    agent_scope: Annotated[
        str | None,
        typer.Option(
            "--agent-scope",
            help="Persist the default repo agent scope: 'global' or 'local'.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    mcp_scope: Annotated[
        str | None,
        typer.Option(
            "--mcp-scope",
            help="Persist the default repo MCP scope: 'global' or 'local'.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    mcp_profile: Annotated[
        str | None,
        typer.Option(
            "--mcp-profile",
            help="Persist the preferred repo MCP profile name.",
            rich_help_panel="Preferences",
        ),
    ] = None,
    clear_mcp_profile: Annotated[
        bool,
        typer.Option(
            "--clear-mcp-profile",
            help="Remove any stored repo MCP profile preference.",
            rich_help_panel="Preferences",
        ),
    ] = False,
) -> None:
    """Show or update the repo-local config used to resolve agent and MCP defaults."""
    manager = get_manager()
    current = _cwd(cwd)
    if agent_scope is not None or mcp_scope is not None or mcp_profile is not None or clear_mcp_profile:
        config_path = manager.write_repo_config(
            current,
            agent_scope=_parse_scope(agent_scope) if agent_scope is not None else None,
            mcp_scope=_parse_scope(mcp_scope) if mcp_scope is not None else None,
            mcp_profile="" if clear_mcp_profile else mcp_profile,
        )
        console().print(f"[green]Updated repo config at {config_path}[/green]")
    snapshot = manager.status_snapshot(current)
    console().print(render_repo_config(snapshot, str(manager.paths.copilot_home)))


@completion_app.command(
    "init",
    short_help="Print shell completion setup.",
    help=(
        "Print a shell-specific startup snippet for bash, zsh, fish, PowerShell, or Nushell. "
        "Use this for quick setup in your shell config. For managed files, use "
        "[cyan]completion install[/cyan] or [cyan]completion script[/cyan]."
    ),
    epilog=(
        '\b\nExamples:\n  eval "$(copilot-plugin-manager completion init bash)"\n'
        "  copilot-plugin-manager completion init powershell\n"
        "  copilot-plugin-manager completion init nushell"
    ),
)
def completion_init_command(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to generate an init snippet for."),
    ],
) -> None:
    """Print a completion/init snippet for the requested shell."""
    typer.echo(shell_init_snippet(shell.value))


@completion_app.command(
    "script",
    short_help="Print a full shell completion script.",
    help=("Render the full completion source for a supported shell. Redirect this output to inspect it, save it manually, or pair it with [cyan]completion install[/cyan]."),
    epilog=(
        "\b\nExamples:\n  copilot-plugin-manager completion script bash\n  copilot-plugin-manager completion script fish > ~/.config/fish/completions/copilot-plugin-manager.fish"
    ),
)
def completion_script_subcommand(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to generate a completion script for."),
    ],
) -> None:
    """Print the full completion source for the requested shell."""
    typer.echo(completion_source(_completion_command(), shell.value))


@completion_app.command(
    "install",
    short_help="Write a shell completion file.",
    help=("Write the generated completion script to a user-level location for the selected shell. Use [cyan]--path[/cyan] to override the destination."),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager completion install fish\n"
        "  copilot-plugin-manager completion install bash --path ~/.local/share/bash-completion/completions/copilot-plugin-manager"
    ),
)
def completion_install_subcommand(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to install a completion file for."),
    ],
    path: Annotated[
        Path | None,
        typer.Option(
            "--path",
            help="Optional output path for the generated completion file.",
            rich_help_panel="Output control",
        ),
    ] = None,
) -> None:
    """Install a generated completion file for the requested shell."""
    target = install_completion_script(_completion_command(), shell.value, path=path)
    typer.echo(f"Installed {shell.value} completion to {target}")
    if message := install_completion_message(shell.value, target):
        typer.echo(message)


@app.command(
    "shell-init",
    short_help="Print shell completion setup.",
    hidden=True,
    help=(
        "Print a shell-specific startup snippet for bash, zsh, fish, PowerShell, or Nushell. "
        "Use this for quick setup in your shell config. For managed files, use "
        "[cyan]completion script[/cyan] or [cyan]completion install[/cyan]."
    ),
    epilog=('\b\nExamples:\n  eval "$(copilot-plugin-manager shell-init bash)"\n  copilot-plugin-manager shell-init powershell\n  copilot-plugin-manager shell-init nushell'),
)
def shell_init_command(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to generate an init snippet for."),
    ],
) -> None:
    """Print a completion/init snippet for the requested shell."""
    typer.echo(shell_init_snippet(shell.value))


@app.command(
    "completion-script",
    short_help="Print a full shell completion script.",
    hidden=True,
    help=("Render the full completion source for a supported shell. Redirect this output to inspect it, save it manually, or pair it with [cyan]completion install[/cyan]."),
    epilog=(
        "\b\nExamples:\n  copilot-plugin-manager completion-script bash\n  copilot-plugin-manager completion-script fish > ~/.config/fish/completions/copilot-plugin-manager.fish"
    ),
)
def completion_script_command(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to generate a completion script for."),
    ],
) -> None:
    """Print the full completion source for the requested shell."""
    typer.echo(completion_source(_completion_command(), shell.value))


@app.command(
    "completion-install",
    short_help="Write a shell completion file.",
    hidden=True,
    help=("Write the generated completion script to a user-level location for the selected shell. Use [cyan]--path[/cyan] to override the destination."),
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager completion-install fish\n"
        "  copilot-plugin-manager completion-install bash --path ~/.local/share/bash-completion/completions/copilot-plugin-manager"
    ),
)
def completion_install_command(
    shell: Annotated[
        ShellName,
        typer.Argument(help="Shell to install a completion file for."),
    ],
    path: Annotated[
        Path | None,
        typer.Option(
            "--path",
            help="Optional output path for the generated completion file.",
            rich_help_panel="Output control",
        ),
    ] = None,
) -> None:
    """Install a generated completion file for the requested shell."""
    target = install_completion_script(_completion_command(), shell.value, path=path)
    typer.echo(f"Installed {shell.value} completion to {target}")
    if message := install_completion_message(shell.value, target):
        typer.echo(message)


def main() -> None:
    app()
