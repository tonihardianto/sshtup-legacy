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
- **Shared terminals**: let someone watch (and, if you allow it, type into) a terminal, end-to-end encrypted. Everyone sees who is in the session and who is typing

![The host list](docs/screenshots/hosts.png)

![The SFTP browser](docs/screenshots/sftp.png)

## Install

Download the package for your system from the [latest release](https://github.com/tonihardianto/sshtup/releases/latest):
a `.dmg` for macOS, an installer or portable `.exe` for Windows, a `.deb`, AppImage or `.tar.gz` for Linux.
The packages are not signed yet, so macOS and Windows warn the first time. [INSTALL.md](INSTALL.md) has the steps for
each system, the optional sync server and building from source.

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
