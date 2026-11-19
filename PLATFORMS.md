# Platforms

| | Status |
|---|---|
| macOS | Built and used by the author. |
| Linux | Expected to work; not yet built here. Needs the window libraries listed in `.github/workflows/ci.yml`. |
| Windows | Builds are checked in CI, but it is not finished (below). |
| Sync server | Pure Rust with bundled SQLite: builds on all three. Runs on Linux x86_64 in production. |

## Known Windows gaps

- The app runs the system `ssh`, `sftp` and `ssh-keygen`: Windows needs the OpenSSH client installed and on PATH.
- The data folder is derived from `HOME`, which Windows does not set: it falls back to the working folder.
- The default local shell is `/bin/zsh`.
- The shared-terminal guest uses `stty`.
- "Open" in SFTP knows `open` (macOS) and `xdg-open` (Linux) only.
- File permission hardening (`chmod 600`) is Unix only.

The CI workflow builds and tests on all three systems on every push, and the Windows test results are shown without failing the run until these are fixed.
