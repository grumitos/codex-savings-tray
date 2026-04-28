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

5. GitHub Actions will build the Windows x64 `.exe` and attach a zip to the
   GitHub Release.

## Signing

The current local build can be signed with a local development certificate, but
public releases should use a real code-signing certificate. GitHub Actions does
not sign the binary yet.
