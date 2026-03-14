from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Literal

from pydantic import BaseModel, Field


class RepositorySource(BaseModel):
    url: str
    description: str
    use_when: str
    license: str
    version: str
    owner: str
    repo: str
    submodule_path: str
    source_url: str | None = None
    version_channel: str | None = None
    tags: list[str] = Field(default_factory=list)


class PluginRecord(BaseModel):
    install_source: str
    description: str | None = None
    use_when: str | None = None
    source_url: str | None = None
    version_channel: str | None = None
    tags: list[str] = Field(default_factory=list)


class ProviderRecord(BaseModel):
    source: str
    prefix: str
    roots: list[str] = Field(default_factory=list)
    description: str | None = None
    use_when: str | None = None
    homepage: str | None = None
    license: str | None = None
    version: str | None = None
    version_channel: str | None = None
    tags: list[str] = Field(default_factory=list)


class EntrypointRecord(BaseModel):
    kind: Literal["plugin", "skill", "agent"]
    source: str
    provider: str | None = None
    source_path: str
    local_name: str
    local_output: str
    title: str
    description: str
    source_url: str
    tags: list[str] = Field(default_factory=list)
    commit_revision: str | None = None
    commit_date: str | None = None
    approval_date: str | None = None
    measured_revision: str | None = None
    measured_at: str | None = None
    first_seen_at: str | None = None
    last_seen_at: str | None = None


class ThemeRecord(BaseModel):
    plugins: list[str] = Field(default_factory=list)
    skills: list[str] = Field(default_factory=list)
    agents: list[str] = Field(default_factory=list)


class ProfileRecord(BaseModel):
    themes: list[str] = Field(default_factory=list)


class ActivationTarget(BaseModel):
    name: str
    kind: Literal["profile", "theme"]
    themes: list[str]


class RepoState(BaseModel):
    active_target: str | None = None
    active_kind: Literal["profile", "theme"] | None = None
    active_themes: list[str] = Field(default_factory=list)
    repo_profile_hint: str | None = None
    updated_at: str | None = None

    @classmethod
    def from_target(cls, target: ActivationTarget, repo_profile_hint: str | None) -> "RepoState":
        return cls(
            active_target=target.name,
            active_kind=target.kind,
            active_themes=target.themes,
            repo_profile_hint=repo_profile_hint,
            updated_at=datetime.now(UTC).isoformat(),
        )


class SourceState(BaseModel):
    revision: str | None = None
    manifest_version: str | None = None
    source_path: str | None = None
    measured_at: str | None = None
    last_seen_at: str | None = None
    updated_at: str | None = None

    def has_comparable_change(self, previous: "SourceState | None") -> bool:
        if previous is None:
            return self.revision is not None or self.manifest_version is not None
        if self.revision and previous.revision:
            return self.revision != previous.revision
        if self.revision or previous.revision:
            return self.revision != previous.revision
        if self.manifest_version and previous.manifest_version:
            return self.manifest_version != previous.manifest_version
        return self.manifest_version != previous.manifest_version


class ProviderSyncState(BaseModel):
    kind: Literal["skill", "agent"]
    source: str
    revision: str | None = None
    manifest_version: str | None = None
    source_path: str | None = None
    outputs: list[str] = Field(default_factory=list)
    definition_signature: str | None = None
    updated_at: str | None = None


class ManagerState(BaseModel):
    repositories: dict[str, RepoState] = Field(default_factory=dict)
    sources: dict[str, SourceState] = Field(default_factory=dict)
    providers: dict[str, ProviderSyncState] = Field(default_factory=dict)


@dataclass(frozen=True)
class InstalledPlugin:
    name: str
    source: str
    version: str | None = None


@dataclass(frozen=True)
class PlannedAction:
    category: Literal["plugin", "skill", "agent", "repo", "state", "info"]
    description: str
    command: tuple[str, ...] | None = None
