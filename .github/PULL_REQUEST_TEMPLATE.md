## What and why

<!-- One or two sentences. Link the issue if there is one. -->

## Checks

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --all` pass (or `./scripts/check.sh` / `.\scripts\check.ps1`)
- [ ] Scanning still never writes; cleanup still goes plan → quarantine → purge
- [ ] New adapter paths: OS and agent version where you verified them are in the PR text
- [ ] CHANGELOG.md updated for user-visible changes
