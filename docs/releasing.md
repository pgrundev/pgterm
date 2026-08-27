# Releasing pgterm

1. Bump `version` in `Cargo.toml` (the release smoke job asserts the binary's
   `--version` matches the tag, so they must agree), run `cargo build` once so
   `Cargo.lock` picks it up, commit.
2. Tag and push:

   ```bash
   git tag v0.1.0
   git push origin main v0.1.0
   ```

3. The `release` workflow builds native binaries for linux amd64/arm64
   (musl, static) and macOS amd64/arm64, attaches
   `pgterm_<version>_<os>_<arch>.tar.gz` + `checksums.txt` to a GitHub
   release, then smoke-downloads and runs `--version` on linux and macOS.

Windows is not shipped yet (crossterm supports it — revisit post-MVP).
