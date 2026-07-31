#!/usr/bin/env bash
# Register the openWarp custom merge driver + enable rerere.
# Run once after the first clone; subsequent upstream merges (merge / cherry-pick / rebase) will then:
# 1. automatically keep the local version for paths marked merge=openwarp-ours in .gitattributes
# 2. have rerere record each conflict resolution, reusing it automatically for the same conflict next time
set -euo pipefail

# The driver name MUST match the `merge=` value used in .gitattributes (openwarp-ours).
# It previously registered `zap-ours`, which no .gitattributes rule references, so every
# fork-owned keep-ours protection silently did nothing on a fresh clone.
git config merge.openwarp-ours.name "Always keep openWarp version (custom driver)"
git config merge.openwarp-ours.driver true
git config rerere.enabled true
# Deliberately false: rerere replays a cached resolution by *staging* it when autoupdate is on,
# so a resolution recorded under an older wave's policy can land in a merge commit unreviewed.
# Keep replays unstaged so each one is inspected (and `git rerere forget <file>` if wrong).
git config rerere.autoupdate false

echo "openWarp merge drivers + rerere configured."
echo "  rerere.enabled          = $(git config --get rerere.enabled)"
echo "  rerere.autoupdate       = $(git config --get rerere.autoupdate)"
echo "  merge.openwarp-ours     = $(git config --get merge.openwarp-ours.driver)"
