# SSHTUP

An SSH client with self-hosted sync.

A native desktop SSH client written in Rust ([egui](https://github.com/emilk/egui)), with an optional
sync server you host yourself.

- Hosts, groups, keychain, snippets, port forwarding, known hosts, session logs
- SFTP browser with two panes, host to host transfers and path autocomplete
- Command history and autocomplete
- Optional master password: the vault, logs and history are encrypted (Argon2id + XChaCha20-Poly1305)
- **Sync** between your devices through your own server, end-to-end encrypted: the server only stores sealed data
- **Teams**: share a host or a whole group with other people on the same server
- **Shared terminals**: let someone watch (and, if you allow it, type into) a terminal, end-to-end encrypted

## Build

```bash
cargo build --release --bin sshtup          # the desktop app
cargo build --release -p sync-server               # the sync server (optional)
```

On Linux the window toolkit needs a few system libraries; see `.github/workflows/ci.yml`.
Platform status is in [PLATFORMS.md](PLATFORMS.md).

## Self-hosting

- [SYNC.md](SYNC.md): run the server, TLS, backups, teams, what the server can and cannot see
- [RELAY.md](RELAY.md): shared terminals (the relay built into the server)
- `deploy/`: helper scripts

## Status

Early software. The encryption design has not had an independent review: read [SYNC.md](SYNC.md)
before trusting it with anything important.

## License

[MIT](LICENSE). The colour schemes other than "Default Dark/Light" are community themes
(Dracula, Solarized, Nord, Gruvbox, Catppuccin and others) that belong to their authors.
