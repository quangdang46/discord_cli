#!/usr/bin/env bash
# =============================================================================
# clone-all.sh  (in .tmp)
#
# Clones every discord-cli repo from the research list into .tmp/, each into
# its own <owner>-<repo>/ folder. Prints the ".git" URL for each so you can
# re-clone / fork from the link.
#
# Usage:
#   ./clone-all.sh                    # clone all repos
#   ./clone-all.sh --dry-run          # print URLs only, no cloning
# =============================================================================
set -euo pipefail
BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.tmp"
DRY="${1:-}"
mkdir -p "$BASE_DIR"

# owner:repo:url  (url uses .git suffix)
REPOS=(
  "jackwener:discord-cli:https://github.com/jackwener/discord-cli.git"
  "fourjr:discord-cli:https://github.com/fourjr/discord-cli.git"
  "Escape-Technologies:discord-cli:https://github.com/Escape-Technologies/discord-cli.git"
  "RickvanLoo:discord-cli:https://github.com/RickvanLoo/discord-cli.git"
  "Rivalo:discord-cli:https://github.com/Rivalo/discord-cli.git"
  "famasya:discord-cli-agent:https://github.com/famasya/discord-cli-agent.git"
  "mrarfarf:discord-cli:https://github.com/mrarfarf/discord-cli.git"
  "langkurt:discord-cli:https://github.com/langkurt/discord-cli.git"
  "virat-mankali:discord-cli:https://github.com/virat-mankali/discord-cli.git"
  "ibbybuilds:discli:https://github.com/ibbybuilds/discli.git"
  "ThePolishCat:discord-cli:https://github.com/ThePolishCat/discord-cli.git"
  "Stone-Red-Code:DiscordCLI:https://github.com/Stone-Red-Code/DiscordCLI.git"
  "ayn2op:discordo:https://github.com/ayn2op/discordo.git"
  "sinjs:clicord:https://github.com/sinjs/clicord.git"
)

count=0
for entry in "${REPOS[@]}"; do
  IFS=: read -r owner repo url <<< "$entry"
  dir="$owner-$repo"
  if [ "$DRY" = "--dry-run" ]; then
    echo "  $dir  <-  $url"
    continue
  fi
  if [ -d "$BASE_DIR/$dir/.git" ]; then
    echo "[skip] $dir ($url)"
  else
    echo "[clone] $dir <- $url"
    if git clone --quiet "$url" "$BASE_DIR/$dir"; then
      count=$((count + 1))
    else
      echo "  [FAIL] $url"
    fi
  fi
done
echo
echo "Done. $count repo(s) cloned into: $BASE_DIR"
