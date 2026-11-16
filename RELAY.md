# Shared terminals (the relay in the Sync server)

The relay is part of the Sync server: one program, one set of accounts. The app needs no relay
address or access token: it finds the relay through your Sync server.

Start the server with a second port for the relay:

    sshtup_sync_server serve --db sync.db --listen 127.0.0.1:8787 --relay-listen 0.0.0.0:7700

Without `--relay-listen` shared terminals are switched off and the app says so.

1. Turn on Sync in the app (Settings > Sync). Everyone who shares or joins needs an account on the same server.
2. Open a terminal, then Collaboration > Share this terminal. A session code appears: `xxxx-xxxx~KEY`.
3. Send the whole code to the other person (it includes the key after the `~`). They open Collaboration, paste it under "Join a shared terminal" and press Join.
4. Turn "Guests can type" on to let guests send keystrokes.

## Network

The app reaches the relay at the **same host name as the Sync server, on the relay port**. If the
Sync server is `https://sync.example.com` behind Caddy, port 7700 of `sync.example.com` must be
open too (the HTTP proxy does not carry it). Behind Tailscale or a VPN nothing else is needed.

## What is protected

- **Terminal content is end-to-end encrypted** (XChaCha20-Poly1305). The key is random per session and lives only in the session code the host hands out. The server and the relay port see sealed bytes, never the terminal.
- **Who may use the relay**: only accounts of this server. The app asks the server (over its normal HTTPS) for a one-time ticket that works once for 60 seconds and shows only that ticket to the relay port, so the account's real token never travels over the relay's plain TCP.
- **Not hidden**: that a session exists, its code, the guests' display names and when traffic flows. Anyone holding the full code and an account on the server can watch (and type, if the host allows it). Share the code like a password.

## Limit on guessing

Failed attempts (a session code that does not exist, or a missing or used ticket) are counted per
address. After 5 failures within 10 minutes the address is blocked for 10 minutes: it gets "Too
many failed attempts" for any code, right or wrong. IPv6 is counted per /64. Behind a proxy or
SSH tunnel every client looks the same, so the limit is shared. Other limits: 8 guests per
session, 256 connections, 256 KB per message.
