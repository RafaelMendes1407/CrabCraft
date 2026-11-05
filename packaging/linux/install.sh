#!/bin/sh
# Installs CrabCraft for the current user: the program, its icon and its menu entry.
set -eu
cd "$(dirname "$0")"
data=${XDG_DATA_HOME:-"$HOME/.local/share"}
bin="$HOME/.local/bin"
install -Dm755 crabcraft "$bin/crabcraft"
install -Dm644 crabcraft.png "$data/icons/hicolor/256x256/apps/crabcraft.png"
install -Dm644 crabcraft.desktop "$data/applications/crabcraft.desktop"
legal="$data/crabcraft"
install -Dm644 LICENSE-MIT "$legal/LICENSE-MIT"
install -Dm644 LICENSE-APACHE "$legal/LICENSE-APACHE"
install -Dm644 THIRD_PARTY_NOTICES.md "$legal/THIRD_PARTY_NOTICES.md"
mkdir -p "$legal/assets"
cp -R assets/fonts assets/icons "$legal/assets/"
update-desktop-database "$data/applications" 2>/dev/null || true
echo "CrabCraft is installed in $bin"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Add $bin to your PATH to start it with the crabcraft command." ;;
esac
