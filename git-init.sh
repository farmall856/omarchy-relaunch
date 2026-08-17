#!/usr/bin/env bash
# One-time helper: initialise the git repo and make the first commit.
# Prints the exact commands to create the GitHub repo and push.
#
# Usage:
#   ./git-init.sh                 # init + commit, print push instructions
#   ./git-init.sh <github-user>   # also print a ready-to-run gh command
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_DIR"

GH_USER="${1:-laytonf}"
REPO_NAME="omarchy-relaunch"

if [[ -d .git ]]; then
  echo "git already initialised here."
else
  git init -b main
  git add .
  git commit -m "Relaunch: initial Omarchy Quattro bar-widget plugin

Save the current app->workspace layout and relaunch apps into the right
workspaces after a reboot or crash. QML bar widget + Go engine that
generates a Hyprland windowrule/exec-once snippet."
  echo "initial commit created."
fi

cat <<EOF

Next — create the GitHub repo and push:

  # with the GitHub CLI (easiest):
  gh repo create $GH_USER/$REPO_NAME --public --source=. --remote=origin --push

  # or manually, after creating an empty repo named "$REPO_NAME" on GitHub:
  git remote add origin git@github.com:$GH_USER/$REPO_NAME.git
  git push -u origin main

Then validate before submitting:

  # Copy the QML side into a plugin dir and validate against the shell:
  ./install.sh
  omarchy plugin validate "\$HOME/.config/omarchy/plugins/io.github.laytonf.relaunch"
  qmllint -I "\$OMARCHY_PATH/shell" BarWidget.qml Panel.qml

Finally, submit at:
  https://github.com/HANCORE-linux/omarchy-plugin-marketplace/issues/new?template=submit-plugin.yml
EOF
