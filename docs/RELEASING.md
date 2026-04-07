# Releasing

## Automated flow

1. Merge changes into `main` using conventional commits such as `feat:`, `fix:`, and `chore:`.
2. Wait for the `Release Please` workflow to open or update the release PR.
3. Review the generated version bump and `CHANGELOG.md` updates in that PR.
4. Merge the release PR to create the git tag and release metadata.
5. Push the resulting `v*` tag if it is not pushed automatically.
6. Let `.github/workflows/release.yml` build platform binaries, upload release assets, and publish npm packages with `NPM_TOKEN`.

## Manual sanity checks before a release

1. Run `cargo test --workspace`.
2. Run `npm test --workspaces --if-present`.
3. Run `node ./scripts/stage-npm-binary.js --component http-server --profile release`.
4. Run `npm pack --workspace @pulsur/http-server`.
5. Install the tarball in a clean directory and verify `require('@pulsur/http-server').getBinaryPath()` resolves correctly.

## Versioning rules

- `feat:` creates a minor release.
- `fix:` creates a patch release.
- `BREAKING CHANGE:` creates a major release.
- All Rust crates stay aligned through `version.workspace` in the root `Cargo.toml`.
