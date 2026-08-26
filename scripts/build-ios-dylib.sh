#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
#
# Build the FamiRust NES/FDS libretro core for iOS and drop the dylib into the
# sibling TrophyHubIOS checkout at iosApp/cores/, where the "Embed libretro
# cores" build phase copies and signs it into the app bundle and the host
# dlopen-loads it at runtime.
#
#   scripts/build-ios-dylib.sh                # simulator (arm64), the default
#   scripts/build-ios-dylib.sh device         # a real phone
#   scripts/build-ios-dylib.sh both           # one after the other
#
# THIS ONLY RUNS ON macOS. It needs Xcode's SDKs and codesigning tools, which
# have no Windows or Linux equivalent, so it is the one FamiRust target that
# cannot be produced from the machine the rest are built on. That is the whole
# reason it is a separate script from scripts/deploy-android-debug.sh.
#
# Unlike every other core in TrophyHubIOS/scripts, this one has no fork to sync
# and no Makefile to coax: the core is Rust in this repository and the entire
# build is one cargo invocation per target. Naming and the output directory
# follow the convention the other scripts established, so the Xcode side needs
# no special case.
#
# Save-state parity: the state format is versioned (nes_core::STATE_VERSION) and
# is byte-identical across targets by construction, so a state made on the phone
# loads on Android and the desktop AS LONG AS all three are built from the same
# commit. Ship this in the same pass as the other two whenever the core changes.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS="$(cd "$REPO/../TrophyHubIOS" 2>/dev/null && pwd || true)"
OUT_NAME="famirust_libretro_ios.dylib"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "ERROR: iOS builds need macOS and Xcode; this is $(uname -s)." >&2
    echo "       Clone https://github.com/Prodigy75000/FamiRust on the Mac and" >&2
    echo "       run this script there." >&2
    exit 1
fi
if [ -z "$IOS" ]; then
    echo "ERROR: no TrophyHubIOS checkout beside this repo at $REPO/../TrophyHubIOS" >&2
    exit 1
fi

build_one() {
    local kind="$1" triple min
    case "$kind" in
        simulator) triple=aarch64-apple-ios-sim; min="IPHONEOS_DEPLOYMENT_TARGET=16.0" ;;
        device)    triple=aarch64-apple-ios;     min="IPHONEOS_DEPLOYMENT_TARGET=16.0" ;;
        *) echo "usage: $0 [simulator|device|both]" >&2; exit 2 ;;
    esac

    rustup target add "$triple" >/dev/null 2>&1 || true
    echo "==> cargo build --release -p nes-libretro --target $triple"
    ( cd "$REPO" && env "$min" cargo build --release -p nes-libretro --target "$triple" )

    local src="$REPO/target/$triple/release/libnescore_libretro.dylib"
    [ -f "$src" ] || { echo "ERROR: no dylib at $src" >&2; exit 1; }

    # An embedded dylib has to be found relative to the app bundle at load time,
    # so its own recorded name must be an @rpath one. Cargo does not set that.
    install_name_tool -id "@rpath/$OUT_NAME" "$src"

    local out="$IOS/iosApp/cores"
    mkdir -p "$out"
    cp "$src" "$out/$OUT_NAME"
    echo "==> Installed: $out/$OUT_NAME  ($kind)"
    vtool -show "$out/$OUT_NAME" | grep -iE 'platform|minos' || true
    strings -a "$out/$OUT_NAME" | grep -E '^0\.[0-9]+\.[0-9]+$' | head -1 || true
}

case "${1:-simulator}" in
    both) build_one simulator; build_one device ;;
    *)    build_one "${1:-simulator}" ;;
esac

cat <<'EOF'

Next, on the Mac:
  * add the dylib to the iosApp target's "Embed libretro cores" phase if it is
    not already listed there;
  * point the NES (and FDS) platform entry at famirust_libretro_ios.dylib.

Note that the simulator and device builds write to the SAME filename, so
whichever ran last is what is sitting in iosApp/cores. Build the one you are
about to run, or keep them in separate checkouts.
EOF
