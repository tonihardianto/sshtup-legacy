# Changelog

All notable changes to SSHTUP. Versions are the tags of the
[releases](https://github.com/tonihardianto/sshtup/releases).

## Unreleased

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
- Hosts: a **Pinned** section at the top (a pinned host is listed once, there) and All / Connected / Has tags filters.
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
