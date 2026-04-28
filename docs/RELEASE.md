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

2. Update `CHANGELOG.md` and bump `version` in `Cargo.toml`.

3. Commit the release prep.

4. Tag and push:

   ```powershell
   git tag v0.1.0
   git push origin main
   git push origin v0.1.0
   ```

5. Package and publish:

   ```powershell
   New-Item -ItemType Directory -Force dist | Out-Null
   Copy-Item target/release/codex-savings-tray.exe dist/
   Compress-Archive -Path dist/codex-savings-tray.exe -DestinationPath dist/codex-savings-tray-windows-x64.zip -Force
   gh release create v0.1.0 dist/codex-savings-tray-windows-x64.zip --title "Codex Savings Tray v0.1.0" --notes-file dist/release-notes-v0.1.0.md --latest
   ```

## Signing

The current local build can be signed with a local development certificate, but
public releases should use a real code-signing certificate.

## CI Automation

GitHub rejected workflow files from the local `gh` token because it lacks the
`workflow` scope. After refreshing auth with that scope, add CI/release
workflows and switch releases back to tag-triggered automation.
