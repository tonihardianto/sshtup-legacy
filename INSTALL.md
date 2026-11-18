# Install SSHTUP

Download the package for your system from the [latest release](https://github.com/tonihardianto/sshtup/releases/latest).
`SHA256SUMS.txt` lists the checksum of every file.

The packages are **not signed or notarized** yet, so macOS and Windows warn about them the first
time. That is expected; the steps below get past it.

## macOS

Use the `.dmg` (or the `.zip`) that matches your Mac: `arm64` for Apple silicon, `x86_64` for Intel.
Drag **SSHTUP** to Applications.

macOS will say it cannot verify that SSHTUP is free of malware. To open it anyway:

1. Try to open the app once and press **Done**.
2. Open **System Settings → Privacy & Security**, scroll to **Security**, and press **Open Anyway** next to "SSHTUP was blocked". Enter your password.

Or remove the quarantine flag in a terminal:

```bash
xattr -dr com.apple.quarantine /Applications/SSHTUP.app
```

## Windows

Run `sshtup-setup-windows-x86_64.exe` (per-user install, no administrator rights needed), or use
the portable `sshtup-windows-x86_64.exe` and run it from anywhere.

If SmartScreen shows "Windows protected your PC", press **More info → Run anyway**.
SSHTUP runs the system `ssh`, `sftp` and `ssh-keygen`, so install the OpenSSH client
(Settings → Apps → Optional features) if you do not have it. See [PLATFORMS.md](PLATFORMS.md) for what is still missing on Windows.

## Linux

- **Debian / Ubuntu:** `sudo apt install ./sshtup-linux-x86_64.deb`
- **AppImage:** `chmod +x sshtup-linux-x86_64.AppImage && ./sshtup-linux-x86_64.AppImage`
- **Any distribution:** unpack `sshtup-linux-x86_64.tar.gz` and run `sshtup`

The packages are built on a recent Ubuntu; older distributions may lack a newer system library.

## Sync server

`sshtup-sync-server-linux-x86_64.tar.gz` is the optional server. See [SYNC.md](SYNC.md) for running it and [Updating the server](SYNC.md#updating-the-server).

## Build from source

```bash
cargo build --release --bin sshtup          # the desktop app
cargo build --release -p sync-server               # the sync server (optional)
```

On Linux the window toolkit needs a few system libraries; see `.github/workflows/ci.yml`.
Platform status is in [PLATFORMS.md](PLATFORMS.md).
