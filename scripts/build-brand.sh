#!/bin/sh
# Rebuilds the rasterised brand assets from their SVG sources in assets/brand:
# the app icon (Clicky.icns, bundled by `cargo xtask bundle`), its 1024 px PNG
# and the web touch icon. The outputs are committed; run this after editing
# an SVG. Needs only the Xcode command line tools (swiftc) and iconutil.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
brand="$root/assets/brand"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

swiftc -O "$root/scripts/render-svg.swift" -o "$tmp/render-svg"
render() { "$tmp/render-svg" "$1" "$2" "$3"; }

# 16 and 32 px use the simplified artwork; the rest use the master.
iconset="$tmp/Clicky.iconset"
mkdir "$iconset"
for spec in 16:16x16:small 32:16x16@2x:small 32:32x32:small 64:32x32@2x:master \
    128:128x128:master 256:128x128@2x:master 256:256x256:master \
    512:256x256@2x:master 512:512x512:master 1024:512x512@2x:master; do
    pixels=${spec%%:*}
    rest=${spec#*:}
    name=${rest%%:*}
    case ${rest#*:} in
        small) source="$brand/app-icon/app-icon-small.svg" ;;
        *) source="$brand/app-icon/app-icon.svg" ;;
    esac
    render "$source" "$iconset/icon_$name.png" "$pixels"
done
iconutil -c icns -o "$brand/app-icon/Clicky.icns" "$iconset"

render "$brand/app-icon/app-icon.svg" "$brand/app-icon/app-icon-1024.png" 1024
render "$brand/web/apple-touch-icon.svg" "$brand/web/apple-touch-icon.png" 180
echo "brand assets rebuilt in $brand"
