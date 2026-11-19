# Contributing to SSHTUP

Thanks for wanting to help. Bug reports, fixes and ideas are all welcome.

## Build and test

You need a recent stable Rust toolchain.

```sh
cargo build --bin sshtup      # build the app first: some tests start it
cargo test --workspace
```

The workspace has four crates: the app (`.`), `sync-core` (shared crypto and
formats), `sync-server` and `sync-client`. `SYNC.md` and `RELAY.md` describe the
sync server and the shared-terminal relay; `PLATFORMS.md` lists what works on
each operating system.

## Sending a change

1. Fork the repository and create a branch from `main`.
2. Make the change. Keep it focused: one topic per pull request.
3. Add or update tests when behaviour changes. A bug fix should come with a test
   that fails without it.
4. Run `cargo test --workspace` before you push.
5. Open a pull request and say what it changes and why.

`main` is protected: nothing is pushed to it directly. A pull request can be
merged only when the checks pass on macOS, Linux and Windows. If a check fails
on a platform you cannot test, say so in the pull request and we will help.

## Things to keep in mind

- **Security-sensitive code** (anything in `sync-core`, `vaultcrypt`, the relay
  protocol, key handling): explain the reasoning in the pull request. Changing a
  cryptographic label, format or protocol breaks existing data, so it needs a
  clear migration story.
- **Secrets**: never commit real hosts, keys, passwords, tokens or addresses of
  personal servers. Use placeholders in docs and tests.
- **Platforms**: do not assume Unix. Gate Unix-only code with `#[cfg(unix)]`
  and give Windows a sensible fallback.
- **Style**: follow the surrounding code and run `cargo fmt` on files you touch
  if the file is already formatted.

## License

By contributing you agree that your work is released under the MIT license that
covers this project (see `LICENSE`).

## Security problems

Do not open a public issue for a vulnerability. See `SECURITY.md`.
