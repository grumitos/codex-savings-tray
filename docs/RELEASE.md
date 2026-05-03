# Release Checklist

1. Run local checks:

   ```powershell
   cargo fmt -- --check
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
   git tag v0.2.1
   git push origin main
   git push origin v0.2.1
   ```

5. Package and publish:

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

## Signing

The current local build can be signed with a local development certificate, but
public releases should use a real code-signing certificate.

## CI Automation

GitHub rejected workflow files from the local `gh` token because it lacks the
`workflow` scope. After refreshing auth with that scope, add CI/release
workflows and switch releases back to tag-triggered automation.
