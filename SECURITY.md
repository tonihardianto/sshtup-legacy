# Security policy

SSHTUP handles SSH credentials and encrypted data, so security reports are taken
seriously.

## Reporting a vulnerability

Please report it privately through GitHub:
**Security tab → "Report a vulnerability"** on this repository.

Do not open a public issue or pull request for a vulnerability. Include what you
found, how to reproduce it, and which version or commit you tested. You will get
a reply as soon as the maintainer can, and the fix is credited to you unless you
prefer otherwise.

## What is in scope

- The app: vault encryption, key and password handling, SSH and SFTP.
- The sync server and client: accounts, end-to-end encrypted sync, teams.
- The shared-terminal relay and its end-to-end encryption.

## Supported versions

The project is young: only the latest commit on `main` is supported.

## Running your own server

Servers are self-hosted. Put the sync server behind TLS, run it as an
unprivileged user, and use `--trust-proxy` only when a reverse proxy on the same
machine forwards the real client address. See `SYNC.md` and `RELAY.md`.
