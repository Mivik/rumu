#!/bin/sh
# Build a static rumu for Harbor task containers (any Linux distribution)
# into a directory to mount at /opt/rumu: the binary and its launcher.
# Needs `cargo zigbuild` and the x86_64-unknown-linux-musl target.
#
#   scripts/harbor/build.sh [out-dir]   (default: target/harbor)
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/target/harbor}"
target=x86_64-unknown-linux-musl
cd "$root"
cargo zigbuild --release --target "$target" -p rumu-server
mkdir -p "$out"
cp "target/$target/release/rumu" "$out/rumu"
cp scripts/harbor/launch.sh "$out/launch.sh"
chmod +x "$out/rumu" "$out/launch.sh"
echo "built $out; add $out/home/settings.yaml before running Harbor"
