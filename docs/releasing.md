# Release process

This is the release runbook for Mini Agent Harness. A release is a versioned
Git commit, an immutable `vX.Y.Z` tag, a GitHub Release, and verified native
archives. GitHub describes releases as packages built from Git tags; this
repository follows that model ([GitHub release documentation](https://docs.github.com/en/repositories/releasing-projects-on-github)).

The release workflow is intentionally tag-driven:

```text
clean commit
  -> CI on the commit
  -> annotated vX.Y.Z tag
  -> push tag
  -> verify Cargo version
  -> build Linux/macOS/Windows archives
  -> verify SHA-256 checksums
  -> publish GitHub Release with generated notes
```

The workflow does not use provider credentials and does not make paid model
requests.

Provider calls are not part of the release gate. Use local tests and build
verification; paid provider checks, if needed, belong in an external evaluation
harness.

`1.0.0` is the already published baseline. The commands below illustrate the
release procedure; source changes made after the `v1.0.0` tag require a new
version and a new tag. Never dispatch the old tag to include newer source or
replace the existing release assets.

The Python SDK is versioned and released with Harness. The companion
`mini-agent-web` repository has its own Gateway/frontend release version and
uses a sibling Harness checkout as an editable SDK source during development.
Its GitHub Release does not attach SDK artifacts. The historical SDK 1.0.0
wheel remains attached to the Web v1.0.0 release; new SDK artifacts are
attached to Harness releases.

## Before changing the version

Confirm the release scope and review the complete diff. For a patch or minor
release, every user-visible behavior change should be represented in
`CHANGELOG.md`; breaking changes require an explicit migration note and a
major-version decision.

Before each release, check:

- [ ] The release scope is agreed and no unrelated work is included.
- [ ] `README.md` answers “what is it, how do I install it, and how do I run it”
      without requiring the reader to understand the architecture first.
- [ ] `CHANGELOG.md` has a dated section for the release version and an empty
      `Unreleased` section for subsequent work.
- [ ] The App Server V2 breaking change is called out, and operators can find
      the V1 Session backup instructions in `docs/app-server.md`.
- [ ] Configuration, limits, troubleshooting, security, and privacy docs agree
      with the current implementation.
- [ ] No credentials, local paths, build output, or generated session data are
      committed.

## Version and changelog

Update the single workspace version in the root `Cargo.toml`. Update any
internal crate dependency that pins the workspace version, then let Cargo
refresh `Cargo.lock` if package version entries change.

Use strict SemVer and the `v` prefix for the Git tag:

```sh
rg -n '^version = |mini-agent-core = ' Cargo.toml crates/*/Cargo.toml
rg -n '^## \[(Unreleased|1\.1\.0)\]' CHANGELOG.md
```

Keep `Unreleased` at the top. Move the completed entries into the dated
release section, and leave `Unreleased` as `No changes yet.` after the release
content is frozen.

## Local verification

Run the repository contract and SDK package checks on the machine where the
release is prepared:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/line_budget.py
python3 scripts/test_package_release.py
cargo build --release --locked -p mini-agent-app-server
python3 scripts/check_sdk_version.py
uv sync --project sdk/python --locked --group dev
uv run --project sdk/python --locked ruff check sdk/python/src sdk/python/tests cookbook/python-demo
uv run --project sdk/python --locked ruff format --check sdk/python/src sdk/python/tests cookbook/python-demo
uv run --project sdk/python --locked pytest sdk/python/tests -q
uv run --project sdk/python --locked python cookbook/python-demo/06_protocol_compatibility.py
uv build sdk/python --out-dir dist
(cd dist && sha256sum mini_agent-*.whl mini_agent-*.tar.gz > SHA256SUMS && sha256sum --check SHA256SUMS)
```

Run affected package tests locally. The full workspace test matrix is evidence
from CI; do not run `cargo test --workspace` locally without explicit approval.
The release tag must point to the commit whose CI matrix passed.

The release archive contains the App Server used by the SDK and Web Studio.
Verify it through the SDK without contacting a provider:

```sh
MINI_AGENT_APP_SERVER_PATH="$PWD/target/release/mini-agent-app-server" uv run --project sdk/python --locked python - <<'PY'
import asyncio
from mini_agent import MiniAgentClient

async def main():
    async with MiniAgentClient() as client:
        result = await client.initialize()
        print(result["serverVersion"])

asyncio.run(main())
PY
```

On Windows, point `MINI_AGENT_APP_SERVER_PATH` at the extracted
`mini-agent-app-server.exe` before running the SDK initialization check.
The Windows environment also needs PowerShell 7 (`pwsh`) for shell-tool
coverage. Do not use a paid provider call as a release gate unless it has been
explicitly authorized; the workspace tests and SDK initialization check are
the default release checks.

Review the package inputs before tagging:

```sh
cargo package --workspace --locked --no-verify
git diff --check
git status --short
```

The current hard gates are 7,500 effective Core + Protocol lines, 45,000
Control Plane lines, and 65,000 Release Rust lines, including tests in supported
packages. `scripts/line_budget.py` is the source of truth. It excludes blank and
comment-only lines; code-bearing lines with trailing comments count once.
The experimental CLI/REPL is reported by the budget script but is excluded from
the release-source gate.
Keep each pull request near 1,000 net effective Release Rust lines where
practical; this is review guidance rather than a hard limit. Run
`python3 scripts/line_budget.py --base <merge-base> --check-delta --json` to
report the increment and check all three absolute hard limits.

Each platform archive contains `mini-agent-app-server` plus `README.md`,
`LICENSE`, and `CHANGELOG.md`; it does not include the interactive CLI.
`scripts/package_release.py` creates deterministic archives named
`mini-agent-app-server-v<version>-<target>` and their `.sha256` files. The SDK
release job also builds a wheel and sdist and publishes their `SHA256SUMS` file.

## Commit and tag

Commit the version, changelog, README, and documentation together. The tag
must point at the exact commit that passed local review and CI:

```sh
git status --short
git add Cargo.toml Cargo.lock README.md CHANGELOG.md docs/releasing.md sdk/python/README.md .github/workflows/release.yml scripts/package_release.py scripts/test_package_release.py
git commit -m "release: prepare v<version>"
git push origin main
git tag -a v<version> -m "Release v<version>"
git push origin v<version>
```

Do not move or overwrite an existing release tag. If the commit is wrong,
delete neither data nor history casually; create a corrective commit and use a
new version unless the repository maintainer has an explicit tag-repair plan.

## GitHub Actions release

`.github/workflows/release.yml` starts when a `v*.*.*` tag is pushed. It can
also be started manually with an existing tag through **Actions → Release →
Run workflow**.

The workflow:

1. checks that the tag is strict SemVer and exactly matches the root Cargo
   version;
2. builds the App Server for Linux x86_64, macOS x86_64, macOS arm64, and
   Windows x86_64;
3. packages the App Server with the public release files;
4. builds the Python SDK wheel and sdist and verifies all downloaded checksums;
5. publishes the GitHub Release and generated release notes.

Do not manually upload replacement archives while the workflow is running.
If it fails, inspect the failed job and fix the source or workflow before
retrying. A manual dispatch is appropriate for rerunning a verified existing
tag, not for publishing a different commit under the same tag.

## Post-release verification

After the workflow succeeds, open the
[Harness Releases page](https://github.com/civaapple-alt/mini-agent-harness/releases)
and verify that the new release contains all four App Server platform archives
and matching `.sha256` files, the Python wheel and sdist, and `SHA256SUMS`.
The existing v1.0.0 release contains only CLI archives; v1.1.0 is the first
release whose platform archives are intended for SDK and Web Studio use.
Download at least one archive from each operating system family when possible.

On macOS/Linux:

```sh
shasum -a 256 -c mini-agent-app-server-v<version>-<target>.tar.gz.sha256
tar -xzf mini-agent-app-server-v<version>-<target>.tar.gz
MINI_AGENT_APP_SERVER_PATH="$PWD/mini-agent-app-server-v<version>-<target>/mini-agent-app-server" uv run --project sdk/python --locked python - <<'PY'
import asyncio
from mini_agent import MiniAgentClient

async def main():
    async with MiniAgentClient() as client:
        result = await client.initialize()
        print(result["serverVersion"])

asyncio.run(main())
PY
```

On Windows PowerShell:

```powershell
Get-FileHash .\\mini-agent-app-server-v<version>-x86_64-pc-windows-msvc.zip -Algorithm SHA256
Expand-Archive .\\mini-agent-app-server-v<version>-x86_64-pc-windows-msvc.zip .\\mini-agent-app-server-v<version>
```

On Windows, install the matching SDK wheel, set
`$env:MINI_AGENT_APP_SERVER_PATH` to the extracted
`mini-agent-app-server.exe`, and run the same `initialize()` check. Confirm the
archive contains only the App Server executable and public files, and that
`serverVersion` matches the tag. Then announce the release with a short summary,
supported platforms, upgrade instructions, and known limitations. Link to the
GitHub Release rather than attaching unverified builds elsewhere.

## Rollback and follow-up

If an archive is broken before broad adoption, mark the GitHub Release as a
pre-release or remove it from the release page while the maintainer decides
whether to issue a new patch version. Do not silently replace a published
archive: users
must be able to reproduce the checksum from the tagged source.

After publishing, open a fresh `Unreleased` section for follow-up work and
record any release incident, platform gap, or documentation correction in the
next changelog entry.
