from __future__ import annotations

import hashlib
import json
import re
import shutil
import tomllib
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal, cast

from rich.progress import BarColumn, Progress, SpinnerColumn, TaskProgressColumn, TextColumn, TimeElapsedColumn

from .catalog import CatalogBundle
from .models import ActivationTarget, EntrypointRecord, McpRecord, McpSyncState, PlannedAction, SourceState
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

    def read_active_target(self, cwd: Path) -> str:
        repo_state = self.state_store.read_repo_state(cwd)
        if repo_state and repo_state.active_target:
            return repo_state.active_target
        if self.paths.legacy_active_target_file.exists():
            return self.paths.legacy_active_target_file.read_text().strip()
        return ""

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
            transient=False,
        )

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
        actions: list[PlannedAction] = []
        for plugin_name in desired:
            if plugin_name not in installed_set:
                actions.append(
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
                )
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
    ) -> None:
        actionable = [action for action in actions if action.command is not None]
        if not actionable:
            return
        with self._new_progress() as progress:
            task_id = progress.add_task(description, total=len(actionable))
            for action in actionable:
                command = action.command
                if command is None:
                    continue
                progress.update(task_id, description=action.description)
                self.runner.run(list(command), cwd=cwd)
                progress.advance(task_id)

    def manage_plugins(self, operation: str) -> None:
        self.runner.require("copilot")
        description = {
            "install": "Installing plugins",
            "update": "Updating plugins",
            "delete": "Removing plugins",
        }.get(operation, "Managing plugins")
        with self._new_progress() as progress:
            task_id = progress.add_task(description, total=len(self.catalog.plugins))
            for name in self.catalog.plugins:
                progress.update(task_id, description=f"{description[:-1]} {name}")
                match operation:
                    case "install":
                        self.runner.run(["copilot", "plugin", "install", self.catalog.plugin_install_source(name)])
                    case "update":
                        self.runner.run(["copilot", "plugin", "update", name])
                    case "delete":
                        self.runner.run(["copilot", "plugin", "uninstall", name], check=False)
                    case _:
                        raise ValueError(f"Unknown plugin operation: {operation}")
                progress.advance(task_id)

    def _resolve_source_checkout(self, source_name: str, cwd: Path) -> Path:
        source = self.catalog.repository_details(source_name)
        current = cwd.resolve()
        for parent in (current, *current.parents):
            candidate = parent / source.submodule_path
            if candidate.exists():
                return candidate
        cached = self.paths.sources_dir / source_name
        if cached.exists():
            return cached
        return self._clone_source_checkout(source_name)

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
    ) -> str:
        entrypoint = self._agent_entrypoint(provider_name, provider_source, source_path)
        if entrypoint is not None:
            return entrypoint.local_output
        flat = source_path.replace("/", "__").replace("\\", "__")
        if flat.endswith(".md"):
            flat = flat[:-3]
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
                {"source_path": entry.source_path, "local_output": entry.local_output}
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
    ) -> bool:
        stored = self.state_store.read_provider_state(kind, provider_name)
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
            )
            self._remember_sync_warnings(warnings)
            return
        self.state_store.clear_provider_state(kind, provider_name)

    def _clear_provider_states(self, kind: Literal["skill", "agent"], provider_names: list[str]) -> None:
        for provider_name in provider_names:
            self.state_store.clear_provider_state(kind, provider_name)

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
                warnings.append(self._sync_warning(provider_name, root, "dangling symlink"))
                continue
            if not source.exists():
                warnings.append(self._sync_warning(provider_name, root, "missing source root"))
                continue
            if source.is_file():
                destination = self.paths.skills_dir / f"{provider.prefix}__{source.stem}"
                self._copy_fs_path(source, destination)
                outputs.append(destination.name)
                continue
            for directory in sorted(item for item in source.iterdir() if item.is_dir()):
                dangling = [
                    self._sync_warning(
                        provider_name,
                        str(path.relative_to(source_root)).replace("\\", "/"),
                        "dangling symlink",
                    )
                    for path in self._dangling_symlinks(directory)
                ]
                destination = self.paths.skills_dir / f"{provider.prefix}__{directory.name}"
                self._copy_fs_path(directory, destination)
                outputs.append(destination.name)
                warnings.extend(dangling)
        synced_outputs = list(dict.fromkeys(outputs))
        self._record_provider_sync("skill", provider_name, provider.source, observed, synced_outputs, list(dict.fromkeys(warnings)))
        return synced_outputs

    def sync_agent_provider(
        self,
        provider_name: str,
        cwd: Path,
        *,
        claimed_source_paths: set[tuple[str, str]] | None = None,
        source_root: Path | None = None,
        observed: SourceState | None = None,
    ) -> list[str]:
        provider = self.catalog.agent_providers[provider_name]
        source_root = source_root or self._resolve_source_checkout(provider.source, cwd)
        observed = observed or self.current_source_state(source_root)
        self.paths.agents_dir.mkdir(parents=True, exist_ok=True)
        outputs: list[str] = []
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
                destination = self.paths.agents_dir / self._agent_output_name(
                    provider_name,
                    provider.prefix,
                    provider.source,
                    source_path,
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
                destination = self.paths.agents_dir / self._agent_output_name(
                    provider_name,
                    provider.prefix,
                    provider.source,
                    source_path,
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
        self._record_provider_sync("agent", provider_name, provider.source, observed, synced_outputs, [])
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
    ) -> None:
        ordered = self.catalog.preferred_provider_order("agent", provider_names)
        if not ordered:
            return

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
                    self.paths.agents_dir,
                    provider.prefix,
                ):
                    self._claim_agent_source_paths(provider_name, claimed_source_paths)
                    progress.advance(task_id)
                    continue
                self._remove_by_prefix(self.paths.agents_dir, [provider.prefix])
                self.sync_agent_provider(
                    provider_name,
                    cwd,
                    claimed_source_paths=claimed_source_paths,
                    source_root=source_root,
                    observed=observed,
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

    def manage_agents(self, operation: str, cwd: Path) -> None:
        if operation in {"install", "update"}:
            self._sync_agent_providers(
                list(self.catalog.agent_providers),
                cwd,
                task_title="Syncing agent providers",
                item_label="Syncing agent provider",
            )
            return
        if operation == "delete":
            provider_names = list(self.catalog.agent_providers)
            self._remove_by_prefix(self.paths.agents_dir, [provider.prefix for provider in self.catalog.agent_providers.values()])
            self._clear_provider_states("agent", provider_names)
            return
        raise ValueError(f"Unknown agent operation: {operation}")

    def manage_target(self, operation: str, target: str, cwd: Path) -> None:
        self._reset_sync_warnings()
        match target:
            case "all":
                self.manage_plugins(operation)
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd)
                self.manage_mcps(operation, cwd)
            case "plugins":
                self.manage_plugins(operation)
            case "skills":
                self.manage_skills(operation, cwd)
            case "agents":
                self.manage_agents(operation, cwd)
            case "mcps":
                self.manage_mcps(operation, cwd)
            case "thirdparty":
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd)
            case _:
                raise ValueError(f"Unknown target: {target}")
        if operation == "delete" and self.paths.legacy_active_target_file.exists():
            self.paths.legacy_active_target_file.write_text("")

    def _remove_unselected_skill_providers(self, desired: list[str]) -> None:
        removed = [name for name in self.catalog.skill_providers if name not in set(desired)]
        self._remove_by_prefix(self.paths.skills_dir, [self.catalog.skill_providers[name].prefix for name in removed])
        self._clear_provider_states("skill", removed)

    def _remove_unselected_agent_providers(self, desired: list[str]) -> None:
        removed = [name for name in self.catalog.agent_providers if name not in set(desired)]
        self._remove_by_prefix(self.paths.agents_dir, [self.catalog.agent_providers[name].prefix for name in removed])
        self._clear_provider_states("agent", removed)

    def _sync_missing_skill_providers(self, desired: list[str], cwd: Path) -> None:
        self._sync_skill_providers(
            desired,
            cwd,
            task_title="Downloading skill providers",
            item_label="Downloading skill provider",
        )

    def _sync_missing_agent_providers(self, desired: list[str], cwd: Path) -> None:
        if not desired:
            return
        self._sync_agent_providers(
            desired,
            cwd,
            task_title="Downloading agent providers",
            item_label="Downloading agent provider",
        )

    def switch_target(self, target_name: str, cwd: Path, exclusive_plugins: bool = False) -> ActivationTarget:
        self._reset_sync_warnings()
        old_target_name = self.read_active_target(cwd)
        target = self.catalog.resolve_target(target_name)
        if old_target_name == target.name:
            return target
        self._remove_named_items(self.paths.skills_dir, LEGACY_SKILLS)
        self._remove_named_items(self.paths.agents_dir, LEGACY_AGENTS)
        installed = self.list_installed_plugins()
        actions = self.plugin_actions_for_switch(target.name, installed, exclusive=exclusive_plugins)
        self._remove_unselected_skill_providers(self.catalog.target_items(target.themes, "skills"))
        self._remove_unselected_agent_providers(self.catalog.target_items(target.themes, "agents"))
        self._execute_actions(actions, cwd=cwd, description="Reconciling plugins")
        self._sync_missing_skill_providers(self.catalog.target_items(target.themes, "skills"), cwd)
        self._sync_missing_agent_providers(self.catalog.target_items(target.themes, "agents"), cwd)
        self.state_store.write_repo_target(cwd, target, self.repo_profile_hint(cwd) or None)
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

            for name, source in self.catalog.repositories.items():
                progress.update(task_id, description=f"Refreshing source {name}")
                cache_dir = self.paths.sources_dir / name
                submodule_checkout = project_root / source.submodule_path if project_root is not None else None
                use_cache = submodule_checkout is None or not submodule_checkout.exists()
                if cache_dir.exists():
                    self.runner.run(["git", "pull", "--ff-only"], cwd=cache_dir)
                elif use_cache:
                    self._clone_source_checkout(name)
                if use_cache and cache_dir.exists():
                    observed = self.current_source_state(cache_dir)
                    revisions.setdefault(name, observed.revision)
                    self.state_store.mark_source_revision(
                        name,
                        observed.revision,
                        manifest_version=observed.manifest_version,
                        source_path=observed.source_path,
                    )
                progress.advance(task_id)
        return revisions

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
            version = loader(manifest)
            if version:
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
        repo_profile_file = next((str(candidate) for candidate in (self.repo_profile_path(cwd, "root"), self.repo_profile_path(cwd, "github")) if candidate.exists()), "")
        active_target_name = self.read_active_target(cwd)
        active_target = self.catalog.resolve_target(active_target_name) if active_target_name in {*self.catalog.profiles, *self.catalog.themes} else None
        skill_count = len([item for item in self.paths.skills_dir.iterdir() if item.is_dir()]) if self.paths.skills_dir.exists() else 0
        agent_count = len(list(self.paths.agents_dir.rglob("*.md"))) if self.paths.agents_dir.exists() else 0
        sync_warnings: list[str] = []
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
        return {
            "repo_hint": repo_hint,
            "repo_profile_file": repo_profile_file,
            "active_target": active_target,
            "installed_plugins": self.installed_plugins_details(),
            "skill_count": skill_count,
            "agent_count": agent_count,
            "sync_warnings": list(dict.fromkeys(sync_warnings)),
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
        if not isinstance(data, dict):
            return {}
        return data

    def write_mcp_config(self, config: dict[str, object]) -> None:
        """Write the MCP config dict to ~/.copilot/mcp-config.json."""
        config_path = self._mcp_config_path()
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2) + "\n")

    def _servers_from_config(self, config: dict[str, object]) -> dict[str, object]:
        servers = config.get("servers")
        if not isinstance(servers, dict):
            return {}
        return cast(dict[str, object], servers)

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
        if not isinstance(data, dict):
            return {}
        return cast(dict[str, object], data)

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
            return version if version else None
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
                    first = versions_part.split(",")[0].strip()
                    if first:
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
        scope: Literal["global", "local"] = "global",
        cwd: Path | None = None,
    ) -> McpSyncState:
        """Add or update a single MCP entry in the config and record its state.

        When *scope* is ``"local"`` the entry is written to
        ``.vscode/mcp.json`` inside *cwd* instead of the global config.
        *cwd* must be provided when *scope* is ``"local"``.
        """
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

        if scope == "local":
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
            scope=scope,
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
    ) -> bool:
        """Return True when the existing config entry matches what we would write.

        When *probe_version* is True and the entry is an unversioned npm/pip
        package, the registry is probed for the latest version; if that differs
        from the stored version the entry is treated as stale and False is
        returned so the caller will update it.

        When *cwd* is provided, entries that were moved to local scope are
        validated against the local ``.vscode/mcp.json``; if the entry is no
        longer present there it is treated as needing a global add.
        """
        stored = self.state_store.read_mcp_state(name)
        if stored is None:
            return False
        # If the entry has been deliberately moved to local scope, skip it in
        # global reconciliation – but only if the local config still contains it.
        if stored.scope == "local":
            if cwd is not None:
                local_servers = dict(self._servers_from_config(self.read_local_mcp_config(cwd)))
                if name in local_servers:
                    return True
                # The local config no longer has this entry; fall through so
                # reconcile_mcps re-adds it to the global config.
            else:
                return True
        if name not in servers:
            return False
        # HTTP MCPs are current as long as the URL hasn't changed.
        if record.kind == "http":
            existing = servers[name]
            if not isinstance(existing, dict):
                return False
            existing_typed = cast(dict[str, object], existing)
            return existing_typed.get("url") == record.url
        # For npm/pip: when probing is enabled and the package is not pinned,
        # compare the stored version against the live registry version.  If
        # a newer version is available, treat the entry as stale.
        if probe_version and not record.pinned_tag and record.package:
            latest: str | None = None
            if record.kind == "npm":
                latest = self.probe_mcp_npm_version(record.package)
            elif record.kind == "pip":
                latest = self.probe_mcp_pip_version(record.package)
            if latest is not None and latest != stored.installed_version:
                return False
        # For npm/pip/local: compare config signatures.
        expected_entry = self.build_mcp_server_entry(name, record, stored.installed_version)
        return stored.config_signature == self._mcp_config_signature(expected_entry)

    def reconcile_mcps(
        self,
        cwd: Path,
        *,
        probe_version: bool = True,
        extra_servers: dict[str, object] | None = None,
        remove_unlisted: bool = False,
    ) -> dict[str, str]:
        """Sync all catalog MCPs (plus any local ones from .vscode/mcp.json).

        Returns a mapping of ``name → action`` where action is one of
        ``"added"``, ``"updated"``, ``"skipped"``, or ``"removed"``.
        """
        results: dict[str, str] = {}
        config = self.read_mcp_config()
        servers = dict(self._servers_from_config(config))

        # Merge in any local MCP definitions from the repo's .vscode/mcp.json.
        local_servers = extra_servers if extra_servers is not None else self.discover_local_mcps(cwd)

        desired_names: set[str] = set(self.catalog.mcps)

        with self._new_progress() as progress:
            total = len(self.catalog.mcps) + len(local_servers)
            task_id = progress.add_task("Syncing MCP servers", total=total)

            for mcp_name, record in self.catalog.mcps.items():
                progress.update(task_id, description=f"Syncing MCP {mcp_name}")
                if self._mcp_entry_current(mcp_name, record, servers, probe_version=probe_version, cwd=cwd):
                    results[mcp_name] = "skipped"
                else:
                    action = "updated" if mcp_name in servers else "added"
                    self.sync_mcp(mcp_name, record, probe_version=probe_version)
                    results[mcp_name] = action
                    # Refresh servers after write.
                    servers = dict(self._servers_from_config(self.read_mcp_config()))
                progress.advance(task_id)

            for local_name, local_entry in local_servers.items():
                progress.update(task_id, description=f"Syncing local MCP {local_name}")
                desired_names.add(local_name)
                # If a catalog MCP has been deliberately moved to local scope,
                # it was already handled (skipped) in the catalog loop above.
                # Don't re-add it to the global config here.
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
                # Strip env vars before merging into the global (user-wide) config
                # to avoid persisting repo-specific secrets outside the repository.
                safe_entry: dict[str, object] = {k: v for k, v in typed_entry.items() if k != "env"}
                action = "updated" if local_name in servers else "added"
                servers[local_name] = safe_entry
                # Write the updated servers dict back to disk.
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
            config = self.read_mcp_config()
            servers = dict(self._servers_from_config(config))
            for existing_name in list(servers):
                if existing_name not in desired_names:
                    del servers[existing_name]
                    self.state_store.clear_mcp_state(existing_name)
                    results[existing_name] = "removed"
            config["servers"] = servers
            self.write_mcp_config(config)

        return results

    def manage_mcps(self, operation: str, cwd: Path) -> dict[str, str]:
        """Top-level MCP management dispatch.

        Supported operations: ``"install"`` / ``"update"`` (both call
        ``reconcile_mcps`` with version probing enabled), and ``"delete"``
        (removes all catalog MCPs from both global and local configs).
        """
        if operation in {"install", "update"}:
            # Both install and update probe the registry for newer versions so
            # that ``copilot-plugin-manager update mcps`` always refreshes to
            # the latest available version.
            return self.reconcile_mcps(cwd, probe_version=True)
        if operation == "delete":
            results: dict[str, str] = {}
            for name in list(self.catalog.mcps):
                removed = self.remove_mcp(name, cwd)
                results[name] = "removed" if removed else "skipped"
            return results
        raise ValueError(f"Unknown MCP operation: {operation}")
