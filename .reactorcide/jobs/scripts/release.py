#!/usr/bin/env python3
"""Cross-build the csilctl CLI, semver-tag it, and publish a GitHub release.

Runs as the job_command for the csilctl-release job, in the working
directory runnerlib already checked the source out into.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Mapping, Sequence


PACKAGE = "csilctl"
RELEASE_TAG = re.compile(
    r"^csilctl/v(?P<version>"
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)\."
    r"(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)$"
)

# (platform label, Rust target triple, binary name)
BUILD_TARGETS = (
    ("linux-x86_64", "x86_64-unknown-linux-gnu.2.28", "csilctl"),
    ("linux-aarch64", "aarch64-unknown-linux-gnu.2.28", "csilctl"),
    ("darwin-aarch64", "aarch64-apple-darwin", "csilctl"),
    ("windows-x86_64", "x86_64-pc-windows-gnu", "csilctl.exe"),
)
ZIGBUILD_IMAGE = "ghcr.io/rust-cross/cargo-zigbuild:latest"


def _run(
    args: Sequence[str | Path],
    *,
    cwd: Path,
    env: Mapping[str, str] | None = None,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    command = tuple(str(arg) for arg in args)
    print(f"+ {' '.join(command)}", flush=True)
    command_env = os.environ.copy()
    if env:
        command_env.update(env)
    return subprocess.run(
        command,
        cwd=cwd,
        env=command_env,
        check=True,
        shell=False,
        text=True,
        capture_output=capture,
    )


BUILDCTL_VERSION = "0.17.3"
# sha256 of buildkit-v0.17.3.linux-{arch}.tar.gz, from each asset's
# .provenance.json subject digest (verified against the downloaded archives).
BUILDCTL_CHECKSUMS = {
    "amd64": "1ab54cc01fd2e174483070451921badc53cc463a4f2e2e980be7db99ca95c0d0",
    "arm64": "1afabc9c0829f7fa5173f439b7212194703b48f8e79a405596ada1af6e6f8220",
}
BUILDCTL_ARCHITECTURES = {
    "x86_64": "amd64",
    "aarch64": "arm64",
}


def _install_buildctl(root: Path) -> Path:
    architecture = BUILDCTL_ARCHITECTURES.get(platform.machine())
    if not architecture:
        raise RuntimeError(f"buildctl is not available for {platform.machine()}")
    tool_dir = root / "target" / "reactorcide-tools" / f"buildctl-{BUILDCTL_VERSION}"
    binary = tool_dir / "buildctl"
    if binary.exists():
        return binary
    tool_dir.mkdir(parents=True, exist_ok=True)
    archive_path = tool_dir / "buildkit.tar.gz"
    url = (
        "https://github.com/moby/buildkit/releases/download/"
        f"v{BUILDCTL_VERSION}/buildkit-v{BUILDCTL_VERSION}.linux-{architecture}.tar.gz"
    )
    print(f"Download buildctl from {url}", flush=True)
    urllib.request.urlretrieve(url, archive_path)
    actual_checksum = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    expected_checksum = BUILDCTL_CHECKSUMS[architecture]
    if actual_checksum != expected_checksum:
        archive_path.unlink(missing_ok=True)
        raise RuntimeError(
            "The BuildKit archive checksum is invalid: "
            f"expected {expected_checksum}, got {actual_checksum}"
        )
    with tarfile.open(archive_path, "r:gz") as archive:
        member = archive.getmember("bin/buildctl")
        source = archive.extractfile(member)
        if source is None:
            raise RuntimeError("The BuildKit archive did not contain buildctl")
        with binary.open("wb") as output:
            shutil.copyfileobj(source, output)
    binary.chmod(0o755)
    archive_path.unlink(missing_ok=True)
    return binary


def _wait_for_buildkit(buildctl: Path, root: Path) -> None:
    if not os.environ.get("BUILDKIT_HOST"):
        raise RuntimeError("The builder capability did not set BUILDKIT_HOST")
    for _ in range(30):
        result = subprocess.run(
            (str(buildctl), "debug", "info"),
            cwd=root,
            shell=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if result.returncode == 0:
            return
        time.sleep(1)
    raise RuntimeError("The BuildKit sidecar did not become ready")


def _build_one_target(
    root: Path,
    output: Path,
    version: str,
    platform: str,
    target: str,
    binary_name: str,
) -> Path:
    build_root = root / "target" / "release-container-builds"
    build_root.mkdir(parents=True, exist_ok=True)
    dockerfile = build_root / f"{platform}.Dockerfile"
    dockerfile.write_text(
        f"FROM {ZIGBUILD_IMAGE} AS build\n"
        "WORKDIR /io\n"
        "COPY . /io\n"
        f"RUN cargo zigbuild --release --manifest-path cli/Cargo.toml "
        f"--target {target}\n"
        "FROM scratch AS export\n"
        f"COPY --from=build /io/cli/target/{target}/release/{binary_name} "
        f"/{binary_name}\n",
        encoding="utf-8",
    )
    export_dir = build_root / platform
    export_dir.mkdir(parents=True, exist_ok=True)
    buildctl = _install_buildctl(root)
    _wait_for_buildkit(buildctl, root)
    _run(
        (
            str(buildctl),
            "build",
            "--frontend",
            "dockerfile.v0",
            "--local",
            f"context={root}",
            "--local",
            f"dockerfile={build_root}",
            "--opt",
            f"filename={dockerfile.name}",
            "--opt",
            "target=export",
            "--output",
            f"type=local,dest={export_dir}",
        ),
        cwd=root,
    )
    built_binary = export_dir / binary_name
    if not built_binary.is_file():
        raise RuntimeError(f"BuildKit did not export {binary_name} for {platform}")

    archive = output / f"csilctl-{version}-{platform}.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as tar:
        tar.add(built_binary, arcname=binary_name)
        tar.add(root / "README.md", arcname="README.md")
    return archive


def _build_release_artifacts(root: Path, version: str) -> Path:
    output = root / "target" / "release-artifacts"
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    for platform, target, binary_name in BUILD_TARGETS:
        _build_one_target(root, output, version, platform, target, binary_name)
    return output


def _semver_tags_binary(root: Path) -> Path:
    tool_dir = root / "target" / "reactorcide-tools" / "semver-tags-v0.6.0"
    binary = tool_dir / "semver-tags"
    if not binary.exists():
        home = root / "target" / "reactorcide-home"
        go_path = home / "go"
        environment = {
            "HOME": str(home),
            "GOPATH": str(go_path),
            "GOCACHE": str(home / ".cache" / "go-build"),
            "GOMODCACHE": str(go_path / "pkg" / "mod"),
            "GOBIN": str(tool_dir),
        }
        for directory in environment.values():
            if directory.startswith(str(root)):
                Path(directory).mkdir(parents=True, exist_ok=True)
        tool_dir.mkdir(parents=True, exist_ok=True)
        _run(
            ("go", "install", "github.com/catalystcommunity/semver-tags@v0.6.0"),
            cwd=root,
            env=environment,
        )
    return binary


def _semver_tags(root: Path, *, dry_run: bool) -> dict:
    binary = _semver_tags_binary(root)
    args = [str(binary), "run", "--output_json", "--directories", "cli"]
    if dry_run:
        args.append("--dry_run")
    result = _run(args, cwd=root, capture=True)
    start = result.stdout.find("{")
    if start < 0:
        raise RuntimeError("semver-tags did not return JSON output")
    return json.loads(result.stdout[start:])


def _github_request(
    method: str,
    url: str,
    token: str,
    *,
    body: bytes | None = None,
    content_type: str = "application/json",
) -> Any:
    request = urllib.request.Request(url, data=body, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    if body is not None:
        request.add_header("Content-Type", content_type)
    with urllib.request.urlopen(request) as response:
        payload = response.read()
    return json.loads(payload) if payload else {}


def _create_github_release(
    token: str,
    repository: str,
    tag: str,
    notes: str,
) -> dict:
    api = f"https://api.github.com/repos/{repository}"
    payload = json.dumps(
        {
            "tag_name": tag,
            "name": tag,
            "body": notes or "No release notes were generated.",
            "draft": False,
            "prerelease": False,
        }
    ).encode("utf-8")
    release = _github_request("POST", f"{api}/releases", token, body=payload)
    if not isinstance(release, dict):
        raise RuntimeError("GitHub returned an invalid release")
    print(f"Created GitHub Release {tag}", flush=True)
    return release


def _upload_release_artifacts(
    token: str,
    release: Mapping[str, Any],
    artifacts: Path,
) -> None:
    upload_url_value = release.get("upload_url")
    if not isinstance(upload_url_value, str):
        raise RuntimeError("GitHub returned an invalid release upload URL")
    upload_url = upload_url_value.split("{", 1)[0]
    for artifact in sorted(artifacts.glob("*.tar.gz")):
        query = urllib.parse.urlencode({"name": artifact.name})
        _github_request(
            "POST",
            f"{upload_url}?{query}",
            token,
            body=artifact.read_bytes(),
            content_type="application/gzip",
        )
        print(f"Uploaded {artifact.name}", flush=True)


def _release_notes(metadata: Mapping[str, Any]) -> str:
    notes_text = metadata.get("New_release_notes_json")
    if not isinstance(notes_text, str):
        return ""
    notes_json = json.loads(notes_text).get("new_release_notes_escaped", {})
    package_notes = notes_json.get(f"package_{PACKAGE}", [])
    if not isinstance(package_notes, list):
        return ""
    return "\n".join(note for note in package_notes if isinstance(note, str))


def main() -> None:
    root = Path.cwd()
    repository = os.environ.get("REACTORCIDE_REPO", "catalystcommunity/csilctl")
    token = os.environ.get("GITHUB_PAT")
    if not token:
        raise RuntimeError("GITHUB_PAT is required to publish a release")

    _run(("git", "fetch", "--tags", "--force"), cwd=root)

    preview = _semver_tags(root, dry_run=True)
    published = preview.get("New_release_published")
    if published != "true":
        lsresult = _run(("ls",), cwd=root, capture=True)
        print(f'published is {published}, and ls is {lsresult.stdout}')
        print("No new csilctl release is required.", flush=True)
        return

    tag = preview.get("New_release_git_tag", "")
    match = RELEASE_TAG.fullmatch(tag)
    if not match:
        raise RuntimeError(f"semver-tags returned an invalid release tag: {tag}")
    version = match.group("version")
    notes = _release_notes(preview)

    artifacts = _build_release_artifacts(root, version)

    _run(
        (
            "git",
            "remote",
            "set-url",
            "origin",
            f"https://x-access-token:{token}@github.com/{repository}.git",
        ),
        cwd=root,
    )
    try:
        actual = _semver_tags(root, dry_run=False)
    finally:
        _run(
            (
                "git",
                "remote",
                "set-url",
                "origin",
                f"https://github.com/{repository}.git",
            ),
            cwd=root,
        )
    if actual.get("New_release_git_tag") != tag:
        raise RuntimeError("semver-tags pushed a different tag than the preview")

    release = _create_github_release(token, repository, tag, notes)
    _upload_release_artifacts(token, release, artifacts)
    print(f"Published csilctl {tag}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # noqa: BLE001
        print(f"error: {exc}", file=sys.stderr, flush=True)
        sys.exit(1)
