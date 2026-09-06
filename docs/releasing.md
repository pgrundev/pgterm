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

4. The `homebrew` job renders `Formula/pgterm.rb` from those checksums
   (`packaging/homebrew/formula.sh`) and pushes it to
   [`pgrundev/homebrew-tap`](https://github.com/pgrundev/homebrew-tap); then
   `brew-smoke` does a real `brew install pgrundev/tap/pgterm` on a fresh mac
   and checks `--version`. Do not edit the formula in the tap by hand — the
   next release overwrites it.

Windows is not shipped yet (crossterm supports it — revisit post-MVP). The
test suite does run on Windows in CI (`tests/bin/fake_pgbot.rs` is the
fixture there), so a Windows build is one matrix entry away.

## Homebrew: how the tap is wired

The workflow's `GITHUB_TOKEN` cannot write to another repository, so the
formula push authenticates with a write **deploy key** registered on the tap
repo. Its private half is the secret `HOMEBREW_TAP_DEPLOY_KEY` on
`pgrundev/pgterm`. Without it the `homebrew` job fails loudly (and
`brew-smoke` never runs) — a lagging formula must never pass silently.

One-time setup, repeated only to rotate the key:

```bash
# A fresh ed25519 pair, no passphrase (CI cannot prompt).
ssh-keygen -t ed25519 -N "" -C "pgterm release → homebrew-tap" -f /tmp/pgterm-tap-key

# Public half → write deploy key on the TAP repo.
gh repo deploy-key add /tmp/pgterm-tap-key.pub --repo pgrundev/homebrew-tap \
  --title "pgterm release workflow (formula push)" --allow-write

# Private half → secret on THIS repo, read by release.yml.
gh secret set HOMEBREW_TAP_DEPLOY_KEY --repo pgrundev/pgterm < /tmp/pgterm-tap-key

# Don't leave the private key on disk.
rm -f /tmp/pgterm-tap-key /tmp/pgterm-tap-key.pub
```

The formula deliberately has **no** `depends_on "pgrundev/tap/pgbot"`:
Homebrew 6 trusts only the third-party formulae named on the command line
(`explicitly_allowed?` in Homebrew's `trust.rb`), so a dependency from the
same tap is refused with "untrusted tap" unless the user ran `brew trust`.
Instead the README installs both by name — `brew install pgrundev/tap/pgterm
pgrundev/tap/pgbot` — and the formula's caveats say how to add pgbot later.
`brew-smoke` runs that same two-formula command. Render the formula locally
to eyeball a release:

```bash
gh release download v0.1.3 -R pgrundev/pgterm -p checksums.txt
sh packaging/homebrew/formula.sh 0.1.3 checksums.txt
```
