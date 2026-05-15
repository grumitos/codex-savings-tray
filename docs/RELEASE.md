# Release Checklist

1. Run local checks:

   ```powershell
   cargo fmt --check
   cargo test
   cargo clippy --all-targets -- -D warnings
   cargo audit
   cargo build --release
   .\target\release\codex-savings-tray.exe --once --all-time
   ```

   `cargo audit` requires the `cargo-audit` subcommand; install it before a
   release if the command is not available locally.

2. Update `CHANGELOG.md`, bump `version` in `Cargo.toml`, and choose the
   release tag, for example `v0.2.1`.

3. Write bilingual release notes in `dist/release-notes-vX.Y.Z.md`, with
   English and Spanish sections for highlights, validation, assets, and any
   known caveats. `dist/` is ignored by git; these notes are local release
   artifacts passed to `gh release`.

4. Commit the tracked release prep, such as the version bump, changelog, and
   documentation updates.

5. Tag and push:

   ```powershell
   $version = "v0.2.1"
   git tag $version
   git push origin main
   git push origin $version
   ```

6. Package and publish:

   ```powershell
   $version = "v0.2.1"
   $exeName = "codex-savings-tray-$version-windows-x64.exe"
   $shaName = "codex-savings-tray-$version-windows-x64.sha256"
   New-Item -ItemType Directory -Force dist | Out-Null
   Copy-Item target/release/codex-savings-tray.exe "dist/$exeName" -Force
   $hash = (Get-FileHash "dist/$exeName" -Algorithm SHA256).Hash.ToLowerInvariant()
   "$hash  $exeName" | Set-Content -Path "dist/$shaName" -NoNewline -Encoding ascii
   gh release create $version "dist/$exeName" "dist/$shaName" --title $version --notes-file "dist/release-notes-$version.md" --latest
   ```

   To refresh an existing release without changing the version, move the tag to
   the new commit, replace the assets, and edit the release notes:

   ```powershell
   $version = "v0.2.1"
   git tag -f $version
   git push --force origin "refs/tags/$version"
   gh release upload $version "dist/codex-savings-tray-$version-windows-x64.exe" "dist/codex-savings-tray-$version-windows-x64.sha256" --clobber
   gh release edit $version --notes-file "dist/release-notes-$version.md" --latest
   ```

## Signing

The current local build can be signed with a local development certificate, but
public releases should use a real code-signing certificate.

## CI Automation

No GitHub Actions workflow is tracked in this repo. Releases remain manual
until a CI/release workflow is added intentionally.
