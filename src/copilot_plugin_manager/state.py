from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal

from .models import ActivationTarget, ManagerState, McpSyncState, ProviderSyncState, RepoState, SourceState
from .paths import ManagerPaths, find_project_root, repo_key


def provider_key(kind: Literal["skill", "agent"], provider_name: str) -> str:
    return f"{kind}:{provider_name}"


@dataclass
class StateStore:
    paths: ManagerPaths

    def _repo_state_path(self, cwd: Path) -> Path:
        return find_project_root(cwd) or cwd.resolve()

    def load(self) -> ManagerState:
        self.paths.ensure_directories()
        if not self.paths.state_file.exists():
            return ManagerState()
        return ManagerState.model_validate(json.loads(self.paths.state_file.read_text()))

    def save(self, state: ManagerState) -> None:
        self.paths.ensure_directories()
        self.paths.state_file.write_text(state.model_dump_json(indent=2) + "\n")

    def read_repo_state(self, cwd: Path) -> RepoState | None:
        state = self.load()
        return state.repositories.get(repo_key(self._repo_state_path(cwd)))

    def read_source_state(self, source_name: str) -> SourceState | None:
        state = self.load()
        return state.sources.get(source_name)

    def read_provider_state(self, kind: Literal["skill", "agent"], provider_name: str) -> ProviderSyncState | None:
        state = self.load()
        return state.providers.get(provider_key(kind, provider_name))

    def source_has_changed(self, source_name: str, observed: SourceState) -> bool:
        return observed.has_comparable_change(self.read_source_state(source_name))

    def write_repo_target(self, cwd: Path, target: ActivationTarget, repo_profile_hint: str | None) -> None:
        state = self.load()
        state.repositories[repo_key(self._repo_state_path(cwd))] = RepoState.from_target(target, repo_profile_hint)
        self.save(state)
        self.paths.legacy_active_target_file.parent.mkdir(parents=True, exist_ok=True)
        self.paths.legacy_active_target_file.write_text(target.name + "\n")

    def mark_source_revision(
        self,
        source_name: str,
        revision: str | None,
        manifest_version: str | None = None,
        source_path: str | None = None,
    ) -> None:
        state = self.load()
        source_state = state.sources.get(source_name, SourceState())
        observed_at = datetime.now(UTC).isoformat()
        source_state.revision = revision
        source_state.manifest_version = manifest_version
        source_state.source_path = source_path
        source_state.measured_at = observed_at
        source_state.last_seen_at = observed_at
        source_state.updated_at = observed_at
        state.sources[source_name] = source_state
        self.save(state)

    def write_provider_state(
        self,
        kind: Literal["skill", "agent"],
        provider_name: str,
        source_name: str,
        observed: SourceState,
        outputs: list[str],
        warnings: list[str],
        definition_signature: str,
    ) -> None:
        state = self.load()
        state.providers[provider_key(kind, provider_name)] = ProviderSyncState(
            kind=kind,
            source=source_name,
            revision=observed.revision,
            manifest_version=observed.manifest_version,
            source_path=observed.source_path,
            outputs=list(dict.fromkeys(outputs)),
            warnings=list(dict.fromkeys(warnings)),
            definition_signature=definition_signature,
            updated_at=datetime.now(UTC).isoformat(),
        )
        self.save(state)

    def clear_provider_state(self, kind: Literal["skill", "agent"], provider_name: str) -> None:
        state = self.load()
        if provider_key(kind, provider_name) not in state.providers:
            return
        del state.providers[provider_key(kind, provider_name)]
        self.save(state)

    def read_mcp_state(self, name: str) -> McpSyncState | None:
        state = self.load()
        return state.mcps.get(name)

    def write_mcp_state(self, mcp_state: McpSyncState) -> None:
        state = self.load()
        state.mcps[mcp_state.name] = mcp_state
        self.save(state)

    def clear_mcp_state(self, name: str) -> None:
        state = self.load()
        if name not in state.mcps:
            return
        del state.mcps[name]
        self.save(state)
