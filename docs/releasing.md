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

## Before changing the version

Confirm the release scope and review the complete diff. For a patch or minor
release, every user-visible behavior change should be represented in
`CHANGELOG.md`; breaking changes require an explicit migration note and a
major-version decision.

For `0.7.0`, check:

- [ ] The release scope is agreed and no unrelated work is included.
- [ ] `README.md` answers “what is it, how do I install it, and how do I run it”
      without requiring the reader to understand the architecture first.
- [ ] `CHANGELOG.md` has a dated `0.7.0` section and an empty `Unreleased`
      section for subsequent work.
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
rg -n '^## \[(Unreleased|0\.7\.0)\]' CHANGELOG.md
```

Keep `Unreleased` at the top. Move the completed entries into the dated
release section, and leave `Unreleased` as `No changes yet.` after the release
content is frozen.

## Local verification

Run the repository contract on the machine where the release is prepared:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/line_budget.py
cargo build --release --locked -p mini-agent-cli
```

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

For the 0.7.0 release, both the 20,000-line runtime budget and the 30,000-line
release-source total, including tests in supported packages, are hard gates.
The experimental CLI/REPL is reported by the budget script but is excluded from
the release-source gate.

The release archives contain only the binary, `README.md`, `LICENSE`, and
`CHANGELOG.md`. `scripts/package_release.py` creates deterministic archives and
their `.sha256` files.

## Commit and tag

Commit the version, changelog, README, and documentation together. The tag
must point at the exact commit that passed local review and CI:

```sh
git status --short
git add Cargo.toml Cargo.lock crates/mini-agent-cli/Cargo.toml README.md CHANGELOG.md docs scripts/line_budget.py
git commit -m "release: prepare v0.7.0"
git push origin main
git tag -a v0.7.0 -m "Release v0.7.0"
git push origin v0.7.0
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
[v0.7.0 release page](https://github.com/civaapple-alt/mini-agent-harness/releases)
and verify that all four platform archives and matching `.sha256` files are
present. Download at least one archive from each operating system family when
possible.

On macOS/Linux:

```sh
shasum -a 256 -c mini-agent-v0.7.0-<target>.tar.gz.sha256
tar -xzf mini-agent-v0.7.0-<target>.tar.gz
./mini-agent-v0.7.0-<target>/mini-agent --version
```

On Windows PowerShell:

```powershell
Get-FileHash .\\mini-agent-v0.7.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256
Expand-Archive .\\mini-agent-v0.7.0-x86_64-pc-windows-msvc.zip .\\mini-agent-v0.7.0
.\\mini-agent-v0.7.0\\mini-agent.exe --version
```

Confirm that `--version` reports `0.7.0`. Then announce the release with a
short summary, supported platforms, upgrade instructions, and known
limitations. Link to the GitHub Release rather than attaching unverified
builds elsewhere.

## Rollback and follow-up

If an archive is broken before broad adoption, mark the GitHub Release as a
pre-release or remove it from the release page while the maintainer decides
whether to issue `0.4.1`. Do not silently replace a published archive: users
must be able to reproduce the checksum from the tagged source.

After publishing, open a fresh `Unreleased` section for follow-up work and
record any release incident, platform gap, or documentation correction in the
next changelog entry.
