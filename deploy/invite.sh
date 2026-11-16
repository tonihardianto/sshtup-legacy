#!/bin/sh
# Prints a one-time registration code for the sync server. Give the code to one person.
# Usage: invite.sh [path/to/sync.db]     (default: sync.db next to this script)
# The server binary is expected next to this script too.
dir=$(cd "$(dirname "$0")" && pwd)
exec "$dir/sshtup_sync_server" invite --db "${1:-$dir/sync.db}"
