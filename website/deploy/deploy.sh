#!/usr/bin/env bash
# Zap website deploy script, executed on the server.
#
# Scenario: the site is managed by 1Panel and nginx root points directly at the site directory, for example
#   /opt/1panel/www/sites/zap.zerx.dev/index, so changing root to a symlink is not available.
# This script uses atomic directory replacement: CI rsyncs the latest output to <site>/.incoming,
# then this script renames .incoming to the live index directory and keeps the previous version for rollback.
#
# Expected layout ($DEPLOY_PATH = site index directory, for example .../zap.zerx.dev/index):
#   <parent>/.incoming   <- latest dist content uploaded by CI via rsync
#   <parent>/index       <- directory served by nginx (= $DEPLOY_PATH)
#   <parent>/.index.bak  <- previous version, used for rollback
set -euo pipefail

# The caller passes $DEPLOY_PATH as the first argument over SSH; it is the live index directory.
INDEX="${1:?Usage: deploy.sh <site-index-dir>}"
PARENT="$(dirname "$INDEX")"
INCOMING="$PARENT/.incoming"
BACKUP="$PARENT/.index.bak"

log() { printf '[deploy] %s\n' "$*"; }

if [ ! -d "$INCOMING" ] || [ -z "$(ls -A "$INCOMING" 2>/dev/null)" ]; then
  log "Error: $INCOMING does not exist or is empty; no deployable output found. CI must rsync to .incoming first."
  exit 1
fi

# Delete the previous backup and move the current index to backup if it exists.
if [ -e "$INDEX" ]; then
  log "Back up current version -> $BACKUP"
  rm -rf "$BACKUP"
  mv -Tf "$INDEX" "$BACKUP"
fi

# Atomic publish: rename .incoming -> index. On the same filesystem, mv is atomic.
log "Publish new version -> $INDEX"
mv -Tf "$INCOMING" "$INDEX"

log "Done. Current served directory: $INDEX"
log "Rollback if needed: rm -rf '$INDEX' && mv -Tf '$BACKUP' '$INDEX'"
