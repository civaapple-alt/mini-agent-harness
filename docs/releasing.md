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

The commands below use the repository's current `1.0.0` version as a concrete
example. For a later release, replace the version in the checklist, tag, archive
names, and verification commands together; never publish a different commit
under an existing tag.

Coordinate this version with the companion `mini-agent-web` repository. Its
Python SDK, Gateway, frontend, and lockfiles must use the same release number.
Create each repository's release tag only after both release commits pass their
own checks.

## Before changing the version

Confirm the release scope and review the complete diff. For a patch or minor
release, every user-visible behavior change should be represented in
`CHANGELOG.md`; breaking changes require an explicit migration note and a
major-version decision.

For `1.0.0`, check:

- [ ] The release scope is agreed and no unrelated work is included.
- [ ] `README.md` answers “what is it, how do I install it, and how do I run it”
      without requiring the reader to understand the architecture first.
- [ ] `CHANGELOG.md` has a dated `1.0.0` section and an empty `Unreleased`
      section for subsequent work.
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
rg -n '^## \[(Unreleased|1\.0\.0)\]' CHANGELOG.md
```

Keep `Unreleased` at the top. Move the completed entries into the dated
release section, and leave `Unreleased` as `No changes yet.` after the release
content is frozen.

## Local verification

Run the repository contract on the machine where the release is prepared:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/line_budget.py
cargo build --release --locked -p mini-agent-cli
```

Run affected package tests locally. The full workspace test matrix is evidence
from CI; do not run `cargo test --workspace` locally without explicit approval.
The release tag must point to the commit whose CI matrix passed.

Exercise the built binary without contacting a provider:

```sh
./target/release/mini-agent --version
```

On Windows, use the equivalent `target\\release\\mini-agent.exe` commands.
The Windows environment also needs PowerShell 7 (`pwsh`) for shell-tool
coverage. Do not use a paid provider call as a release gate unless it has been
explicitly authorized; the workspace tests and binary version check are the
default release checks.

Review the package inputs before tagging:

```sh
cargo package --workspace --locked --no-verify
git diff --check
git status --short
```

The current hard gates are 7,000 effective Core + Protocol lines, 45,000
Control Plane lines, and 65,000 Release Rust lines, including tests in supported
packages. `scripts/line_budget.py` is the source of truth. It excludes blank and
comment-only lines; code-bearing lines with trailing comments count once.
The experimental CLI/REPL is reported by the budget script but is excluded from
the release-source gate.
Keep each pull request near 1,000 net effective Release Rust lines where
practical; this is review guidance rather than a hard limit. Run
`python3 scripts/line_budget.py --base <merge-base> --check-delta --json` to
report the increment and check all three absolute hard limits.

The release archives contain only the binary, `README.md`, `LICENSE`, and
`CHANGELOG.md`. `scripts/package_release.py` creates deterministic archives and
their `.sha256` files.

## Commit and tag

Commit the version, changelog, README, and documentation together. The tag
must point at the exact commit that passed local review and CI:

```sh
git status --short
git add Cargo.toml Cargo.lock crates/*/Cargo.toml README.md CHANGELOG.md docs scripts/line_budget.py
git commit -m "release: prepare v1.0.0"
git push origin main
git tag -a v1.0.0 -m "Release v1.0.0"
git push origin v1.0.0
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
2. builds Linux x86_64, macOS x86_64, macOS arm64, and Windows x86_64;
3. packages each binary with the public release files;
4. verifies every downloaded archive against its SHA-256 file; and
5. publishes the GitHub Release and generated release notes.

Do not manually upload replacement archives while the workflow is running.
If it fails, inspect the failed job and fix the source or workflow before
retrying. A manual dispatch is appropriate for rerunning a verified existing
tag, not for publishing a different commit under the same tag.

## Post-release verification

After the workflow succeeds, open the
[v1.0.0 release page](https://github.com/civaapple-alt/mini-agent-harness/releases/tag/v1.0.0)
and verify that all four platform archives and matching `.sha256` files are
present. Download at least one archive from each operating system family when
possible.

On macOS/Linux:

```sh
shasum -a 256 -c mini-agent-v1.0.0-<target>.tar.gz.sha256
tar -xzf mini-agent-v1.0.0-<target>.tar.gz
./mini-agent-v1.0.0-<target>/mini-agent --version
```

On Windows PowerShell:

```powershell
Get-FileHash .\\mini-agent-v1.0.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256
Expand-Archive .\\mini-agent-v1.0.0-x86_64-pc-windows-msvc.zip .\\mini-agent-v1.0.0
.\\mini-agent-v1.0.0\\mini-agent.exe --version
```

Confirm that `--version` reports `1.0.0`. Then announce the release with a
short summary, supported platforms, upgrade instructions, and known
limitations. Link to the GitHub Release rather than attaching unverified
builds elsewhere.

## Rollback and follow-up

If an archive is broken before broad adoption, mark the GitHub Release as a
pre-release or remove it from the release page while the maintainer decides
whether to issue a new patch version. Do not silently replace a published
archive: users
must be able to reproduce the checksum from the tagged source.

After publishing, open a fresh `Unreleased` section for follow-up work and
record any release incident, platform gap, or documentation correction in the
next changelog entry.
