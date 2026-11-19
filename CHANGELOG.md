# Changelog

All notable changes to SSHTUP. Versions are the tags of the
[releases](https://github.com/tonihardianto/sshtup/releases).

## 0.4.1

- Teams: the owner can **delete a team** (the bin icon on the team's card in Settings > Sync). Every item in it goes back
  to the owner's Personal vault first. Other members lose the team and its items. The sync server must be updated with
  `deploy/update.sh` first; the app checks this before it moves anything.
- Settings > Sync: each team is a card with its members, their roles and an Add member button of its own.
- Settings: tabs in the order General, Terminal, Security, Sync, About, each with an icon. General opens first.
- Menus: the Move to team and Move to group submenus are compact and list only the names.
- Light theme: the sidebar's text is darker and heavier, so it reads more easily.

## 0.4.0

- Teams: **roles**. Add people as Admin, Editor or Viewer and change it later in Settings > Sync. Viewers can connect to
  a team's hosts but not change or delete its items; admins can add people and set editor/viewer roles; only the owner
  removes people or makes admins. People already in a team become editors. Update the sync server with
  `deploy/update.sh` first. See SYNC.md for what a role does and does not protect.
- Teams: **vault switcher**. The tile at the top of the sidebar now opens a list of All vaults, Personal and each team,
  and hosts, groups, forwards, snippets, keys and identities show only what is in the chosen vault. New items go to
  the chosen team when you can edit it.
- Teams: **Move to team / Move to Personal** in the right-click menu of hosts (also the "..." menu), groups, forwards and
  snippets. Moving a group takes its hosts with it. A move that lets the team read a saved password or key asks first.
- **Move to group** in the menu of hosts and groups: put a host or group into another group, or back to the top level.

## 0.3.0

- **App theme**: Settings > General > Appearance now has Dark, Light and System for the app itself. Terminals keep
  their own color scheme. The Dark palette is a little lighter than before, so panels, cards and the sidebar are easier
  to tell apart; the tab strip stays dark in both themes.
- **Interface font**: choose Inter (the new default), DM Sans, the system font or the classic egui font. Inter and DM
  Sans are bundled (SIL Open Font License, texts in `assets/fonts`).
- **Check for updates** in Settings > About: asks GitHub for the latest release when you press the button and links to
  the download. Nothing is checked on its own.
- Settings > About tab with the version, platform, data folder and links.
- Forms: one design for hosts, keys, identities, port forwards and snippets, with required-field hints, a collapsible
  Advanced section, and only **Save** and **Save & connect** at the bottom. The panel slides in and out smoothly.
- Scrollbars are thin and follow the app colors; the panel resize line no longer flashes on hover.

## 0.2.1

- Dependencies: axum 0.8, rusqlite 0.42, x25519-dalek 3, chacha20poly1305 0.11, base64 0.23 and newer GitHub Actions.
  The sync server routes and the encryption code were adapted to them; nothing changes for users.
- Settings: a new layout with a search box, underlined tabs and cards of settings, each with **Reset to defaults**. The
  Terminal tab has a preview that follows every change (the scheme's 16 colors show their names on hover).
- Terminal: pick the **font** from the monospace fonts installed on the computer (Hack stays built in), set the **line
  height** (1.0, 1.2 or 1.4) and the **cursor** (block, bar or underline; blinking or steady).
- Terminal: **Bell** is now Off, Sound (the system alert) or Flash. A saved "Bell sound" setting becomes Sound.
- Terminal: Ctrl + and Ctrl − (Cmd on a Mac) change the font size, Ctrl 0 sets it back. A host with its own size changes
  for that tab only.
- The sidebar groups Known Hosts, Logs and Collaboration under **More**.
- Icons: the interface now uses Lucide icons (rounder and lighter than the hand-drawn ones). Their licence is in
  `LICENSES/lucide.txt`.
- Hosts: a new top bar with one search box (press Ctrl+K, or Cmd+K on a Mac, to jump to it), a grid/list switch and a
  **New Host** button whose menu holds New Group, Import ~/.ssh/config, Local terminal and Join shared session.
- Shared terminals: a guest's window now shows who is in the session (avatars on the tab and the card) and a badge under
  the cursor with the name of whoever is typing, the host included. Needs the sync server updated
  (`deploy/update.sh`); with an older server guests just see no badge.
- Settings > Terminal > **Session card**: put the floating card in any corner of the terminal, or hide it. A hidden card
  still appears when the connection drops, because it holds the Reconnect button.
- `deploy/update.sh` updates the sync server from a release: checks the checksum, backs up the database, swaps the
  binary, restarts the service and goes back if it does not stay up.
- The floating card is one flat list now (Snippets, the session code with a copy button, one switch per guest for typing,
  Stop sharing, Disconnect) instead of menus inside menus. Folded, it is a round icon in the colour of the connection.
- A guest cannot share the host's terminal onward: "Share this terminal" is gone from a guest's tab, and the app refuses
  it too.
- Hosts: new cards with an avatar (the last number of the IP address, or two letters, in the colour of the group, with
  a green dot while connected), `user@address:port`, and chips for team and tags. SFTP and a menu (Connect, Open SFTP,
  Edit, Duplicate, Pin to top, Delete) show on hover or keyboard focus; right-click gives the same menu. A long user
  name is shortened first, with the full text in a tooltip.
- Hosts: a **Pinned** section at the top (a pinned host is listed once, there).
- SFTP: **Actions > Disconnect** on the host pane closes the connection and goes back to the host list, instead of only
  being able to change host.

## 0.2.0

- Shared terminals: the host can allow typing guest by guest. New guests start view only, and the host drops keystrokes
  from a guest without permission. Avatars of the people in the session on the tab, and a badge under the host's cursor
  with the name of the guest who typed.
- The toolbar row of a terminal (Snippets, Collaboration, Disconnect) is replaced by a small floating card at the top
  right. It is folded to a dot until the pointer is on it, and opens by itself when the connection is lost.
- Fixed: a refusal that arrived in pieces (a short line after a quiet spell) could be taken for a successful login.

## 0.1.1

- The app icon fits macOS: rounded square, margin and shadow.

## 0.1.0

- First release: hosts, groups, keychain, snippets, port forwarding, known hosts and session logs; the SFTP browser;
  command history and autocomplete; the optional master password; Sync through your own server, with teams and shared
  terminals. Packages for macOS, Windows and Linux, and the server for Linux.
