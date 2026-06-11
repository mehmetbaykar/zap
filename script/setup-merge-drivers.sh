#!/usr/bin/env bash
# Register the openWarp custom merge driver + enable rerere.
# Run once after the first clone; subsequent upstream merges (merge / cherry-pick / rebase) will then:
# 1. automatically keep the local version for paths marked merge=zap-ours in .gitattributes
# 2. have rerere record each conflict resolution, reusing it automatically for the same conflict next time
set -euo pipefail

git config merge.zap-ours.name "Always keep openWarp version (custom driver)"
git config merge.zap-ours.driver true
git config rerere.enabled true
git config rerere.autoupdate true

echo "openWarp merge drivers + rerere configured."
echo "  rerere.enabled        = $(git config --get rerere.enabled)"
echo "  rerere.autoupdate     = $(git config --get rerere.autoupdate)"
echo "  merge.zap-ours   = $(git config --get merge.zap-ours.driver)"
