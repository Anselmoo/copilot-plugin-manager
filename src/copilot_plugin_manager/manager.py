from __future__ import annotations

import json
import re
import shutil
import tomllib
from pathlib import Path

from rich.progress import BarColumn, Progress, SpinnerColumn, TaskProgressColumn, TextColumn, TimeElapsedColumn

from .catalog import CatalogBundle
from .models import ActivationTarget, EntrypointRecord, PlannedAction, SourceState
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

    def repo_profile_hint(self, cwd: Path) -> str:
        return find_repo_profile(cwd, self.paths.copilot_home)

    def read_active_target(self, cwd: Path) -> str:
        repo_state = self.state_store.read_repo_state(cwd)
        if repo_state and repo_state.active_target:
            return repo_state.active_target
        if self.paths.legacy_active_target_file.exists():
            return self.paths.legacy_active_target_file.read_text().strip()
        return ""

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
        if destination.exists():
            if destination.is_dir() and not destination.is_symlink():
                shutil.rmtree(destination)
            else:
                destination.unlink()
        if source.is_dir():
            shutil.copytree(source, destination)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)

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
                if child.name.startswith(f"{prefix}__") or child.name.startswith(f"{prefix}-"):
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
        return any(child.name.startswith(f"{prefix}__") or child.name.startswith(f"{prefix}-") for child in root.iterdir())

    def sync_skill_provider(self, provider_name: str, cwd: Path) -> None:
        provider = self.catalog.skill_providers[provider_name]
        source_root = self._resolve_source_checkout(provider.source, cwd)
        self.paths.skills_dir.mkdir(parents=True, exist_ok=True)
        for root in provider.roots:
            source = source_root / root
            if not source.exists():
                continue
            if source.is_file():
                destination = self.paths.skills_dir / f"{provider.prefix}__{source.stem}"
                self._copy_fs_path(source, destination)
                continue
            for directory in sorted(item for item in source.iterdir() if item.is_dir()):
                destination = self.paths.skills_dir / f"{provider.prefix}__{directory.name}"
                self._copy_fs_path(directory, destination)

    def sync_agent_provider(
        self,
        provider_name: str,
        cwd: Path,
        *,
        claimed_source_paths: set[tuple[str, str]] | None = None,
    ) -> None:
        provider = self.catalog.agent_providers[provider_name]
        source_root = self._resolve_source_checkout(provider.source, cwd)
        self.paths.agents_dir.mkdir(parents=True, exist_ok=True)
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
                progress.update(task_id, description=f"{item_label} {provider_name}")
                self.sync_agent_provider(provider_name, cwd, claimed_source_paths=claimed_source_paths)
                progress.advance(task_id)

    def manage_skills(self, operation: str, cwd: Path) -> None:
        prefixes = [provider.prefix for provider in self.catalog.skill_providers.values()]
        if operation in {"install", "update"}:
            self._remove_by_prefix(self.paths.skills_dir, prefixes)
            provider_names = list(self.catalog.skill_providers)
            with self._new_progress() as progress:
                task_id = progress.add_task("Syncing skill providers", total=len(provider_names))
                for provider_name in provider_names:
                    progress.update(task_id, description=f"Syncing skill provider {provider_name}")
                    self.sync_skill_provider(provider_name, cwd)
                    progress.advance(task_id)
            return
        if operation == "delete":
            self._remove_by_prefix(self.paths.skills_dir, prefixes)
            return
        raise ValueError(f"Unknown skill operation: {operation}")

    def manage_agents(self, operation: str, cwd: Path) -> None:
        prefixes = [provider.prefix for provider in self.catalog.agent_providers.values()]
        if operation in {"install", "update"}:
            self._remove_by_prefix(self.paths.agents_dir, prefixes)
            self._sync_agent_providers(
                list(self.catalog.agent_providers),
                cwd,
                task_title="Syncing agent providers",
                item_label="Syncing agent provider",
            )
            return
        if operation == "delete":
            self._remove_by_prefix(self.paths.agents_dir, prefixes)
            return
        raise ValueError(f"Unknown agent operation: {operation}")

    def manage_target(self, operation: str, target: str, cwd: Path) -> None:
        match target:
            case "all":
                self.manage_plugins(operation)
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd)
            case "plugins":
                self.manage_plugins(operation)
            case "skills":
                self.manage_skills(operation, cwd)
            case "agents":
                self.manage_agents(operation, cwd)
            case "thirdparty":
                self.manage_skills(operation, cwd)
                self.manage_agents(operation, cwd)
            case _:
                raise ValueError(f"Unknown target: {target}")
        if operation == "delete" and self.paths.legacy_active_target_file.exists():
            self.paths.legacy_active_target_file.write_text("")

    def _remove_unselected_skill_providers(self, desired: list[str]) -> None:
        prefixes = [self.catalog.skill_providers[name].prefix for name in self.catalog.skill_providers if name not in set(desired)]
        self._remove_by_prefix(self.paths.skills_dir, prefixes)

    def _remove_unselected_agent_providers(self, desired: list[str]) -> None:
        prefixes = [self.catalog.agent_providers[name].prefix for name in self.catalog.agent_providers if name not in set(desired)]
        self._remove_by_prefix(self.paths.agents_dir, prefixes)

    def _sync_missing_skill_providers(self, desired: list[str], cwd: Path) -> None:
        pending = [name for name in desired if not self._prefixed_content_exists(self.paths.skills_dir, self.catalog.skill_providers[name].prefix)]
        if not pending:
            return
        with self._new_progress() as progress:
            task_id = progress.add_task("Downloading skill providers", total=len(pending))
            for name in pending:
                progress.update(task_id, description=f"Downloading skill provider {name}")
                self.sync_skill_provider(name, cwd)
                progress.advance(task_id)

    def _sync_missing_agent_providers(self, desired: list[str], cwd: Path) -> None:
        if not desired:
            return
        self._remove_by_prefix(
            self.paths.agents_dir,
            [self.catalog.agent_providers[name].prefix for name in desired],
        )
        self._sync_agent_providers(
            desired,
            cwd,
            task_title="Downloading agent providers",
            item_label="Downloading agent provider",
        )

    def switch_target(self, target_name: str, cwd: Path, exclusive_plugins: bool = False) -> ActivationTarget:
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
        active_target_name = self.read_active_target(cwd)
        active_target = self.catalog.resolve_target(active_target_name) if active_target_name in {*self.catalog.profiles, *self.catalog.themes} else None
        skill_count = len([item for item in self.paths.skills_dir.iterdir() if item.is_dir()]) if self.paths.skills_dir.exists() else 0
        agent_count = len(list(self.paths.agents_dir.rglob("*.md"))) if self.paths.agents_dir.exists() else 0
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
        return {
            "repo_hint": repo_hint,
            "active_target": active_target,
            "installed_plugins": self.installed_plugins_details(),
            "skill_count": skill_count,
            "agent_count": agent_count,
            "source_revisions": source_revisions,
        }
