# Sync server

`sshtup_sync_server` keeps your vault's items in step across your computers and with a small team.
It stores **encrypted blobs only**. Item contents, team keys and the sync password never reach it in the clear.

## What the server can and cannot see

| Sees | Does not see |
|---|---|
| Account emails, device names, when each device last connected | Hostnames, usernames, passwords, keys, snippets, forward rules |
| Item ids like `host:00000000000000ab`, how many items, their sizes, when they changed | Anything inside an item |
| Which accounts are in which team, team names | The team key, any account's sync key |
| A stored hash of a login value derived from your password (not the password) | Your sync password, your recovery key |

## Run it

```bash
cargo build --release -p sync-server
./target/release/sshtup_sync_server invite --db /var/lib/sshtup-sync/sync.db
./target/release/sshtup_sync_server serve  --db /var/lib/sshtup-sync/sync.db --listen 127.0.0.1:8787
```

* `invite` prints a one-time registration code. Nobody can register without one. Run it again for each person.
* The server speaks plain HTTP on purpose. **Put TLS in front of it.** The app refuses `http://` addresses except
  `localhost`, `127.0.0.1`, `::1` or when "allow private http" is ticked.
* The database file is created readable only by its owner.
* Behind a reverse proxy on the same machine (nginx, Caddy), add `--trust-proxy`. Otherwise every user looks like 127.0.0.1 and the failed-login limit is shared by everyone. With it the server reads the client address from the proxy's `X-Forwarded-For` header (last entry), and only for connections that come from this machine. The proxy must **set** the header, in nginx `proxy_set_header X-Forwarded-For $remote_addr;`. Do not use the flag when clients can reach the server port directly.
* Optional: add `--relay-listen 0.0.0.0:7700` to also run the shared-terminal relay on its own port (same accounts, end-to-end encrypted). See [RELAY.md](RELAY.md); the port must be reachable at the same host name as the server.

### TLS with Caddy

```
sync.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

### Or keep it private with Tailscale

Listen on the Tailscale address (`--listen 100.x.y.z:8787`), or run `tailscale serve --https=443 http://127.0.0.1:8787`,
and use `https://<machine>.<tailnet>.ts.net` in the app. The server is then unreachable from the public internet.

## First sync

1. In the app: Settings → Security → set a master password (sync keeps its keys inside the encrypted vault).
2. Settings → Sync → server address, your email, a **sync password** (8+ characters, different from the master
   password) and the invite code → *Create account*.
3. Write down the **recovery key** that is shown once. It is the only way back in if you forget the sync password;
   the server cannot give it back.
4. On the next computer: same server, same email and password → *Sign in*.

Before the first sync the app saves a copy of `vault.json` next to it and gives every item a random id.

## Teams

* Create a team in Settings → Sync. Add someone who already has an account on the same server: type their email and press
  *Look up*. The app shows **their fingerprint** (8 groups of 5 digits). Ask them to read you the number shown in their own
  Settings → Sync. Only if it matches, press *add to the team*. This is what stops the server from swapping in its own
  key. There is no accept step: the person is a member from that moment and gets the key on their next sync round.
* Every member has a **role**, chosen when you add them and changeable later in the member list:

  | Role | Can read and connect | Can add and change items | Can add people and set roles | Can remove people |
  |---|---|---|---|---|
  | Owner | yes | yes | yes | yes |
  | Admin | yes | yes | yes (editors and viewers only) | no |
  | Editor | yes | yes | no | no |
  | Viewer | yes | no | no | no |

  Only the owner makes admins. People added before roles existed are editors. A viewer's computer only receives;
  the server also refuses anything a viewer sends. **A viewer still holds the team key**, so they can open every item,
  saved passwords included: a role limits who can *change* things, not who can *read* them. Keep secrets a person
  should not see in a team they are not in. Update the server (`deploy/update.sh`) before using roles; an older server
  treats everyone but the owner as an editor.
* In the form of a new item, **Who can see this** picks *Only me* or a team before you save. A new host or sub-group
  created inside a shared group starts on that group's team.
* In an item's editor, **Who can see this** moves it between *Only me* and a team. Moving a host with a saved password,
  an identity or a key with a passphrase asks for a confirmation first.
* A host in a team that points at a personal identity or key shows up for the others with that reference empty. Move
  the identity or key too.
* Removing a member replaces the team key and re-seals every shared item. Do it from the owner's computer; if it is
  interrupted, the next sync of any of the owner's devices finishes it. Only one removal at a time.

## Backups

```bash
./target/release/sshtup_sync_server backup --db /var/lib/sshtup-sync/sync.db --out /backups/sync-2026-11-16.db
```

It is safe to run while the server is serving and refuses to overwrite a file. The copy is the whole server: restore it
with `serve --db <copy>`. Back up the file off the machine; everything in it is encrypted or hashed, but it is
the only copy of your team structure and the sealed keys.

## Updating the server

On the machine that runs the server, put [`deploy/update.sh`](deploy/update.sh) next to `sshtup_sync_server` and run it:

```bash
./update.sh            # the latest release
./update.sh v0.2.0     # a specific one, which is also how you go back
```

It downloads the server tarball, checks it against the release's `SHA256SUMS.txt`, backs up the database, swaps the
binary, restarts the systemd unit (`SERVICE`, default `sshtup-sync`) and waits to see that it stays up. If it does not,
the old binary is put back. Nothing is changed before the checksum passes. `DIR`, `DB`, `BACKUPS` and `KEEP` can be set
as environment variables; the header of the script lists them.

## Limits you should know about

* **Metadata is visible** to whoever runs the server (see the table).
* **The server can roll back or withhold** data: it can serve an older version of an item or hide new ones. It cannot
  forge content, because every blob is authenticated, but a malicious server can make you miss an update.
* **The team key is handed over without proof of who sent it.** The fingerprint check protects the new member's public key
  (the server cannot swap it), but a member does not verify the owner's. A server operator who actively attacks could
  seal a key of their own to a joining member (or at a rotation) and then read what that member writes into the team.
  Run the server yourself, or only use teams on a server you trust not to do this.
* **Team members can add hosts, snippets and port forwards that appear on your computer.** Treat a team like shared
  shell access: read what you run. A team item can never overwrite one of your personal items; it waits until your own
  device says it moved.
* **Someone you remove still knows what they saw.** Passwords and keys they could read stay known to them. Change those
  secrets by hand.
* **Last write wins**, per item, by the time on the device that made the change. A device whose clock is far ahead is
  clamped to the server's time, but small skews can pick the "wrong" winner. The version that loses is kept, encrypted,
  on the computer where it lost (Settings → Sync shows how many).
* **Key files** are not synced unless you tick *Also sync this key file…* on that key. `known_hosts` is per device.
* **Forgotten sync password and lost recovery key** means the synced data cannot be opened by anyone. Your local
  vault is untouched.
* Item ids carry no secrets, but deleted items leave a small tombstone on the server forever.
* If an item in a team cannot be decrypted by its owner any more (corruption), it blocks removing the next member until it
  is deleted or replaced.

## Shared groups

Sharing a group with a team moves the group, its sub-groups, their hosts and the keys and identities those hosts use in one step
(one confirmation when saved passwords or passphrases would become readable). Setting the group back to "Only me" moves them all back.
A host or sub-group created inside a shared group joins the same team. Moving an existing host into a shared group by editing it
does not move it; share it from its own editor.
