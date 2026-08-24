# Releasing

1. Ensure `CHANGELOG.md` describes the release and the workspace version in
   `Cargo.toml` is strict SemVer.
2. Run the complete local verification contract from `AGENTS.md`.
3. Commit with a clean working tree and create an annotated `v<version>` tag.
4. Push the tag. The release workflow verifies that the tag matches Cargo,
   builds native archives, generates SHA-256 files, and publishes a GitHub
   Release.
5. Download at least one archive, verify its checksum, and run
   `mini-codex --version` and `mini-codex doctor` from the extracted binary.

Release archives contain the binary, `README.md`, `LICENSE`, and
`CHANGELOG.md`. No release job uses provider credentials or makes a paid model
request.
