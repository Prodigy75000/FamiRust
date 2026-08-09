#!/usr/bin/env bash
#
# One-liner: build the arm64 FamiRust libretro core from the current working
# tree, drop it into the TrophyHubAndroid debug app's jniLibs, and assemble the
# debug APK (optionally install it to an attached device).
#
#   scripts/deploy-android-debug.sh            # build core -> jniLibs -> assembleDebug
#   scripts/deploy-android-debug.sh install    # ... then adb install -r the APK
#
# Why this exists: the core (FamiRust) and the app (TrophyHubAndroid) are
# separate repos in the TrophyHub umbrella, so getting a core change onto a device
# is a fixed three-step dance. This pins it so no one rediscovers it. Only
# arm64-v8a ships (see the app's abiFilters); the two devices are arm64.
#
# FamiRust owns nes AND fds on Android (Nestopia is retired), so a bad core here
# takes both platforms with it -- run `cargo test` before deploying.
#
# Save states: the state format is versioned (nes_core::STATE_VERSION) and a core
# only accepts states at or below its own version. When a change bumps it, the
# desktop core (TrophyHubDesktop/runtime-deps/cores_windows/nescore_libretro.dll)
# has to be rebuilt in the same pass or the phone and the desktop stop agreeing.
# The version each build reports is its libretro library_version -- read it in
# the app's core-info line, or `strings` the .so.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID="$(cd "$REPO/../TrophyHubAndroid" && pwd)"
TRIPLE="aarch64-linux-android"
ABI="arm64-v8a"
SO="libnescore_libretro.so"
# FamiRust is a standalone workspace (open-source), so its target dir is local --
# not the umbrella's shared TrophyHub/target. The umbrella's .cargo/config.toml
# still supplies the NDK linker + the 16 KB max-page-size link arg.
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"

echo "[1/3] cargo build --release -p nes-libretro --target $TRIPLE"
( cd "$REPO" && cargo build --release -p nes-libretro --target "$TRIPLE" )

SRC="$TARGET_DIR/$TRIPLE/release/$SO"
DST="$ANDROID/app/src/main/jniLibs/$ABI/$SO"
[ -f "$SRC" ] || { echo "ERROR: built core not found at $SRC" >&2; exit 1; }
echo "[2/3] cp core -> $DST"
cp "$SRC" "$DST"
# Keep the checked-in-looking copy under out/release in step with what shipped,
# so "what is on the device" can be answered from this repo alone.
mkdir -p "$REPO/out/release"
cp "$SRC" "$REPO/out/release/libnescore_libretro.android-arm64.so"

echo "[3/3] ./gradlew :app:assembleDebug"
( cd "$ANDROID" && ./gradlew :app:assembleDebug )

APK="$ANDROID/app/build/outputs/apk/debug/app-debug.apk"
echo "APK: $APK"

# The debug APK always lives at this fixed Drive slot, replacing the current one,
# so a phone/tablet can pull it without a cable (Drive for Desktop syncs it up).
DRIVE_SLOT="${TROPHYHUB_DEBUG_APK:-/g/My Drive/Trophy Hub/TrophyHub-debug.apk}"
if [ -d "$(dirname "$DRIVE_SLOT")" ]; then
  cp "$APK" "$DRIVE_SLOT"
  echo "Drive: $DRIVE_SLOT (replaced)"
else
  echo "note: Drive slot dir missing, skipped ($DRIVE_SLOT)"
fi

if [ "${1:-}" = "install" ]; then
  echo "adb install -r (device must be authorized for USB debugging)"
  adb install -r "$APK"
fi
