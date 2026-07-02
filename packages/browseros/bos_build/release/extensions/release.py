#!/usr/bin/env python3
"""Ext release orchestration: build → stamp → pack → upload per spec, then
regenerate the update manifests through the Part B feeds publisher.

This is the consolidation the old actions repo could not do — it generated
update-manifest.xml locally and told the operator to upload it by hand. Here
the manifests flow through FeedPublisher's rails (dry-run by default,
--publish-manifest to write), so a released crx and its feed entry can no
longer drift apart.
"""

import os
import re
from pathlib import Path
from typing import List, Optional, Tuple

from ...core.step import Step, ValidationError
from ...lib.paths import get_package_root
from ...lib.r2 import BOTO3_AVAILABLE, get_r2_client, upload_file_to_r2
from ...lib.utils import log_info, log_success, log_warning
from ..feeds.spec import CDN_BASE_URL, EXTENSIONS as FEED_EXTENSIONS
from .crx import find_chrome_binary, pack_crx
from .manifests import CHANNELS, ExtensionsFeedModule
from .specs import (
    ExtensionSpec,
    ExternalRepoSource,
    InRepoSource,
    select_specs,
    spec_by_name,
)
from .workspace import (
    resolve_source,
    run_command,
    update_manifest_version,
    write_env_file,
)

# Chrome requires extension versions to be 1-4 dot-separated integers.
_VERSION_RE = re.compile(r"^\d+(\.\d+){0,3}$")


def _require_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    # <=1 mirrors the old release tool's guard: 0-1 chars is an unset or
    # placeholder value ("0", "-"), never a real key.
    if len(value) <= 1:
        raise EnvironmentError(f"Missing or empty environment variable: {name}")
    return value


class ExtensionReleaseModule(Step):
    """Build, version-stamp, pack and upload the selected extension CRXs."""

    produces = []
    requires = []
    description = "Build and upload extension CRXs to the CDN"

    def __init__(
        self,
        version: str,
        names: Tuple[str, ...],
        branch_override: Optional[str] = None,
        chrome_binary: Optional[str] = None,
        monorepo_root: Optional[Path] = None,
        work_root: Optional[Path] = None,
        r2_client=None,
    ):
        self.version = version
        self.names = names
        self.branch_override = branch_override
        self.chrome_binary = chrome_binary
        self._monorepo_root = monorepo_root
        self._work_root = work_root
        self._r2_client = r2_client

    def _specs(self) -> List[ExtensionSpec]:
        return [spec_by_name(name) for name in self.names]

    def validate(self, ctx) -> None:
        try:
            specs = self._specs()
        except ValueError as e:
            raise ValidationError(str(e))

        for spec in specs:
            try:
                _require_env(spec.signing_key_env)
            except EnvironmentError:
                raise ValidationError(
                    f"Signing key env var '{spec.signing_key_env}' for "
                    f"'{spec.name}' is missing or empty"
                )

        if not BOTO3_AVAILABLE:
            raise ValidationError("boto3 library not installed - run: pip install boto3")
        if not ctx.env.has_r2_config():
            raise ValidationError("R2 configuration not set")

        # Public repos clone fine without it — warn, don't fail.
        if not os.environ.get("GH_TOKEN") and any(
            isinstance(spec.source, ExternalRepoSource) for spec in specs
        ):
            log_warning(
                "GH_TOKEN is not set — cloning external extension repos "
                "will fail if any of them is private"
            )

        try:
            find_chrome_binary(self.chrome_binary)
        except RuntimeError as e:
            raise ValidationError(str(e))

    def execute(self, ctx) -> None:
        package_root = get_package_root()
        monorepo_root = self._monorepo_root or package_root.parent.parent
        work_root = self._work_root or package_root / "build" / "extensions"

        client = self._r2_client
        if client is None:
            client = get_r2_client(ctx.env)
            if client is None:
                raise RuntimeError("Failed to create R2 client")
        chrome = find_chrome_binary(self.chrome_binary)

        for spec in self._specs():
            log_info(f"\n=== Releasing extension: {spec.name} v{self.version} ===")
            source_root = resolve_source(
                spec,
                monorepo_root=monorepo_root,
                work_root=work_root,
                branch_override=self.branch_override,
            )
            update_manifest_version(
                source_root / spec.manifest_path, self.version
            )
            if isinstance(spec.source, InRepoSource):
                touched = spec.manifest_path
                if spec.env:
                    touched += f" and {spec.env_dir or '.'}/.env"
                log_warning(
                    f"stamped {touched} in the working tree — "
                    "revert them if this was a local test run"
                )
            if spec.env:
                env_dir = (
                    source_root / spec.env_dir if spec.env_dir else source_root
                )
                write_env_file(env_dir, spec.env)
            if spec.pre_build:
                run_command(spec.pre_build, source_root)
            run_command(spec.build, source_root)

            crx_path = pack_crx(
                source_root / spec.dist_path,
                _require_env(spec.signing_key_env),
                chrome,
                work_root / "dist" / spec.crx_filename(self.version),
            )

            r2_key = spec.crx_key(self.version)
            if not upload_file_to_r2(client, crx_path, r2_key, ctx.env.r2_bucket):
                raise RuntimeError(f"Upload failed for {r2_key}")
            log_success(f"CRX live at {CDN_BASE_URL}/{r2_key}")


def build_pipeline(
    version: str,
    name: Optional[str],
    channel: str,
    publish_manifest: bool,
    branch: Optional[str],
    chrome_binary: Optional[str],
) -> List[Step]:
    """Assemble the release pipeline for one --name (or every spec).

    Extensions the feeds table knows get their new version pinned into an
    ExtensionsFeedModule step; a selection with no feed members (controller)
    releases the crx only — there is nothing to regenerate.

    Channel and version are rejected here, at assembly time — the runner
    validates steps just-in-time, so leaving this to the feeds step would
    burn a full build + crx upload before a typo surfaces.
    """
    if channel not in CHANNELS:
        raise ValueError(
            f"channel must be one of {'/'.join(CHANNELS)}, got '{channel}'"
        )
    if not _VERSION_RE.fullmatch(version):
        raise ValueError(
            f"Invalid version '{version}' — Chrome extension versions are "
            "1-4 dot-separated integers (e.g. 0.0.118)"
        )
    # A '-'-prefixed value would be parsed as a git option, not a branch.
    if branch and branch.startswith("-"):
        raise ValueError(f"Invalid branch name '{branch}'")
    specs = select_specs(name)
    steps: List[Step] = [
        ExtensionReleaseModule(
            version=version,
            names=tuple(spec.name for spec in specs),
            branch_override=branch,
            chrome_binary=chrome_binary,
        )
    ]

    feed_names = {ext.name for ext in FEED_EXTENSIONS}
    pins = {spec.name: version for spec in specs if spec.name in feed_names}
    if pins:
        steps.append(
            ExtensionsFeedModule(
                channel=channel,
                set_versions=pins,
                publish=publish_manifest,
            )
        )
    return steps
