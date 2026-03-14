from __future__ import annotations

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
    render_providers,
    render_repositories,
    render_status,
    render_themes,
)

APP_HELP = """[bold]Copilot Plugin Manager[/bold]

Install, update, inspect, and switch GitHub Copilot plugins, local skills, and local agents
from one production-focused Python CLI.

[bold]What happens by default?[/bold]
Running [cyan]copilot-plugin-manager[/cyan] with no subcommand prints a repository-aware overview.
If the current repository declares a Copilot profile hint, that target is resolved automatically.

[bold]Main workflows[/bold]
• [cyan]list[/cyan] to explore bundled catalogs and active state
• [cyan]install[/cyan], [cyan]update[/cyan], or [cyan]delete[/cyan] to manage plugins / skills / agents
• [cyan]switch[/cyan] or [cyan]switch-exclusive[/cyan] to activate a profile or theme
• [cyan]repo-update[/cyan] to refresh upstream sources before syncing third-party content
"""

APP_EPILOG = """\b
Quick start:
  copilot-plugin-manager repo-update --remote
  copilot-plugin-manager list overview
  copilot-plugin-manager install thirdparty
  copilot-plugin-manager switch minimal

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


app = typer.Typer(
    name="copilot-plugin-manager",
    no_args_is_help=False,
    add_completion=True,
    invoke_without_command=True,
    rich_markup_mode="rich",
    help=APP_HELP,
    epilog=APP_EPILOG,
    context_settings={"help_option_names": ["-h", "--help"]},
)


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
    """Render the default repository overview when no subcommand is selected."""
    if version:
        typer.echo(__version__)
        raise typer.Exit()
    if ctx.invoked_subcommand is None:
        manager = get_manager()
        current = Path.cwd().resolve()
        repo_hint = manager.repo_profile_hint(current)
        if repo_hint:
            activation = manager.switch_target(repo_hint, current, exclusive_plugins=False)
            console().print(build_target_tree(manager.catalog, activation))
            raise typer.Exit()
        _, active_target = _active_target(manager, current)
        for renderable in render_overview(manager.catalog, active_target, repo_hint):
            console().print(renderable)
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
    section: Annotated[
        ListSection,
        typer.Argument(help="Which catalog view to render. Use 'overview' for the summary or 'all' for every view."),
    ] = ListSection.overview,
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
) -> None:
    """Install the selected managed content scope."""
    get_manager().manage_target("install", target.value, _cwd(cwd))


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
) -> None:
    """Update the selected managed content scope."""
    get_manager().manage_target("update", target.value, _cwd(cwd))


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
) -> None:
    """Delete the selected managed content scope."""
    get_manager().manage_target("delete", target.value, _cwd(cwd))


@app.command(
    "switch",
    short_help="Activate a profile or theme.",
    help=("Switch to a profile or theme while preserving unrelated installed plugins when possible. Use this for a safer day-to-day activation flow."),
    epilog="\b\nExamples:\n  copilot-plugin-manager switch minimal\n  copilot-plugin-manager switch docs --cwd /path/to/repo",
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
) -> None:
    """Switch to a profile or theme without aggressively pruning unrelated plugins."""
    manager = get_manager()
    activation = manager.switch_target(target, _cwd(cwd), exclusive_plugins=False)
    console().print(build_target_tree(manager.catalog, activation))


@app.command(
    "switch-exclusive",
    short_help="Activate a profile or theme exclusively.",
    help=("Switch to a profile or theme and prune managed plugins that are not part of the target. Use this when you want strict alignment with the selected setup."),
    epilog="Example:\n  copilot-plugin-manager switch-exclusive minimal",
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
) -> None:
    """Switch to a profile or theme and keep managed plugins aligned exactly to the target."""
    manager = get_manager()
    activation = manager.switch_target(target, _cwd(cwd), exclusive_plugins=True)
    console().print(build_target_tree(manager.catalog, activation))


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


def _parse_mcp_scope(scope: str) -> Literal["global", "local"]:
    """Validate and return a typed MCP scope literal, exiting on unknown values."""
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
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager mcp-add playwright\n"
        "  copilot-plugin-manager mcp-add context7\n"
        "  copilot-plugin-manager mcp-add my-custom --no-probe-version"
    ),
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
    resolved_scope = _parse_mcp_scope(scope)
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
    epilog=(
        "\b\nExamples:\n"
        "  copilot-plugin-manager mcp-remove playwright\n"
        "  copilot-plugin-manager mcp-remove context7 --cwd /path/to/repo"
    ),
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
    removed = manager.remove_mcp(name, current)
    if removed:
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
    resolved_scope = _parse_mcp_scope(to)
    current = _cwd(cwd)
    manager = get_manager()
    try:
        manager.move_mcp_to_scope(name, resolved_scope, current)
    except KeyError as exc:
        console().print(f"[red]{exc}[/red]")
        raise typer.Exit(1)
    src = ".vscode/mcp.json" if resolved_scope == "global" else "~/.copilot/mcp-config.json"
    dst = "~/.copilot/mcp-config.json" if resolved_scope == "global" else ".vscode/mcp.json"
    console().print(f"[green]Moved MCP '{name}' from {src} → {dst}[/green]")


@app.command(
    "shell-init",
    short_help="Print shell completion setup.",
    help=(
        "Print a shell-specific startup snippet for bash, zsh, fish, PowerShell, or Nushell. "
        "Use this for quick setup in your shell config. For managed files, use "
        "[cyan]completion-script[/cyan] or [cyan]completion-install[/cyan]."
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
    help=("Render the full completion source for a supported shell. Redirect this output to inspect it, save it manually, or pair it with [cyan]completion-install[/cyan]."),
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
