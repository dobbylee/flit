#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && /bin/pwd -P)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/.." && /bin/pwd -P)"
TARGET_ROOT="$REPOSITORY_ROOT/target"
OUTPUT_ROOT="$TARGET_ROOT/flit-macos"

if [[ -L "$TARGET_ROOT" ]]; then
    echo "Repository target directory must not be a symbolic link" >&2
    exit 2
fi
/bin/mkdir -p "$TARGET_ROOT"
if [[ "$(cd "$TARGET_ROOT" && /bin/pwd -P)" != "$TARGET_ROOT" ]]; then
    echo "Repository target directory escaped its canonical path" >&2
    exit 2
fi
export CARGO_TARGET_DIR="$TARGET_ROOT"

BINDING_DIR="$OUTPUT_ROOT/generated"
BUILD_DIR="$OUTPUT_ROOT/build"
MODULE_CACHE="$OUTPUT_ROOT/module-cache"
APP_DIR="$OUTPUT_ROOT/Flit.app"
APP_EXECUTABLE="$APP_DIR/Contents/MacOS/Flit"
HOST_DYLIB="$REPOSITORY_ROOT/target/release/libflit_bridge.dylib"
BINDGEN="$REPOSITORY_ROOT/target/release/flit-bindgen"

/bin/rm -rf "$OUTPUT_ROOT"
/bin/mkdir -p "$BINDING_DIR" "$BUILD_DIR" "$MODULE_CACHE" "$APP_DIR/Contents/MacOS"

cd "$REPOSITORY_ROOT"
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --locked --release \
    -p flit-bridge -p flit-bindgen
"$BINDGEN" "$HOST_DYLIB" "$BINDING_DIR"

SWIFT_SOURCES=("$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/"*.swift)
RUST_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
SWIFT_ARCHITECTURES=(arm64 x86_64)

for index in 0 1; do
    rust_target="${RUST_TARGETS[$index]}"
    swift_arch="${SWIFT_ARCHITECTURES[$index]}"
    static_library="$REPOSITORY_ROOT/target/$rust_target/release/libflit_bridge.a"
    executable="$BUILD_DIR/Flit-$swift_arch"

    MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --locked --release \
        --target "$rust_target" -p flit-bridge

    /usr/bin/swiftc \
        -O \
        -whole-module-optimization \
        -swift-version 6 \
        -strict-concurrency=complete \
        -warnings-as-errors \
        -target "$swift_arch-apple-macosx14.0" \
        -module-cache-path "$MODULE_CACHE/$swift_arch" \
        -I "$BINDING_DIR" \
        -Xcc "-fmodule-map-file=$BINDING_DIR/FlitBridgeFFI.modulemap" \
        "$BINDING_DIR/FlitBridge.swift" \
        "$BINDING_DIR/FlitProtocol.swift" \
        "${SWIFT_SOURCES[@]}" \
        "$static_library" \
        -framework AppKit \
        -framework SwiftUI \
        -o "$executable"
done

/usr/bin/lipo -create \
    "$BUILD_DIR/Flit-arm64" \
    "$BUILD_DIR/Flit-x86_64" \
    -output "$APP_EXECUTABLE"
/bin/cp "$REPOSITORY_ROOT/apps/macos/Resources/Info.plist" "$APP_DIR/Contents/Info.plist"
/usr/bin/plutil -lint "$APP_DIR/Contents/Info.plist"
/usr/bin/codesign --force --sign - --timestamp=none "$APP_DIR"

architectures="$(/usr/bin/lipo -archs "$APP_EXECUTABLE")"
if [[ "$architectures" != *"arm64"* || "$architectures" != *"x86_64"* ]]; then
    echo "Flit executable is not universal: $architectures" >&2
    exit 1
fi
if /usr/bin/otool -L "$APP_EXECUTABLE" | /usr/bin/grep -q 'libflit_bridge'; then
    echo "Flit must statically link the Rust bridge" >&2
    exit 1
fi

echo "Built $APP_DIR ($architectures)"
