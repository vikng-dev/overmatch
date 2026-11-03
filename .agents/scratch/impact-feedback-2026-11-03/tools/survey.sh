#!/bin/bash
# Dump the map's masks and heights to raw arrays, then run the site survey.
B=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2
S=/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/impact-stills
M=$B/assets/maps/kalinovo
magick "$M/terrain_masks.png" -depth 8 "rgb:$S/masks.rgb"
magick "$M/terrain_height.png" -filter point -resize 1024x1024 -endian MSB -depth 16 "gray:$S/height.gray16"
python3 -I "$S/survey.py" "$S" "$M/level.json"
