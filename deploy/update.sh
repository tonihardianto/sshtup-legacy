#!/bin/sh
# Updates the sync server and relay on a Linux machine to a release from GitHub.
#
#   ./update.sh              the latest release
#   ./update.sh v0.2.0       a specific one (also how you go back)
#
# What it does, in order: downloads the server tarball, checks it against the release's SHA256SUMS.txt, makes a backup
# of the database, swaps the binary, restarts the service and waits to see that it stays up. If it does not, the old
# binary is put back and the service is started again with it. Nothing is changed before the checksum passes.
#
# Settings (environment variables, all optional):
#   SERVICE   systemd unit to restart            (default: sshtup-sync)
#   DIR       folder with sshtup_sync_server     (default: the folder this script is in)
#   DB        database to back up                (default: $DIR/sync.db)
#   REPO      GitHub repository                  (default: tonihardianto/sshtup)
#   BACKUPS   where the backups go               (default: $DIR/backups)
#   KEEP      how many backups to keep           (default: 10)
set -eu

SERVICE=${SERVICE:-sshtup-sync}
DIR=${DIR:-$(cd "$(dirname "$0")" && pwd)}
DB=${DB:-$DIR/sync.db}
REPO=${REPO:-tonihardianto/sshtup}
BACKUPS=${BACKUPS:-$DIR/backups}
KEEP=${KEEP:-10}
ASSET=sshtup-sync-server-linux-x86_64.tar.gz
BIN=$DIR/sshtup_sync_server

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# systemctl needs root for restarts; use sudo when we are not.
if [ "$(id -u)" -eq 0 ]; then SUDO=; else SUDO=sudo; fi

[ "$(uname -s)" = Linux ] || die "this script is for Linux (the server build is linux-x86_64)"
case $(uname -m) in x86_64 | amd64) ;; *) die "the release only has an x86_64 server build; this machine is $(uname -m)" ;; esac
for tool in curl tar sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done
[ -x "$BIN" ] || die "$BIN not found; set DIR to the folder that holds sshtup_sync_server"

# Which release.
if [ $# -ge 1 ]; then
  tag=$1
else
  tag=$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") || die "could not reach GitHub"
  tag=${tag##*/}
fi
case $tag in v[0-9]*) ;; *) die "'$tag' is not a release tag like v0.2.0" ;; esac

say "Updating $BIN to $tag"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

# Download and check before touching anything.
base=https://github.com/$REPO/releases/download/$tag
say "Downloading $ASSET ..."
curl -fsSL -o "$work/$ASSET" "$base/$ASSET" || die "could not download $base/$ASSET (does the release exist?)"
curl -fsSL -o "$work/SHA256SUMS.txt" "$base/SHA256SUMS.txt" || die "could not download SHA256SUMS.txt"
want=$(awk -v f="$ASSET" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS.txt")
[ -n "$want" ] || die "SHA256SUMS.txt has no line for $ASSET"
got=$(sha256sum "$work/$ASSET" | awk '{ print $1 }')
[ "$want" = "$got" ] || die "checksum does not match (wanted $want, got $got); nothing was changed"
say "Checksum ok."

mkdir "$work/new"
tar -xzf "$work/$ASSET" -C "$work/new"
[ -f "$work/new/sshtup_sync_server" ] || die "the tarball has no sshtup_sync_server"
chmod 755 "$work/new/sshtup_sync_server"

# Back up the database while the old server is still running (the command is safe on a live database).
if [ -f "$DB" ]; then
  mkdir -p "$BACKUPS"
  out=$BACKUPS/sync-$(date +%Y%m%d-%H%M%S).db
  "$BIN" backup --db "$DB" --out "$out" || die "the backup failed; nothing was changed"
  say "Backup:  $out"
  # Keep the newest $KEEP.
  ls -1t "$BACKUPS"/sync-*.db 2>/dev/null | tail -n +"$((KEEP + 1))" | while read -r old; do rm -f "$old"; done
else
  say "No database at $DB; skipping the backup."
fi

# Swap. The old binary stays next to the new one as sshtup_sync_server.prev.
cp -p "$BIN" "$BIN.prev"
mv "$work/new/sshtup_sync_server" "$BIN.new"
mv "$BIN.new" "$BIN"
say "Restarting $SERVICE ..."
$SUDO systemctl restart "$SERVICE"

# The service has to still be running a few seconds later (a crash on start shows up as 'activating' or 'failed').
sleep 4
if $SUDO systemctl is-active --quiet "$SERVICE"; then
  say "Done. $SERVICE is running $tag."
  say "The previous binary is kept as $BIN.prev; to go back run: ./update.sh <old tag>"
  exit 0
fi

say "The service did not stay up. Putting the old binary back ..." >&2
mv "$BIN.prev" "$BIN"
$SUDO systemctl restart "$SERVICE" || true
sleep 2
if $SUDO systemctl is-active --quiet "$SERVICE"; then
  die "$tag failed to start; the previous version is running again. Check: journalctl -u $SERVICE -n 50"
fi
die "$tag failed to start and the previous version did not start either. Check: journalctl -u $SERVICE -n 50"
