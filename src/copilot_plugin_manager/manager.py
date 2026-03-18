from __future__ import annotations

import hashlib
import json
import re
import shutil
import tomllib
from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal, cast

from pydantic import ValidationError
from rich.progress import BarColumn, Progress, SpinnerColumn, TaskProgressColumn, TextColumn, TimeElapsedColumn

from .catalog import CatalogBundle
from .models import ActivationTarget, EntrypointRecord, McpRecord, McpSyncState, PlannedAction, RepoConfig, RepositorySource, SourceState
from .paths import ManagerPaths, find_project_root, find_repo_profile
from .runner import CommandError, ShellRunner, parse_installed_plugins
from .state import StateStore

LEGACY_SKILLS = [
    "skill-creator",
    "mcp-builder",
    "webapp-testing",
    "frontend-design",
    "pdf",
    "docx",
    "pptx",
    "xlsx",
    "bgpt-paper-search",
    "biopython",
    "chembl-database",
    "clinicaltrials-database",
    "citation-management",
    "exploratory-data-analysis",
    "hypothesis-generation",
    "infographics",
    "latex-posters",
]

LEGACY_AGENTS = [
    "engineering-backend-architect.md",
    "engineering-devops-automator.md",
    "engineering-security-engineer.md",
    "engineering-technical-writer.md",
    "project-management-project-shepherd.md",
    "project-manager-senior.md",
    "testing-reality-checker.md",
    "code-reviewer.md",
    "debugger.md",
    "security-auditor.md",
    "test-automator.md",
    "mcp-developer.md",
    "refactoring-specialist.md",
    "research-analyst.md",
    "scientific-literature-researcher.md",
    "search-specialist.md",
]


class PluginManager:
    def __init__(
        self,
        catalog: CatalogBundle,
        paths: ManagerPaths,
        runner: ShellRunner | None = None,
        state_store: StateStore | None = None,
    ) -> None:
        self.catalog = catalog
        self.paths = paths
        self.runner = runner or ShellRunner()
        self.state_store = state_store or StateStore(paths)
        self.sync_warnings: list[str] = []

    def repo_profile_hint(self, cwd: Path) -> str:
        return find_repo_profile(cwd, self.paths.copilot_home)

    def repo_profile_path(self, cwd: Path, location: Literal["root", "github"] = "root") -> Path:
        base = find_project_root(cwd) or cwd.resolve()
        return base / (".copilot-profile" if location == "root" else ".github/copilot-profile")

    def write_repo_profile(self, cwd: Path, target_name: str, location: Literal["root", "github"] = "root") -> Path:
        profile_path = self.repo_profile_path(cwd, location)
        profile_path.parent.mkdir(parents=True, exist_ok=True)
        profile_path.write_text(target_name + "\n")
        return profile_path

    def repo_config_path(self, cwd: Path) -> Path:
        return self.paths.repo_config_file(cwd)

    def read_repo_config(self, cwd: Path) -> RepoConfig:
        config_path = self.repo_config_path(cwd)
        if not config_path.exists():
            return RepoConfig()
        try:
            return RepoConfig.model_validate(json.loads(config_path.read_text()))
        except (OSError, json.JSONDecodeError, ValidationError):
            return RepoConfig()

    def write_repo_config(
        self,
        cwd: Path,
        *,
        agent_scope: Literal["global", "local"] | None = None,
        mcp_scope: Literal["global", "local"] | None = None,
        mcp_profile: str | None = None,
    ) -> Path:
        config = self.read_repo_config(cwd)
        if agent_scope is not None:
            config.agents.scope = agent_scope
        if mcp_scope is not None:
            config.mcps.scope = mcp_scope
        if mcp_profile is not None:
            config.mcps.profile = mcp_profile or None
        config_path = self.repo_config_path(cwd)
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(config.model_dump_json(indent=2) + "\n")
        return config_path

    def agent_scope(self, cwd: Path, scope: Literal["global", "local"] | None = None) -> Literal["global", "local"]:
        if scope is not None:
            return scope
        return self.read_repo_config(cwd).agents.scope or "global"

    def mcp_scope(self, cwd: Path, scope: Literal["global", "local"] | None = None) -> Literal["global", "local"]:
        if scope is not None:
            return scope
        return self.read_repo_config(cwd).mcps.scope or "global"

    def mcp_profile(self, cwd: Path) -> str | None:
        return self.read_repo_config(cwd).mcps.profile

    def agent_output_dir(self, cwd: Path, scope: Literal["global", "local"] | None = None) -> Path:
        resolved_scope = self.agent_scope(cwd, scope)
        return self.paths.local_agents_dir(cwd) if resolved_scope == "local" else self.paths.agents_dir

    def read_active_target(self, cwd: Path) -> str:
        repo_state = self.state_store.read_repo_state(cwd)
        if repo_state and repo_state.active_target:
            return repo_state.active_target
        if self.paths.legacy_active_target_file.exists():
            return self.paths.legacy_active_target_file.read_text().strip()
        return ""

    def _resolve_repo_workflow_target(self, cwd: Path, target_name: str | None = None) -> ActivationTarget:
        if target_name is not None:
            return self.catalog.resolve_target(target_name)
        for candidate_name in (self.repo_profile_hint(cwd), self.read_active_target(cwd)):
            if not candidate_name:
                continue
            try:
                return self.catalog.resolve_target(candidate_name)
            except KeyError:
                continue
        raise RuntimeError("No repo target is configured. Run `copilot-plugin-manager repo-init <profile-or-theme>` or `copilot-plugin-manager switch <profile-or-theme>` first.")

    def initialize_repo(
        self,
        cwd: Path,
        *,
        target_name: str | None = None,
        location: Literal["root", "github"] = "root",
        agent_scope: Literal["global", "local"] | None = None,
        mcp_scope: Literal["global", "local"] | None = None,
        mcp_profile: str | None = None,
        force: bool = False,
    ) -> tuple[ActivationTarget, Path, Path | None]:
        activation = self._resolve_repo_workflow_target(cwd, target_name)
        existing_hint = self.repo_profile_hint(cwd)
        if existing_hint and existing_hint != activation.name and not force:
            raise RuntimeError(f"Repo target hint already points to '{existing_hint}'. Pass --force to replace it with '{activation.name}'.")
        profile_path = self.repo_profile_path(cwd, location)
        if profile_path.exists():
            current_value = profile_path.read_text().strip()
            if current_value and current_value != activation.name and not force:
                raise RuntimeError(f"Repo target hint already exists at {profile_path}. Pass --force to replace '{current_value}' with '{activation.name}'.")
        written_profile = self.write_repo_profile(cwd, activation.name, location)
        config_path: Path | None = None
        if agent_scope is not None or mcp_scope is not None or mcp_profile is not None:
            config_path = self.write_repo_config(
                cwd,
                agent_scope=agent_scope,
                mcp_scope=mcp_scope,
                mcp_profile=mcp_profile,
            )
        return activation, written_profile, config_path

    def cleanup_repo(
        self,
        cwd: Path,
        *,
        target_name: str | None = None,
        agent_scope: Literal["global", "local"] | None = None,
    ) -> ActivationTarget:
        activation = self._resolve_repo_workflow_target(cwd, target_name)
        return self.switch_target(activation.name, cwd, exclusive_plugins=True, agent_scope=agent_scope)

    def _reset_sync_warnings(self) -> None:
        self.sync_warnings = []

    def _remember_sync_warnings(self, warnings: list[str]) -> None:
        for warning in warnings:
            if warning not in self.sync_warnings:
                self.sync_warnings.append(warning)

    def _sync_warning(self, provider_name: str, relative_path: str, reason: str) -> str:
        return f"{provider_name}: skipped {relative_path} ({reason})"

    def _new_progress(self) -> Progress:
        return Progress(
            SpinnerColumn(),
            TextColumn("[bold blue]{task.description}"),
            BarColumn(),
            TaskProgressColumn(),
            TimeElapsedColumn(),
            transient=True,
        )

    def _parallel_workers(self, total: int) -> int:
        return max(1, min(4, total))

    def list_installed_plugins(self) -> list[str]:
        self.runner.require("copilot")
        result = self.runner.run(["copilot", "plugin", "list"])
        return [plugin.name for plugin in parse_installed_plugins(result.stdout)]

    def plugin_actions_for_switch(
        self,
        target_name: str,
        installed: list[str],
        exclusive: bool = False,
    ) -> list[PlannedAction]:
        target = self.catalog.resolve_target(target_name)
        desired = self.catalog.target_items(target.themes, "plugins")
        installed_set = set(installed)
        desired_set = set(desired)
        managed_set = set(self.catalog.plugins)
        removals = installed_set - desired_set if exclusive else (installed_set & managed_set) - desired_set
        actions: list[PlannedAction] = [
            PlannedAction(
                category="plugin",
                description=f"Installing plugin {plugin_name}",
                command=(
                    "copilot",
                    "plugin",
                    "install",
                    self.catalog.plugin_install_source(plugin_name),
                ),
            )
            for plugin_name in desired
            if plugin_name not in installed_set
        ]
        for plugin_name in sorted(removals):
            actions.append(
                PlannedAction(
                    category="plugin",
                    description=f"Removing plugin {plugin_name}",
                    command=("copilot", "plugin", "uninstall", plugin_name),
                )
            )
        return actions

    def _execute_actions(
        self,
        actions: list[PlannedAction],
        cwd: Path | None = None,
        description: str = "Applying changes",
        parallel_workers: int = 1,
    ) -> None:
        actionable = [action for action in actions if action.command is not None]
        if not actionable:
            return

        workers = min(self._parallel_workers(len(actionable)), max(1, parallel_workers))
        with self._new_progress() as progress:
            item_task_ids = {action: progress.add_task(action.description, total=1) for action in actionable}
            task_id = progress.add_task(f"{description} total", total=len(actionable))
            if workers == 1:
                for action in actionable:
                    command = action.command
                    if command is None:
                        continue
                    self.runner.run(list(command), cwd=cwd, check=action.check)
                    progress.update(item_task_ids[action], completed=1)
                    progress.advance(task_id)
                return

            future_map: dict[Future[object], PlannedAction] = {}
            with ThreadPoolExecutor(max_workers=workers) as executor:
                for action in actionable:
                    command = action.command
                    if command is None:
                        continue
                    future = executor.submit(self.runner.run, list(command), cwd=cwd, check=action.check)
                    future_map[future] = action
                for future in as_completed(future_map):
                    action = future_map[future]
                    try:
                        future.result()
                    except Exception:
                        progress.update(item_task_ids[action], description=f"{action.description} [failed]")
                        raise
                    progress.update(item_task_ids[action], completed=1, description=f"{action.description} [done]")
                    progress.advance(task_id)

    def _format_copy_error(self, exc: OSError | shutil.Error) -> str:
        if isinstance(exc, shutil.Error) and exc.args:
            details = exc.args[0]
            if isinstance(details, list):
                if problem_paths := [str(source) for source, _destination, _message in details[:3]]:
                    return f"copy failed for {', '.join(problem_paths)}"
            return str(exc)
        return str(exc)

    def _plugin_actions_for_manage(self, operation: str) -> tuple[str, list[PlannedAction]]:
        description = {
            "install": "Installing plugins",
            "update": "Updating plugins",
            "delete": "Removing plugins",
        }.get(operation, "Managing plugins")
        actions: list[PlannedAction] = []
        for name in self.catalog.plugins:
            match operation:
                case "install":
                    command = ("copilot", "plugin", "install", self.catalog.plugin_install_source(name))
                    check = True
                case "update":
                    command = ("copilot", "plugin", "update", name)
                    check = True
                case "delete":
                    command = ("copilot", "plugin", "uninstall", name)
                    check = False
                case _:
                    raise ValueError(f"Unknown plugin operation: {operation}")
            actions.append(
                PlannedAction(
                    category="plugin",
                    description=f"{description[:-1]} {name}",
                    command=command,
                    check=check,
                )
            )
        return description, actions

    def manage_plugins(self, operation: str) -> None:
        self.runner.require("copilot")
        description, actions = self._plugin_actions_for_manage(operation)
        self._execute_actions(
            actions,
            description=description,
            parallel_workers=self._parallel_workers(len(actions)),
        )

    def _resolve_source_checkout(self, source_name: str, cwd: Path) -> Path:
        source = self.catalog.repository_details(source_name)
        current = cwd.resolve()
        for parent in (current, *current.parents):
            candidate = parent / source.submodule_path
            if candidate.exists():
                return candidate
        cached = self.paths.sources_dir / source_name
        return cached if cached.exists() else self._clone_source_checkout(source_name)

    def _clone_source_checkout(self, source_name: str) -> Path:
        source = self.catalog.repository_details(source_name)
        cache_dir = self.paths.sources_dir / source_name
        if cache_dir.exists():
            return cache_dir
        self.paths.ensure_directories()
        self.runner.require("git")
        self.runner.run(
            [
                "git",
                "clone",
                "--depth",
                "1",
                f"https://github.com/{source.owner}/{source.repo}.git",
                str(cache_dir),
            ]
        )
        if cache_dir.exists():
            return cache_dir
        raise RuntimeError(f"Source checkout missing for {source_name}. Initialize the configured submodules or run repo-update first.")

    def _copy_fs_path(self, source: Path, destination: Path) -> None:
        if source.is_symlink() and not source.exists():
            raise FileNotFoundError(f"Dangling symlink: {source}")
        if not source.exists():
            raise FileNotFoundError(f"Missing source path: {source}")
        if destination.exists():
            if destination.is_dir() and not destination.is_symlink():
                shutil.rmtree(destination)
            else:
                destination.unlink()
        if source.is_dir():
            shutil.copytree(source, destination, ignore_dangling_symlinks=True)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)

    def _dangling_symlinks(self, source: Path) -> list[Path]:
        if source.is_symlink() and not source.exists():
            return [source]
        if not source.exists() or not source.is_dir():
            return []
        return sorted(path for path in source.rglob("*") if path.is_symlink() and not path.exists())

    def _resolve_microsoft_skill_directory(self, source_root: Path, candidate: Path) -> Path | None:
        if not candidate.is_symlink():
            return None
        symlink_target = candidate.readlink()
        resolved_target = (candidate.parent / symlink_target).resolve(strict=False)
        if resolved_target.is_dir() and resolved_target.is_relative_to(source_root):
            return resolved_target
        target_name = symlink_target.name
        matches: list[Path] = []
        direct_skill = source_root / ".github" / "skills" / target_name
        if direct_skill.is_dir():
            matches.append(direct_skill)
        plugin_skills_root = source_root / ".github" / "plugins"
        if plugin_skills_root.exists():
            matches.extend(sorted(path for path in plugin_skills_root.glob(f"*/skills/{target_name}") if path.is_dir()))
        return None if len(matches) != 1 else matches[0]

    def _resolve_skill_symlink(self, source_name: str, source_root: Path, candidate: Path) -> Path | None:
        if source_name != "microsoft-skills":
            return None
        return self._resolve_microsoft_skill_directory(source_root, candidate)

    def _copy_skill_path(self, source_name: str, source_root: Path, source: Path, destination: Path) -> None:
        if source.is_symlink() and not source.exists():
            resolved = self._resolve_skill_symlink(source_name, source_root, source)
            if resolved is None:
                return
            source = resolved
        if not source.exists():
            return
        if destination.exists():
            if destination.is_dir() and not destination.is_symlink():
                shutil.rmtree(destination)
            else:
                destination.unlink()
        if source.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
            for child in sorted(source.iterdir(), key=lambda item: item.name):
                self._copy_skill_path(source_name, source_root, child, destination / child.name)
            return
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)

    def _dangling_skill_symlinks(self, source_name: str, source_root: Path, source: Path) -> list[Path]:
        if source.is_symlink() and not source.exists():
            return [] if self._resolve_skill_symlink(source_name, source_root, source) is not None else [source]
        if not source.exists() or not source.is_dir():
            return []
        broken: list[Path] = []
        for path in source.rglob("*"):
            if not path.is_symlink() or path.exists():
                continue
            if self._resolve_skill_symlink(source_name, source_root, path) is None:
                broken.append(path)
        return sorted(broken)

    def _skill_entry_candidates(self, source: Path) -> list[Path]:
        if not source.is_dir():
            return [source]
        children = sorted(item for item in source.iterdir() if item.is_dir() or item.is_symlink())
        return children or [source]

    def _title_from_path(self, source_path: str) -> str:
        stem = Path(source_path).stem
        parts = [part for part in re.split(r"[-_/]", stem) if part]
        return " ".join(part.upper() if len(part) <= 3 else part[:1].upper() + part[1:] for part in parts) or stem

    def _agent_entrypoint(self, provider_name: str, provider_source: str, source_path: str) -> EntrypointRecord | None:
        return self.catalog.entrypoint_for_path("agent", provider_source, source_path, provider=provider_name)

    def _agent_output_name(
        self,
        provider_name: str,
        provider_prefix: str,
        provider_source: str,
        source_path: str,
        *,
        scope: Literal["global", "local"] = "global",
        destination_root: Path | None = None,
    ) -> str:
        entrypoint = self._agent_entrypoint(provider_name, provider_source, source_path)
        if scope == "local":
            local_name = f"{Path(source_path).stem}.agent.md"
            if destination_root is None or not (destination_root / local_name).exists():
                return local_name
            flat = source_path.replace("/", "__").replace("\\", "__").removesuffix(".md")
            return f"{provider_prefix}__{flat}.agent.md"
        if entrypoint is not None:
            return entrypoint.local_output
        flat = source_path.replace("/", "__").replace("\\", "__")
        flat = flat.removesuffix(".md")
        return f"{provider_prefix}__{flat}.agent.md"

    def _render_normalized_agent(
        self,
        source_text: str,
        source_name: str,
        source_path: str,
        entrypoint: EntrypointRecord | None,
    ) -> str:
        title = entrypoint.title if entrypoint is not None else self._title_from_path(source_path)
        description = entrypoint.description if entrypoint is not None else f"Imported Copilot agent derived from {source_name}/{source_path}."
        state = self.state_store.read_source_state(source_name)
        metadata_lines = [
            "Generated by copilot-plugin-manager.",
            f"source: {source_name}",
            f"source_path: {source_path}",
        ]
        if entrypoint and entrypoint.source_url:
            metadata_lines.append(f"source_url: {entrypoint.source_url}")
        if entrypoint and entrypoint.commit_revision:
            metadata_lines.append(f"commit_revision: {entrypoint.commit_revision}")
        if entrypoint and entrypoint.commit_date:
            metadata_lines.append(f"commit_date: {entrypoint.commit_date}")
        if entrypoint and entrypoint.approval_date:
            metadata_lines.append(f"approval_date: {entrypoint.approval_date}")
        if state and state.revision:
            metadata_lines.append(f"revision: {state.revision}")
        if state and state.measured_at:
            metadata_lines.append(f"measured_at: {state.measured_at}")
        metadata = "<!--\n" + "\n".join(metadata_lines) + "\n-->"

        body_lines = source_text.lstrip().splitlines()
        if body_lines and body_lines[0].startswith("#"):
            heading = body_lines[0].lstrip("#").strip()
            if heading.casefold() == title.casefold():
                body_lines = body_lines[1:]
                while body_lines and not body_lines[0].strip():
                    body_lines = body_lines[1:]
        body = "\n".join(body_lines).strip()
        sections = [metadata, "", f"# {title}", "", f"> {description}", ""]
        if body:
            sections.append(body)
        return "\n".join(sections).rstrip() + "\n"

    def _copy_agent_file(
        self,
        source: Path,
        destination: Path,
        source_name: str,
        source_path: str,
        entrypoint: EntrypointRecord | None,
    ) -> None:
        rendered = self._render_normalized_agent(source.read_text(), source_name, source_path, entrypoint)
        if destination.exists():
            destination.unlink()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(rendered)

    def _remove_by_prefix(self, root: Path, prefixes: list[str]) -> None:
        if not root.exists():
            return
        for prefix in prefixes:
            for child in root.iterdir():
                if child.name.startswith(f"{prefix}__"):
                    if child.is_dir() and not child.is_symlink():
                        shutil.rmtree(child)
                    else:
                        child.unlink()

    def _remove_named_items(self, root: Path, names: list[str]) -> None:
        if not root.exists():
            return
        for name in names:
            candidate = root / name
            if candidate.exists():
                if candidate.is_dir() and not candidate.is_symlink():
                    shutil.rmtree(candidate)
                else:
                    candidate.unlink()

    def _prefixed_content_exists(self, root: Path, prefix: str) -> bool:
        if not root.exists():
            return False
        return any(child.name.startswith(f"{prefix}__") for child in root.iterdir())

    def _existing_outputs(self, root: Path, outputs: list[str]) -> bool:
        return bool(outputs) and all((root / output).exists() for output in outputs)

    def _provider_definition_signature(self, kind: Literal["skill", "agent"], provider_name: str) -> str:
        provider = self.catalog.provider_registry(kind)[provider_name]
        payload: dict[str, object] = {
            "kind": kind,
            "source": provider.source,
            "prefix": provider.prefix,
            "roots": provider.roots,
        }
        if kind == "agent":
            payload["entrypoints"] = [
                {
                    "source_path": entry.source_path,
                    "local_output": entry.local_output,
                    "commit_revision": entry.commit_revision,
                }
                for entry in sorted(
                    self.catalog.entrypoint_records(kind, provider=provider_name),
                    key=lambda entry: (entry.source_path, entry.local_output),
                )
            ]
        return hashlib.sha256(json.dumps(payload, sort_keys=True).encode("utf-8")).hexdigest()

    def _provider_outputs_current(
        self,
        kind: Literal["skill", "agent"],
        provider_name: str,
        provider_source: str,
        observed: SourceState,
        root: Path,
        prefix: str,
        *,
        scope: Literal["global", "local"] = "global",
        cwd: Path | None = None,
    ) -> bool:
        stored = self.state_store.read_provider_state(kind, provider_name, scope=scope, cwd=cwd)
        if stored is None or stored.source != provider_source:
            return False
        if stored.warnings:
            return False
        if stored.definition_signature != self._provider_definition_signature(kind, provider_name):
            return False
        previous = SourceState(
            revision=stored.revision,
            manifest_version=stored.manifest_version,
            source_path=stored.source_path,
        )
        if observed.has_comparable_change(previous):
            return False
        return self._existing_outputs(root, stored.outputs) or (not stored.outputs and self._prefixed_content_exists(root, prefix))

    def _record_provider_sync(
        self,
        kind: Literal["skill", "agent"],
        provider_name: str,
        provider_source: str,
        observed: SourceState,
        outputs: list[str],
        warnings: list[str],
        *,
        scope: Literal["global", "local"] = "global",
        cwd: Path | None = None,
    ) -> None:
        if outputs or warnings:
            self.state_store.write_provider_state(
                kind,
                provider_name,
                provider_source,
                observed,
                outputs,
                warnings,
                self._provider_definition_signature(kind, provider_name),
                scope=scope,
                cwd=cwd,
            )
            self._remember_sync_warnings(warnings)
            return
        self.state_store.clear_provider_state(kind, provider_name, scope=scope, cwd=cwd)

    def _clear_provider_states(
        self,
        kind: Literal["skill", "agent"],
        provider_names: list[str],
        *,
        scope: Literal["global", "local"] = "global",
        cwd: Path | None = None,
    ) -> None:
        for provider_name in provider_names:
            self.state_store.clear_provider_state(kind, provider_name, scope=scope, cwd=cwd)

    def _remove_agent_provider_outputs(
        self,
        provider_name: str,
        root: Path,
        *,
        scope: Literal["global", "local"],
        cwd: Path,
    ) -> None:
        stored = self.state_store.read_provider_state("agent", provider_name, scope=scope, cwd=cwd if scope == "local" else None)
        if stored is None or not stored.outputs:
            return
        self._remove_named_items(root, stored.outputs)

    def _claim_agent_source_paths(self, provider_name: str, claimed_source_paths: set[tuple[str, str]]) -> None:
        for entry in self.catalog.entrypoint_records("agent", provider=provider_name):
            claimed_source_paths.add((entry.source, entry.source_path))

    def sync_skill_provider(
        self,
        provider_name: str,
        cwd: Path,
        *,
        source_root: Path | None = None,
        observed: SourceState | None = None,
    ) -> list[str]:
        provider = self.catalog.skill_providers[provider_name]
        source_root = source_root or self._resolve_source_checkout(provider.source, cwd)
        observed = observed or self.current_source_state(source_root)
        self.paths.skills_dir.mkdir(parents=True, exist_ok=True)
        outputs: list[str] = []
        warnings: list[str] = []
        for root in provider.roots:
            source = source_root / root
            if source.is_symlink() and not source.exists():
                resolved_source = self._resolve_skill_symlink(provider.source, source_root, source)
                if resolved_source is None:
                    warnings.append(self._sync_warning(provider_name, root, "dangling symlink"))
                    continue
                source = resolved_source
            if not source.exists():
                warnings.append(self._sync_warning(provider_name, root, "missing source root"))
                continue
            if source.is_file():
                destination = self.paths.skills_dir / f"{provider.prefix}__{source.stem}"
                try:
                    self._copy_skill_path(provider.source, source_root, source, destination)
                except (OSError, shutil.Error) as exc:
                    warnings.append(self._sync_warning(provider_name, root, self._format_copy_error(exc)))
                    continue
                outputs.append(destination.name)
                continue
            for directory in self._skill_entry_candidates(source):
                resolved_directory = self._resolve_skill_symlink(provider.source, source_root, directory) if directory.is_symlink() else directory
                dangling = [
                    self._sync_warning(
                        provider_name,
                        str(path.relative_to(source_root)).replace("\\", "/"),
                        "dangling symlink",
                    )
                    for path in self._dangling_skill_symlinks(provider.source, source_root, directory)
                ]
                destination = self.paths.skills_dir / f"{provider.prefix}__{directory.name}"
                if directory.is_symlink() and not directory.exists() and resolved_directory is None:
                    warnings.extend(dangling)
                    continue
                try:
                    self._copy_skill_path(provider.source, source_root, directory, destination)
                except (OSError, shutil.Error) as exc:
                    warnings.append(
                        self._sync_warning(
                            provider_name,
                            str(directory.relative_to(source_root)).replace("\\", "/"),
                            self._format_copy_error(exc),
                        )
                    )
                    warnings.extend(dangling)
                    continue
                outputs.append(destination.name)
                warnings.extend(dangling)
        synced_outputs = list(dict.fromkeys(outputs))
        self._record_provider_sync("skill", provider_name, provider.source, observed, synced_outputs, list(dict.fromkeys(warnings)))
        return synced_outputs

    def _ensure_commit_available(self, checkout: Path, commit_revision: str) -> None:
        result = self.runner.run(["git", "cat-file", "-e", f"{commit_revision}^{{commit}}"], cwd=checkout, check=False)
        if result.returncode == 0:
            return
        self.runner.run(["git", "fetch", "--depth", "1", "origin", commit_revision], cwd=checkout)

    def _read_agent_source_text(self, checkout: Path, source_path: str, entrypoint: EntrypointRecord | None) -> str:
        normalized_path = source_path.replace("\\", "/")
        if entrypoint is not None and entrypoint.commit_revision:
            self._ensure_commit_available(checkout, entrypoint.commit_revision)
            return self.runner.run(["git", "show", f"{entrypoint.commit_revision}:{normalized_path}"], cwd=checkout).stdout
        source = checkout / normalized_path
        return source.read_text()

    def _write_agent_output(
        self,
        destination: Path,
        source_text: str,
        source_name: str,
        source_path: str,
        entrypoint: EntrypointRecord | None,
    ) -> None:
        rendered = self._render_normalized_agent(source_text, source_name, source_path, entrypoint)
        if destination.exists():
            destination.unlink()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(rendered)

    def sync_agent_provider(
        self,
        provider_name: str,
        cwd: Path,
        *,
        claimed_source_paths: set[tuple[str, str]] | None = None,
        source_root: Path | None = None,
        observed: SourceState | None = None,
        scope: Literal["global", "local"] | None = None,
    ) -> list[str]:
        provider = self.catalog.agent_providers[provider_name]
        source_root = source_root or self._resolve_source_checkout(provider.source, cwd)
        observed = observed or self.current_source_state(source_root)
        resolved_scope = self.agent_scope(cwd, scope)
        destination_root = self.agent_output_dir(cwd, resolved_scope)
        destination_root.mkdir(parents=True, exist_ok=True)
        outputs: list[str] = []
        if entrypoints := sorted(
            self.catalog.entrypoint_records("agent", provider=provider_name),
            key=lambda entry: (entry.source_path, entry.local_output),
        ):
            for entrypoint in entrypoints:
                source_path = entrypoint.source_path
                source = source_root / source_path
                if not source.exists() and not (source_root / ".git").exists():
                    continue
                claim_key = (provider.source, source_path)
                if claimed_source_paths is not None:
                    if claim_key in claimed_source_paths:
                        continue
                    claimed_source_paths.add(claim_key)
                destination_name = self._agent_output_name(
                    provider_name,
                    provider.prefix,
                    provider.source,
                    source_path,
                    scope=resolved_scope,
                    destination_root=destination_root,
                )
                destination = destination_root / destination_name
                try:
                    source_text = self._read_agent_source_text(source_root, source_path, entrypoint)
                except (CommandError, OSError) as exc:
                    commit = entrypoint.commit_revision or "the current checkout"
                    raise RuntimeError(f"Unable to load agent {provider_name}:{source_path} from {provider.source} at {commit}.") from exc
                self._write_agent_output(destination, source_text, provider.source, source_path, entrypoint)
                outputs.append(destination.name)
        else:
            for root in provider.roots:
                source = source_root / root
                if not source.exists():
                    continue
                if source.is_file():
                    source_path = str(root).replace("\\", "/")
                    claim_key = (provider.source, source_path)
                    if claimed_source_paths is not None:
                        if claim_key in claimed_source_paths:
                            continue
                        claimed_source_paths.add(claim_key)
                    destination = destination_root / self._agent_output_name(
                        provider_name,
                        provider.prefix,
                        provider.source,
                        source_path,
                        scope=resolved_scope,
                        destination_root=destination_root,
                    )
                    self._copy_agent_file(
                        source,
                        destination,
                        provider.source,
                        source_path,
                        self._agent_entrypoint(provider_name, provider.source, source_path),
                    )
                    outputs.append(destination.name)
                    continue
                for file_path in sorted(source.rglob("*.md")):
                    if file_path.name == "README.md":
                        continue
                    relative = file_path.relative_to(source_root)
                    source_path = str(relative).replace("\\", "/")
                    claim_key = (provider.source, source_path)
                    if claimed_source_paths is not None:
                        if claim_key in claimed_source_paths:
                            continue
                        claimed_source_paths.add(claim_key)
                    destination = destination_root / self._agent_output_name(
                        provider_name,
                        provider.prefix,
                        provider.source,
                        source_path,
                        scope=resolved_scope,
                        destination_root=destination_root,
                    )
                    self._copy_agent_file(
                        file_path,
                        destination,
                        provider.source,
                        source_path,
                        self._agent_entrypoint(provider_name, provider.source, source_path),
                    )
                    outputs.append(destination.name)
        synced_outputs = list(dict.fromkeys(outputs))
        self._record_provider_sync(
            "agent",
            provider_name,
            provider.source,
            observed,
            synced_outputs,
            [],
            scope=resolved_scope,
            cwd=cwd if resolved_scope == "local" else None,
        )
        return synced_outputs

    def _sync_skill_providers(
        self,
        provider_names: list[str],
        cwd: Path,
        *,
        task_title: str,
        item_label: str,
    ) -> None:
        if not provider_names:
            return
        with self._new_progress() as progress:
            task_id = progress.add_task(task_title, total=len(provider_names))
            for provider_name in provider_names:
                provider = self.catalog.skill_providers[provider_name]
                source_root = self._resolve_source_checkout(provider.source, cwd)
                observed = self.current_source_state(source_root)
                progress.update(task_id, description=f"{item_label} {provider_name}")
                if self._provider_outputs_current("skill", provider_name, provider.source, observed, self.paths.skills_dir, provider.prefix):
                    progress.advance(task_id)
                    continue
                self._remove_by_prefix(self.paths.skills_dir, [provider.prefix])
                self.sync_skill_provider(provider_name, cwd, source_root=source_root, observed=observed)
                progress.advance(task_id)

    def _sync_agent_providers(
        self,
        provider_names: list[str],
        cwd: Path,
        *,
        task_title: str,
        item_label: str,
        scope: Literal["global", "local"] | None = None,
    ) -> None:
        ordered = self.catalog.preferred_provider_order("agent", provider_names)
        if not ordered:
            return

        resolved_scope = self.agent_scope(cwd, scope)
        destination_root = self.agent_output_dir(cwd, resolved_scope)
        claimed_source_paths: set[tuple[str, str]] = set()
        with self._new_progress() as progress:
            task_id = progress.add_task(task_title, total=len(ordered))
            for provider_name in ordered:
                provider = self.catalog.agent_providers[provider_name]
                source_root = self._resolve_source_checkout(provider.source, cwd)
                observed = self.current_source_state(source_root)
                entrypoints = self.catalog.entrypoint_records("agent", provider=provider_name)
                has_claim_conflict = any((entry.source, entry.source_path) in claimed_source_paths for entry in entrypoints)
                progress.update(task_id, description=f"{item_label} {provider_name}")
                if not has_claim_conflict and self._provider_outputs_current(
                    "agent",
                    provider_name,
                    provider.source,
                    observed,
                    destination_root,
                    provider.prefix,
                    scope=resolved_scope,
                    cwd=cwd if resolved_scope == "local" else None,
                ):
                    self._claim_agent_source_paths(provider_name, claimed_source_paths)
                    progress.advance(task_id)
                    continue
                if resolved_scope == "local":
                    self._remove_agent_provider_outputs(provider_name, destination_root, scope=resolved_scope, cwd=cwd)
                else:
                    self._remove_by_prefix(destination_root, [provider.prefix])
                self.sync_agent_provider(
                    provider_name,
                    cwd,
                    claimed_source_paths=claimed_source_paths,
                    source_root=source_root,
                    observed=observed,
                    scope=resolved_scope,
                )
                progress.advance(task_id)

    def manage_skills(self, operation: str, cwd: Path) -> None:
        if operation in {"install", "update"}:
            self._sync_skill_providers(
                list(self.catalog.skill_providers),
                cwd,
                task_title="Syncing skill providers",
                item_label="Syncing skill provider",
            )
            return
        if operation == "delete":
            provider_names = list(self.catalog.skill_providers)
            self._remove_by_prefix(self.paths.skills_dir, [provider.prefix for provider in self.catalog.skill_providers.values()])
            self._clear_provider_states("skill", provider_names)
            return
        raise ValueError(f"Unknown skill operation: {operation}")

    def manage_agents(
        self,
        operation: str,
        cwd: Path,
        *,
        scope: Literal["global", "local"] | None = None,
    ) -> None:
        resolved_scope = self.agent_scope(cwd, scope)
        destination_root = self.agent_output_dir(cwd, resolved_scope)
        if operation in {"install", "update"}:
            self._sync_agent_providers(
                list(self.catalog.agent_providers),
                cwd,
                task_title="Syncing agent providers",
                item_label="Syncing agent provider",
                scope=resolved_scope,
            )
            return
        if operation == "delete":
            provider_names = list(self.catalog.agent_providers)
            if resolved_scope == "local":
                for provider_name in provider_names:
                    self._remove_agent_provider_outputs(provider_name, destination_root, scope=resolved_scope, cwd=cwd)
            else:
                self._remove_by_prefix(destination_root, [provider.prefix for provider in self.catalog.agent_providers.values()])
            self._clear_provider_states("agent", provider_names, scope=resolved_scope, cwd=cwd if resolved_scope == "local" else None)
            return
        raise ValueError(f"Unknown agent operation: {operation}")

    def manage_target(
        self,
        operation: str,
        target: str,
        cwd: Path,
        *,
        agent_scope: Literal["global", "local"] | None = None,
        mcp_scope: Literal["global", "local"] | None = None,
    ) -> None:
        self._reset_sync_warnings()
        resolved_agent_scope = self.agent_scope(cwd, agent_scope)
        resolved_mcp_scope = self.mcp_scope(cwd, mcp_scope)
        match target:
            case "all":
                self.manage_plugins(operation)
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd, scope=resolved_agent_scope)
                self.manage_mcps(operation, cwd, scope=resolved_mcp_scope)
            case "plugins":
                self.manage_plugins(operation)
            case "skills":
                self.manage_skills(operation, cwd)
            case "agents":
                self.manage_agents(operation, cwd, scope=resolved_agent_scope)
            case "mcps":
                self.manage_mcps(operation, cwd, scope=resolved_mcp_scope)
            case "thirdparty":
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd, scope=resolved_agent_scope)
            case _:
                raise ValueError(f"Unknown target: {target}")
        if operation == "delete" and self.paths.legacy_active_target_file.exists():
            self.paths.legacy_active_target_file.write_text("")

    def _remove_unselected_skill_providers(self, desired: list[str]) -> None:
        removed = [name for name in self.catalog.skill_providers if name not in set(desired)]
        self._remove_by_prefix(self.paths.skills_dir, [self.catalog.skill_providers[name].prefix for name in removed])
        self._clear_provider_states("skill", removed)

    def _remove_unselected_agent_providers(
        self,
        desired: list[str],
        cwd: Path,
        *,
        scope: Literal["global", "local"] | None = None,
    ) -> None:
        resolved_scope = self.agent_scope(cwd, scope)
        removed = [name for name in self.catalog.agent_providers if name not in set(desired)]
        destination_root = self.agent_output_dir(cwd, resolved_scope)
        if resolved_scope == "local":
            for provider_name in removed:
                self._remove_agent_provider_outputs(provider_name, destination_root, scope=resolved_scope, cwd=cwd)
        else:
            self._remove_by_prefix(destination_root, [self.catalog.agent_providers[name].prefix for name in removed])
        self._clear_provider_states("agent", removed, scope=resolved_scope, cwd=cwd if resolved_scope == "local" else None)

    def _sync_missing_skill_providers(self, desired: list[str], cwd: Path) -> None:
        self._sync_skill_providers(
            desired,
            cwd,
            task_title="Downloading skill providers",
            item_label="Downloading skill provider",
        )

    def _sync_missing_agent_providers(
        self,
        desired: list[str],
        cwd: Path,
        *,
        scope: Literal["global", "local"] | None = None,
    ) -> None:
        if not desired:
            return
        self._sync_agent_providers(
            desired,
            cwd,
            task_title="Downloading agent providers",
            item_label="Downloading agent provider",
            scope=scope,
        )

    def _provider_outputs_present(
        self,
        kind: Literal["skill", "agent"],
        provider_name: str,
        root: Path,
        *,
        scope: Literal["global", "local"] = "global",
        cwd: Path | None = None,
    ) -> bool:
        provider = self.catalog.provider_registry(kind)[provider_name]
        stored = self.state_store.read_provider_state(kind, provider_name, scope=scope, cwd=cwd)
        if stored is None:
            return False
        return self._existing_outputs(root, stored.outputs) or (not stored.outputs and self._prefixed_content_exists(root, provider.prefix))

    def _collect_target_verification_warnings(
        self,
        target: ActivationTarget,
        *,
        cwd: Path,
        exclusive_plugins: bool,
        agent_scope: Literal["global", "local"] | None = None,
    ) -> list[str]:
        desired_plugins = set(self.catalog.target_items(target.themes, "plugins"))
        desired_skills = set(self.catalog.target_items(target.themes, "skills"))
        desired_agents = set(self.catalog.target_items(target.themes, "agents"))
        managed_plugins = set(self.catalog.plugins)
        warnings: list[str] = []
        resolved_agent_scope = self.agent_scope(cwd, agent_scope)
        agent_root = self.agent_output_dir(cwd, resolved_agent_scope)

        with self._new_progress() as progress:
            task_id = progress.add_task("Verifying applied target", total=3)

            progress.update(task_id, description="Verifying plugins")
            try:
                installed_plugins = set(self.list_installed_plugins())
            except (CommandError, RuntimeError) as exc:
                warnings.append(f"verification: unable to list installed plugins ({exc})")
            else:
                warnings.extend(self._grouped_verification_messages("verification: missing plugin", "verification: missing plugins", desired_plugins - installed_plugins))
                unexpected_plugins = (installed_plugins - desired_plugins) if exclusive_plugins else ((installed_plugins & managed_plugins) - desired_plugins)
                warnings.extend(
                    self._grouped_verification_messages(
                        "verification: unexpected plugin still installed",
                        "verification: unexpected plugins still installed",
                        unexpected_plugins,
                    )
                )
            progress.advance(task_id)

            progress.update(task_id, description="Verifying skills")
            missing_skills = {provider_name for provider_name in desired_skills if not self._provider_outputs_present("skill", provider_name, self.paths.skills_dir)}
            warnings.extend(
                self._grouped_verification_messages(
                    "verification: missing synced skill content for",
                    "verification: missing synced skill content for",
                    missing_skills,
                )
            )
            stale_skills = {
                provider_name
                for provider_name in set(self.catalog.skill_providers) - desired_skills
                if self._prefixed_content_exists(self.paths.skills_dir, self.catalog.skill_providers[provider_name].prefix)
            }
            warnings.extend(
                self._grouped_verification_messages(
                    "verification: stale skill content still present for",
                    "verification: stale skill content still present for",
                    stale_skills,
                )
            )
            progress.advance(task_id)

            progress.update(task_id, description="Verifying agents")
            missing_agents = {
                provider_name
                for provider_name in desired_agents
                if not self._provider_outputs_present(
                    "agent",
                    provider_name,
                    agent_root,
                    scope=resolved_agent_scope,
                    cwd=cwd if resolved_agent_scope == "local" else None,
                )
            }
            warnings.extend(
                self._grouped_verification_messages(
                    "verification: missing synced agent content for",
                    "verification: missing synced agent content for",
                    missing_agents,
                )
            )
            if resolved_agent_scope == "local":
                stale_agents = {
                    provider_name
                    for provider_name in set(self.catalog.agent_providers) - desired_agents
                    if self._provider_outputs_present(
                        "agent",
                        provider_name,
                        agent_root,
                        scope=resolved_agent_scope,
                        cwd=cwd,
                    )
                }
            else:
                stale_agents = {
                    provider_name
                    for provider_name in set(self.catalog.agent_providers) - desired_agents
                    if self._prefixed_content_exists(agent_root, self.catalog.agent_providers[provider_name].prefix)
                }
            warnings.extend(
                self._grouped_verification_messages(
                    "verification: stale agent content still present for",
                    "verification: stale agent content still present for",
                    stale_agents,
                )
            )
            progress.advance(task_id)

        return warnings

    def _grouped_verification_messages(self, singular_prefix: str, plural_prefix: str, names: set[str]) -> list[str]:
        if ordered := sorted(names):
            return [f"{singular_prefix} {ordered[0]}"] if len(ordered) == 1 else [f"{plural_prefix} {', '.join(ordered)}"]
        else:
            return []

    def switch_target(
        self,
        target_name: str,
        cwd: Path,
        exclusive_plugins: bool = False,
        agent_scope: Literal["global", "local"] | None = None,
    ) -> ActivationTarget:
        self._reset_sync_warnings()
        target = self.catalog.resolve_target(target_name)
        desired_skills = self.catalog.target_items(target.themes, "skills")
        desired_agents = self.catalog.target_items(target.themes, "agents")
        resolved_agent_scope = self.agent_scope(cwd, agent_scope)
        agent_root = self.agent_output_dir(cwd, resolved_agent_scope)
        self._remove_named_items(self.paths.skills_dir, LEGACY_SKILLS)
        self._remove_named_items(agent_root, LEGACY_AGENTS)
        installed = self.list_installed_plugins()
        actions = self.plugin_actions_for_switch(target.name, installed, exclusive=exclusive_plugins)
        self._remove_unselected_skill_providers(desired_skills)
        self._remove_unselected_agent_providers(desired_agents, cwd, scope=resolved_agent_scope)
        self._execute_actions(
            actions,
            cwd=cwd,
            description="Reconciling plugins",
            parallel_workers=self._parallel_workers(len(actions)),
        )
        self._sync_missing_skill_providers(desired_skills, cwd)
        self._sync_missing_agent_providers(desired_agents, cwd, scope=resolved_agent_scope)
        self._remember_sync_warnings(
            self._collect_target_verification_warnings(
                target,
                cwd=cwd,
                exclusive_plugins=exclusive_plugins,
                agent_scope=resolved_agent_scope,
            )
        )
        self.state_store.write_repo_target(
            cwd,
            target,
            self.repo_profile_hint(cwd) or None,
            verification_warnings=self.sync_warnings,
        )
        return target

    def repo_update(self, cwd: Path, remote: bool = True) -> dict[str, str | None]:
        self.runner.require("git")
        revisions: dict[str, str | None] = {}
        project_root = find_project_root(cwd)
        uses_submodules = project_root is not None and (project_root / ".gitmodules").exists()
        total_steps = len(self.catalog.repositories) + (1 if uses_submodules else 0)

        with self._new_progress() as progress:
            task_id = progress.add_task("Refreshing source repositories", total=total_steps)

            if uses_submodules and project_root is not None:
                args = ["git", "submodule", "update", "--init", "--recursive"]
                if remote:
                    args.insert(4, "--remote")
                progress.update(task_id, description="Refreshing git submodules")
                self.runner.run(args, cwd=project_root)
                progress.advance(task_id)

                for name, source in self.catalog.repositories.items():
                    checkout = project_root / source.submodule_path
                    if checkout.exists():
                        observed = self.current_source_state(checkout)
                        revisions[name] = observed.revision
                        self.state_store.mark_source_revision(
                            name,
                            observed.revision,
                            manifest_version=observed.manifest_version,
                            source_path=observed.source_path,
                        )

            future_map: dict[Future[tuple[str, SourceState | None]], str] = {}
            with ThreadPoolExecutor(max_workers=self._parallel_workers(len(self.catalog.repositories))) as executor:
                for name, source in self.catalog.repositories.items():
                    future = executor.submit(self._refresh_source_cache, name, source, project_root)
                    future_map[future] = name
                for future in as_completed(future_map):
                    name, observed = future.result()
                    progress.update(task_id, description=f"Refreshing source {name}")
                    if observed is not None:
                        revisions.setdefault(name, observed.revision)
                        self.state_store.mark_source_revision(
                            name,
                            observed.revision,
                            manifest_version=observed.manifest_version,
                            source_path=observed.source_path,
                        )
                    progress.advance(task_id)
        return revisions

    def _refresh_source_cache(self, name: str, source: RepositorySource, project_root: Path | None) -> tuple[str, SourceState | None]:
        submodule_path = source.submodule_path
        cache_dir = self.paths.sources_dir / name
        submodule_checkout = project_root / submodule_path if project_root is not None else None
        use_cache = submodule_checkout is None or not submodule_checkout.exists()
        if cache_dir.exists():
            self.runner.run(["git", "pull", "--ff-only"], cwd=cache_dir)
        elif use_cache:
            self._clone_source_checkout(name)
        if use_cache and cache_dir.exists():
            return name, self.current_source_state(cache_dir)
        return name, None

    def current_revision(self, checkout: Path) -> str | None:
        try:
            result = self.runner.run(["git", "rev-parse", "HEAD"], cwd=checkout)
        except CommandError:
            return None
        return result.stdout.strip() or None

    def probe_manifest_version(self, checkout: Path) -> str | None:
        version, _ = self._probe_manifest_state(checkout)
        return version

    def _probe_manifest_state(self, checkout: Path) -> tuple[str | None, str | None]:
        probes = (
            ("package.json", self._version_from_package_json),
            ("pyproject.toml", self._version_from_pyproject_toml),
            ("Cargo.toml", self._version_from_package_table),
            ("composer.json", self._version_from_package_json),
        )
        for manifest_name, loader in probes:
            manifest = checkout / manifest_name
            if not manifest.exists():
                continue
            if version := loader(manifest):
                return version, manifest_name
        return None, None

    def current_source_state(self, checkout: Path) -> SourceState:
        manifest_version, source_path = self._probe_manifest_state(checkout)
        return SourceState(
            revision=self.current_revision(checkout),
            manifest_version=manifest_version,
            source_path=source_path or ".",
        )

    def source_has_changed(self, source_name: str, checkout: Path) -> bool:
        return self.state_store.source_has_changed(source_name, self.current_source_state(checkout))

    def _version_from_package_json(self, manifest: Path) -> str | None:
        try:
            data = json.loads(manifest.read_text())
        except (OSError, json.JSONDecodeError):
            return None
        version = data.get("version")
        return version if isinstance(version, str) and version.strip() else None

    def _version_from_pyproject_toml(self, manifest: Path) -> str | None:
        try:
            data = tomllib.loads(manifest.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            return None
        project = data.get("project")
        if not isinstance(project, dict):
            return None
        version = project.get("version")
        return version if isinstance(version, str) and version.strip() else None

    def _version_from_package_table(self, manifest: Path) -> str | None:
        try:
            data = tomllib.loads(manifest.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            return None
        package = data.get("package")
        if not isinstance(package, dict):
            return None
        version = package.get("version")
        return version if isinstance(version, str) and version.strip() else None

    def self_update(self, cwd: Path) -> dict[str, str | None]:
        project_root = find_project_root(cwd)
        if project_root is None or not (project_root / ".git").exists():
            raise RuntimeError("Self-update requires running from a git checkout. Upgrade the package with uv/pip/pipx, then run repo-update.")
        self.runner.require("git")
        self.runner.run(["git", "pull", "--ff-only"], cwd=project_root)
        return self.repo_update(project_root, remote=True)

    def installed_plugins_details(self) -> list[dict[str, str | None]]:
        try:
            self.runner.require("copilot")
            result = self.runner.run(["copilot", "plugin", "list"])
        except RuntimeError:
            return []
        return [{"name": plugin.name, "source": plugin.source, "version": plugin.version} for plugin in parse_installed_plugins(result.stdout)]

    def status_snapshot(self, cwd: Path) -> dict[str, object]:
        repo_hint = self.repo_profile_hint(cwd)
        repo_hint_target = self.catalog.resolve_target(repo_hint) if repo_hint in {*self.catalog.profiles, *self.catalog.themes} else None
        repo_state = self.state_store.read_repo_state(cwd)
        repo_profile_file = next((str(candidate) for candidate in (self.repo_profile_path(cwd, "root"), self.repo_profile_path(cwd, "github")) if candidate.exists()), "")
        repo_config_path = self.repo_config_path(cwd)
        repo_config = self.read_repo_config(cwd)
        resolved_agent_scope = self.agent_scope(cwd)
        resolved_mcp_scope = self.mcp_scope(cwd)
        agent_root = self.agent_output_dir(cwd, resolved_agent_scope)
        active_target_name = self.read_active_target(cwd)
        active_target = self.catalog.resolve_target(active_target_name) if active_target_name in {*self.catalog.profiles, *self.catalog.themes} else None
        skill_count = len([item for item in self.paths.skills_dir.iterdir() if item.is_dir()]) if self.paths.skills_dir.exists() else 0
        agent_count = len(list(agent_root.rglob("*.md"))) if agent_root.exists() else 0
        sync_warnings: list[str] = list(repo_state.verification_warnings) if repo_state is not None else []
        source_revisions: list[dict[str, str | int | None]] = []
        for name in self.catalog.repositories:
            stored = self.state_store.read_source_state(name)
            snapshot = self.catalog.source_entrypoint_summary(name)
            source_revisions.append(
                {
                    "name": name,
                    "revision": (stored.revision if stored is not None else None) or snapshot["revision"],
                    "commit_date": snapshot["commit_date"],
                    "measured_at": (stored.measured_at if stored is not None else None) or snapshot["measured_at"],
                    "file_count": snapshot["file_count"],
                    "provider_count": snapshot["provider_count"],
                }
            )
        for provider_name in self.catalog.skill_providers:
            stored_provider = self.state_store.read_provider_state("skill", provider_name)
            if stored_provider is None:
                continue
            sync_warnings.extend(stored_provider.warnings)
        for provider_name in self.catalog.agent_providers:
            stored_provider = self.state_store.read_provider_state(
                "agent",
                provider_name,
                scope=resolved_agent_scope,
                cwd=cwd if resolved_agent_scope == "local" else None,
            )
            if stored_provider is None:
                continue
            sync_warnings.extend(stored_provider.warnings)
        return {
            "repo_hint": repo_hint,
            "repo_hint_kind": repo_hint_target.kind if repo_hint_target is not None else "",
            "repo_hint_themes": repo_hint_target.themes if repo_hint_target is not None else [],
            "repo_profile_file": repo_profile_file,
            "repo_config_file": str(repo_config_path) if repo_config_path.exists() else "",
            "repo_config": repo_config.model_dump(mode="json"),
            "agent_scope": resolved_agent_scope,
            "agent_root": str(agent_root),
            "mcp_scope": resolved_mcp_scope,
            "mcp_profile": repo_config.mcps.profile,
            "active_target": active_target,
            "installed_plugins": self.installed_plugins_details(),
            "skill_count": skill_count,
            "agent_count": agent_count,
            "sync_warnings": list(dict.fromkeys(sync_warnings)),
            "last_verified_at": repo_state.last_verified_at if repo_state is not None else None,
            "source_revisions": source_revisions,
        }

    # ─── MCP management ──────────────────────────────────────────────────────

    def _mcp_config_path(self) -> Path:
        if self.paths.mcp_config_file is not None:
            return self.paths.mcp_config_file
        return self.paths.copilot_home / "mcp-config.json"

    def read_mcp_config(self) -> dict[str, object]:
        """Read ~/.copilot/mcp-config.json, returning an empty dict on missing/invalid file."""
        config_path = self._mcp_config_path()
        if not config_path.exists():
            return {}
        try:
            data = json.loads(config_path.read_text())
        except (OSError, json.JSONDecodeError):
            return {}
        return data if isinstance(data, dict) else {}

    def write_mcp_config(self, config: dict[str, object]) -> None:
        """Write the MCP config dict to ~/.copilot/mcp-config.json."""
        config_path = self._mcp_config_path()
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2) + "\n")

    def _servers_from_config(self, config: dict[str, object]) -> dict[str, object]:
        servers = config.get("servers")
        return cast(dict[str, object], servers) if isinstance(servers, dict) else {}

    def _local_mcp_config_path(self, cwd: Path) -> Path:
        return cwd / ".vscode" / "mcp.json"

    def read_local_mcp_config(self, cwd: Path) -> dict[str, object]:
        """Read .vscode/mcp.json from *cwd*, returning an empty dict on missing/invalid file."""
        config_path = self._local_mcp_config_path(cwd)
        if not config_path.exists():
            return {}
        try:
            data = json.loads(config_path.read_text())
        except (OSError, json.JSONDecodeError):
            return {}
        return cast(dict[str, object], data) if isinstance(data, dict) else {}

    def write_local_mcp_config(self, cwd: Path, config: dict[str, object]) -> None:
        """Write *config* to .vscode/mcp.json inside *cwd*."""
        config_path = self._local_mcp_config_path(cwd)
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2) + "\n")

    def discover_local_mcps(self, cwd: Path) -> dict[str, object]:
        """Discover MCP server definitions from .vscode/mcp.json in the repo."""
        candidates = [
            self._local_mcp_config_path(cwd),
            cwd / ".vscode" / "mcp" / "mcp.json",
        ]
        for candidate in candidates:
            if not candidate.exists():
                continue
            try:
                data = json.loads(candidate.read_text())
            except (OSError, json.JSONDecodeError):
                continue
            if not isinstance(data, dict):
                continue
            servers = data.get("servers", data.get("mcpServers", {}))
            if isinstance(servers, dict):
                return cast(dict[str, object], servers)
        return {}

    def probe_mcp_npm_version(self, package: str) -> str | None:
        """Probe npm registry for the latest version tag of an npm package.

        Returns the version string (e.g. ``"1.2.3"``) or ``None`` when npm is
        unavailable or the package cannot be found.
        """
        if self.runner.which("npm") is None:
            return None
        try:
            result = self.runner.run(["npm", "view", package, "version"])
            version = result.stdout.strip()
            return version or None
        except (CommandError, RuntimeError):
            return None

    def probe_mcp_pip_version(self, package: str) -> str | None:
        """Probe PyPI for the latest version of a pip package via ``pip index versions``.

        Returns the version string (e.g. ``"1.2.3"``) or ``None`` when pip is
        unavailable, the package is not found, or the output cannot be parsed.

        The ``pip index versions`` command outputs a line like::

            Available versions: 1.2.3, 1.2.2, ...

        The first entry on that line is the latest available version.
        """
        pip_cmd = self.runner.which("pip") or self.runner.which("pip3")
        if pip_cmd is None:
            return None
        try:
            result = self.runner.run([pip_cmd, "index", "versions", package])
            for line in result.stdout.splitlines():
                stripped = line.strip()
                # Match "Available versions: 1.2.3, 1.2.2, ..." (case-insensitive)
                if stripped.lower().startswith("available versions:"):
                    versions_part = stripped.split(":", 1)[1].strip()
                    if first := versions_part.split(",")[0].strip():
                        return first
        except (CommandError, RuntimeError):
            pass
        return None

    def _mcp_config_signature(self, entry: dict[str, object]) -> str:
        return hashlib.sha256(json.dumps(entry, sort_keys=True).encode()).hexdigest()

    def build_mcp_server_entry(self, name: str, record: McpRecord, installed_version: str | None = None) -> dict[str, object]:
        """Build the VS Code–compatible MCP server config entry for *record*.

        Raises ``ValueError`` when the record is invalid, e.g. an HTTP MCP
        without a ``url``.
        """
        if record.kind == "http":
            if not record.url:
                raise ValueError(f"MCP '{name}' has kind='http' but no url is set.")
            return {"type": "http", "url": record.url}
        if record.kind == "local":
            entry: dict[str, object] = {"type": "stdio"}
            if record.command:
                entry["command"] = record.command
            if record.args:
                entry["args"] = list(record.args)
            if record.env:
                entry["env"] = dict(record.env)
            return entry
        if record.kind == "pip":
            if record.args and (record.command is not None or record.args[0] == "--from"):
                pip_entry: dict[str, object] = {"type": "stdio", "command": record.command or "uvx", "args": list(record.args)}
                if record.env:
                    pip_entry["env"] = dict(record.env)
                return pip_entry
            # Python packages are run via uvx (uv's tool runner); version is
            # pinned with pip-style ``package==version`` syntax.
            package = record.package or name
            version_suffix = ""
            if record.pinned_tag:
                version_suffix = f"=={record.pinned_tag}"
            elif installed_version:
                version_suffix = f"=={installed_version}"
            versioned_package = f"{package}{version_suffix}"
            command = record.command or "uvx"
            pip_args: list[object] = [versioned_package, *record.args]
            pip_entry: dict[str, object] = {"type": "stdio", "command": command, "args": pip_args}
            if record.env:
                pip_entry["env"] = dict(record.env)
            return pip_entry
        # npm (and docker treated similarly via command override)
        package = record.package or name
        version_suffix = ""
        if record.pinned_tag:
            version_suffix = f"@{record.pinned_tag}"
        elif installed_version:
            version_suffix = f"@{installed_version}"
        versioned_package = f"{package}{version_suffix}"
        command = record.command or "npx"
        args: list[object] = ["-y", versioned_package, *record.args]
        npm_entry: dict[str, object] = {"type": "stdio", "command": command, "args": args}
        if record.env:
            npm_entry["env"] = dict(record.env)
        return npm_entry

    def sync_mcp(
        self,
        name: str,
        record: McpRecord,
        *,
        probe_version: bool = True,
        scope: Literal["global", "local"] | None = None,
        cwd: Path | None = None,
    ) -> McpSyncState:
        """Add or update a single MCP entry in the config and record its state.

        When the resolved *scope* is ``"local"`` the entry is written to
        ``.vscode/mcp.json`` inside *cwd* instead of the global config.
        *cwd* must be provided when the resolved scope is ``"local"``.
        """
        resolved_scope = self.mcp_scope(cwd or Path.cwd(), scope) if (scope is None and cwd is not None) else (scope or "global")
        installed_version: str | None = None
        installed_sha: str | None = None

        if record.kind == "npm" and not record.pinned_tag:
            if probe_version and record.package:
                installed_version = self.probe_mcp_npm_version(record.package)
            if installed_version is None and record.pinned_sha:
                installed_sha = record.pinned_sha
        elif record.kind == "pip" and not record.pinned_tag:
            if probe_version and record.package:
                installed_version = self.probe_mcp_pip_version(record.package)
            if installed_version is None and record.pinned_sha:
                installed_sha = record.pinned_sha

        entry = self.build_mcp_server_entry(name, record, installed_version)

        if resolved_scope == "local":
            if cwd is None:
                raise ValueError("cwd must be provided when scope='local'")
            local_config = self.read_local_mcp_config(cwd)
            local_servers = dict(self._servers_from_config(local_config))
            local_servers[name] = entry
            local_config["servers"] = local_servers
            self.write_local_mcp_config(cwd, local_config)
        else:
            global_config = self.read_mcp_config()
            global_servers = dict(self._servers_from_config(global_config))
            global_servers[name] = entry
            global_config["servers"] = global_servers
            self.write_mcp_config(global_config)

        mcp_state = McpSyncState(
            kind=record.kind,
            name=name,
            package=record.package,
            url=record.url,
            installed_version=record.pinned_tag or installed_version,
            installed_sha=installed_sha,
            config_signature=self._mcp_config_signature(entry),
            scope=resolved_scope,
            updated_at=datetime.now(UTC).isoformat(),
        )
        self.state_store.write_mcp_state(mcp_state)
        return mcp_state

    def remove_mcp(self, name: str, cwd: Path | None = None) -> bool:
        """Remove an MCP entry from the global config (and local config if *cwd* given).

        Returns True if the entry was present in either config.
        """
        removed = False
        # Remove from global config.
        global_config = self.read_mcp_config()
        global_servers = dict(self._servers_from_config(global_config))
        if name in global_servers:
            del global_servers[name]
            global_config["servers"] = global_servers
            self.write_mcp_config(global_config)
            removed = True
        # Remove from local config if cwd is given.
        if cwd is not None:
            local_config = self.read_local_mcp_config(cwd)
            local_servers = dict(self._servers_from_config(local_config))
            if name in local_servers:
                del local_servers[name]
                local_config["servers"] = local_servers
                self.write_local_mcp_config(cwd, local_config)
                removed = True
        self.state_store.clear_mcp_state(name)
        return removed

    def move_mcp_to_scope(
        self,
        name: str,
        target_scope: Literal["global", "local"],
        cwd: Path,
    ) -> McpSyncState:
        """Move an MCP server entry between global and local scope.

        - ``"local"``  — removes from ``~/.copilot/mcp-config.json``, writes to
          ``.vscode/mcp.json`` inside *cwd*.
        - ``"global"`` — removes from ``.vscode/mcp.json`` inside *cwd*, writes
          to ``~/.copilot/mcp-config.json``.

        Raises ``KeyError`` when the entry is not found in the source config.
        """
        global_config = self.read_mcp_config()
        global_servers = dict(self._servers_from_config(global_config))
        local_config = self.read_local_mcp_config(cwd)
        local_servers = dict(self._servers_from_config(local_config))

        if target_scope == "local":
            if name not in global_servers:
                raise KeyError(f"MCP '{name}' not found in global config (~/.copilot/mcp-config.json).")
            entry = global_servers.pop(name)
            global_config["servers"] = global_servers
            self.write_mcp_config(global_config)
            local_servers[name] = entry
            local_config["servers"] = local_servers
            self.write_local_mcp_config(cwd, local_config)
        else:
            if name not in local_servers:
                raise KeyError(f"MCP '{name}' not found in local config ({self._local_mcp_config_path(cwd)}).")
            entry = local_servers.pop(name)
            local_config["servers"] = local_servers
            self.write_local_mcp_config(cwd, local_config)
            global_servers[name] = entry
            global_config["servers"] = global_servers
            self.write_mcp_config(global_config)

        # Update or create state entry with the new scope.
        stored = self.state_store.read_mcp_state(name)
        if not isinstance(entry, dict):
            # The entry in the config was malformed; this should not happen with
            # valid VS Code MCP config files, but guard against it explicitly.
            raise ValueError(f"MCP entry '{name}' in config is not a JSON object; cannot compute signature.")
        entry_typed = cast(dict[str, object], entry)
        mcp_state = McpSyncState(
            kind=stored.kind if stored else "npm",
            name=name,
            package=stored.package if stored else None,
            url=stored.url if stored else None,
            installed_version=stored.installed_version if stored else None,
            installed_sha=stored.installed_sha if stored else None,
            config_signature=self._mcp_config_signature(entry_typed),
            scope=target_scope,
            updated_at=datetime.now(UTC).isoformat(),
        )
        self.state_store.write_mcp_state(mcp_state)
        return mcp_state

    def _mcp_entry_current(
        self,
        name: str,
        record: McpRecord,
        servers: dict[str, object],
        *,
        probe_version: bool = False,
        cwd: Path | None = None,
        scope: Literal["global", "local"] = "global",
    ) -> bool:
        """Return True when the existing config entry matches what we would write."""
        stored = self.state_store.read_mcp_state(name)
        if stored is None:
            return False
        if scope == "global" and stored.scope == "local":
            if cwd is not None:
                local_servers = dict(self._servers_from_config(self.read_local_mcp_config(cwd)))
                if name in local_servers:
                    return True
            else:
                return True
        if scope == "local" and stored.scope == "global":
            return False
        if name not in servers:
            return False
        if record.kind == "http":
            existing = servers[name]
            if not isinstance(existing, dict):
                return False
            existing_typed = cast(dict[str, object], existing)
            return existing_typed.get("url") == record.url
        if probe_version and not record.pinned_tag and record.package:
            latest: str | None = None
            if record.kind == "npm":
                latest = self.probe_mcp_npm_version(record.package)
            elif record.kind == "pip":
                latest = self.probe_mcp_pip_version(record.package)
            if latest is not None and latest != stored.installed_version:
                return False
        expected_entry = self.build_mcp_server_entry(name, record, stored.installed_version)
        return stored.config_signature == self._mcp_config_signature(expected_entry)

    def reconcile_mcps(
        self,
        cwd: Path,
        *,
        probe_version: bool = True,
        extra_servers: dict[str, object] | None = None,
        remove_unlisted: bool = False,
        scope: Literal["global", "local"] | None = None,
    ) -> dict[str, str]:
        """Sync catalog MCPs into the resolved target scope.

        Returns a mapping of ``name → action`` where action is one of
        ``"added"``, ``"updated"``, ``"skipped"``, or ``"removed"``.
        """
        results: dict[str, str] = {}
        resolved_scope = self.mcp_scope(cwd, scope)
        config = self.read_local_mcp_config(cwd) if resolved_scope == "local" else self.read_mcp_config()
        servers = dict(self._servers_from_config(config))

        local_servers = extra_servers if extra_servers is not None else (self.discover_local_mcps(cwd) if resolved_scope == "global" else {})
        desired_names: set[str] = set(self.catalog.mcps)

        with self._new_progress() as progress:
            total = len(self.catalog.mcps) + len(local_servers)
            task_id = progress.add_task("Syncing MCP servers", total=total)

            for mcp_name, record in self.catalog.mcps.items():
                progress.update(task_id, description=f"Syncing MCP {mcp_name}")
                if self._mcp_entry_current(
                    mcp_name,
                    record,
                    servers,
                    probe_version=probe_version,
                    cwd=cwd,
                    scope=resolved_scope,
                ):
                    results[mcp_name] = "skipped"
                else:
                    action = "updated" if mcp_name in servers else "added"
                    self.sync_mcp(mcp_name, record, probe_version=probe_version, scope=resolved_scope, cwd=cwd)
                    results[mcp_name] = action
                    refreshed = self.read_local_mcp_config(cwd) if resolved_scope == "local" else self.read_mcp_config()
                    servers = dict(self._servers_from_config(refreshed))
                progress.advance(task_id)

            if resolved_scope == "global":
                for local_name, local_entry in local_servers.items():
                    progress.update(task_id, description=f"Syncing local MCP {local_name}")
                    desired_names.add(local_name)
                    if local_name in self.catalog.mcps:
                        stored_for_local = self.state_store.read_mcp_state(local_name)
                        if stored_for_local is not None and stored_for_local.scope == "local":
                            results.setdefault(local_name, "skipped")
                            progress.advance(task_id)
                            continue
                    if not isinstance(local_entry, dict):
                        results[local_name] = "skipped"
                        progress.advance(task_id)
                        continue
                    typed_entry = cast(dict[str, object], local_entry)
                    safe_entry: dict[str, object] = {k: v for k, v in typed_entry.items() if k != "env"}
                    action = "updated" if local_name in servers else "added"
                    servers[local_name] = safe_entry
                    fresh_config = self.read_mcp_config()
                    fresh_config["servers"] = servers
                    self.write_mcp_config(fresh_config)
                    mcp_state = McpSyncState(
                        kind="local",
                        name=local_name,
                        config_signature=self._mcp_config_signature(safe_entry),
                        updated_at=datetime.now(UTC).isoformat(),
                    )
                    self.state_store.write_mcp_state(mcp_state)
                    results[local_name] = action
                    progress.advance(task_id)

        if remove_unlisted:
            config = self.read_local_mcp_config(cwd) if resolved_scope == "local" else self.read_mcp_config()
            servers = dict(self._servers_from_config(config))
            for existing_name in list(servers):
                if existing_name not in desired_names:
                    del servers[existing_name]
                    self.state_store.clear_mcp_state(existing_name)
                    results[existing_name] = "removed"
            config["servers"] = servers
            if resolved_scope == "local":
                self.write_local_mcp_config(cwd, config)
            else:
                self.write_mcp_config(config)

        return results

    def manage_mcps(
        self,
        operation: str,
        cwd: Path,
        *,
        scope: Literal["global", "local"] | None = None,
    ) -> dict[str, str]:
        """Top-level MCP management dispatch."""
        resolved_scope = self.mcp_scope(cwd, scope)
        if operation in {"install", "update"}:
            return self.reconcile_mcps(cwd, probe_version=True, scope=resolved_scope)
        if operation == "delete":
            results: dict[str, str] = {}
            for name in list(self.catalog.mcps):
                if resolved_scope == "local":
                    local_config = self.read_local_mcp_config(cwd)
                    local_servers = dict(self._servers_from_config(local_config))
                    removed = name in local_servers
                    if removed:
                        del local_servers[name]
                        local_config["servers"] = local_servers
                        self.write_local_mcp_config(cwd, local_config)
                        self.state_store.clear_mcp_state(name)
                else:
                    removed = self.remove_mcp(name, None)
                results[name] = "removed" if removed else "skipped"
            return results
        raise ValueError(f"Unknown MCP operation: {operation}")
