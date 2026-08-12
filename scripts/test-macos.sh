#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && /bin/pwd -P)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/.." && /bin/pwd -P)"
TARGET_ROOT="$REPOSITORY_ROOT/target"
OUTPUT_ROOT="$TARGET_ROOT/flit-macos"
GENERATION_A="$OUTPUT_ROOT/test-generation-a"
GENERATION_B="$OUTPUT_ROOT/test-generation-b"
TEST_MODULE_CACHE="$OUTPUT_ROOT/test-module-cache"
TEST_EXECUTABLE="$OUTPUT_ROOT/native-health-tests"
HOST_DYLIB="$REPOSITORY_ROOT/target/release/libflit_bridge.dylib"
BINDGEN="$REPOSITORY_ROOT/target/release/flit-bindgen"
APP_DIR="$OUTPUT_ROOT/Flit.app"
APP_GIT_RUNNER="$APP_DIR/Contents/Helpers/flit-git-noexec"
PACKAGED_GIT_TEST_APP="$OUTPUT_ROOT/git-observation-harness/Flit.app"
PACKAGED_GIT_TEST_EXECUTABLE="$PACKAGED_GIT_TEST_APP/Contents/MacOS/Flit"
PACKAGED_GIT_TEST_RUNNER="$PACKAGED_GIT_TEST_APP/Contents/Helpers/flit-git-noexec"
PACKAGED_GIT_TEST_MODULE_CACHE="$OUTPUT_ROOT/git-observation-harness/module-cache"

if [[ -L "$TARGET_ROOT" ]]; then
    echo "Repository target directory must not be a symbolic link" >&2
    exit 2
fi
/bin/mkdir -p "$TARGET_ROOT"
if [[ "$(cd "$TARGET_ROOT" && /bin/pwd -P)" != "$TARGET_ROOT" ]]; then
    echo "Repository target directory escaped its canonical path" >&2
    exit 2
fi

PATH_SAFETY_ROOT="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/flit-path-safety.XXXXXX")"
PACKAGED_GIT_TEST_ROOT="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/flit-packaged-git.XXXXXX")"
cleanup_path_safety() {
    /bin/rm -rf "$PATH_SAFETY_ROOT"
    /bin/rm -rf "$PACKAGED_GIT_TEST_ROOT"
}
trap cleanup_path_safety EXIT
/usr/bin/touch "$PATH_SAFETY_ROOT/sentinel"
/bin/rm -rf "$OUTPUT_ROOT"
/bin/ln -s "$PATH_SAFETY_ROOT" "$OUTPUT_ROOT"
FLIT_MACOS_BUILD_DIR="$PATH_SAFETY_ROOT/traversal-escape" "$SCRIPT_DIR/build-macos.sh"
if [[ ! -f "$PATH_SAFETY_ROOT/sentinel" || -e "$PATH_SAFETY_ROOT/traversal-escape" ]]; then
    echo "Native build output escaped the repository target boundary" >&2
    exit 1
fi
if [[ -L "$OUTPUT_ROOT" ]]; then
    echo "Native build retained an unsafe output symlink" >&2
    exit 1
fi

if [[ -L "$APP_GIT_RUNNER" || ! -f "$APP_GIT_RUNNER" || ! -x "$APP_GIT_RUNNER" ]]; then
    echo "Native build did not retain one exact executable Git runner" >&2
    exit 1
fi
runner_architectures="$(/usr/bin/lipo -archs "$APP_GIT_RUNNER")"
if [[ "$runner_architectures" != *"arm64"* || "$runner_architectures" != *"x86_64"* ]]; then
    echo "Native build Git runner is not universal: $runner_architectures" >&2
    exit 1
fi
/usr/bin/codesign --verify --strict "$APP_GIT_RUNNER"
/usr/bin/codesign --verify --strict --deep "$APP_DIR"
set +e
runner_failure="$($APP_GIT_RUNNER 2>&1)"
runner_status=$?
set -e
if [[ "$runner_status" -ne 120 || "$runner_failure" != "flit-git-noexec:v1:invalid-arguments" ]]; then
    echo "Packaged Git runner did not expose the exact static protocol failure" >&2
    exit 1
fi

/bin/rm -rf "$GENERATION_A" "$GENERATION_B" "$TEST_MODULE_CACHE" "$TEST_EXECUTABLE"
/bin/mkdir -p "$GENERATION_A" "$GENERATION_B" "$TEST_MODULE_CACHE"
"$BINDGEN" "$HOST_DYLIB" "$GENERATION_A"
"$BINDGEN" "$HOST_DYLIB" "$GENERATION_B"

for generated_file in \
    FlitBridge.swift \
    FlitBridgeFFI.h \
    FlitBridgeFFI.modulemap \
    FlitProtocol.swift; do
    /usr/bin/cmp "$GENERATION_A/$generated_file" "$GENERATION_B/$generated_file"
done

case "$(/usr/bin/uname -m)" in
    arm64) host_target="aarch64-apple-darwin" ;;
    x86_64) host_target="x86_64-apple-darwin" ;;
    *)
        echo "Unsupported macOS architecture: $(/usr/bin/uname -m)" >&2
        exit 1
        ;;
esac

/bin/mkdir -p \
    "$PACKAGED_GIT_TEST_APP/Contents/MacOS" \
    "$PACKAGED_GIT_TEST_APP/Contents/Helpers" \
    "$PACKAGED_GIT_TEST_MODULE_CACHE"
/bin/cp "$APP_GIT_RUNNER" "$PACKAGED_GIT_TEST_RUNNER"
/bin/cp "$REPOSITORY_ROOT/apps/macos/Resources/Info.plist" \
    "$PACKAGED_GIT_TEST_APP/Contents/Info.plist"
/usr/bin/swiftc \
    -O \
    -whole-module-optimization \
    -swift-version 6 \
    -strict-concurrency=complete \
    -warnings-as-errors \
    -module-cache-path "$PACKAGED_GIT_TEST_MODULE_CACHE" \
    -I "$GENERATION_A" \
    -Xcc "-fmodule-map-file=$GENERATION_A/FlitBridgeFFI.modulemap" \
    "$GENERATION_A/FlitBridge.swift" \
    "$GENERATION_A/FlitProtocol.swift" \
    "$REPOSITORY_ROOT/apps/macos/Tests/NativeGitObservationTests.swift" \
    "$REPOSITORY_ROOT/target/$host_target/release/libflit_bridge.a" \
    -o "$PACKAGED_GIT_TEST_EXECUTABLE"
/usr/bin/codesign --force --sign - --timestamp=none "$PACKAGED_GIT_TEST_RUNNER"
/usr/bin/codesign --force --sign - --timestamp=none "$PACKAGED_GIT_TEST_APP"
/usr/bin/codesign --verify --strict --deep "$PACKAGED_GIT_TEST_APP"
"$PACKAGED_GIT_TEST_EXECUTABLE" "$PACKAGED_GIT_TEST_ROOT"

/usr/bin/swiftc \
    -O \
    -whole-module-optimization \
    -swift-version 6 \
    -D FLIT_NATIVE_TESTS \
    -strict-concurrency=complete \
    -warnings-as-errors \
    -module-cache-path "$TEST_MODULE_CACHE" \
    -I "$GENERATION_A" \
    -Xcc "-fmodule-map-file=$GENERATION_A/FlitBridgeFFI.modulemap" \
    "$GENERATION_A/FlitBridge.swift" \
    "$GENERATION_A/FlitProtocol.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/ApplicationLifecycle.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/AppDelegate.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/ExplicitQuit.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/FoundationCopy.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/SystemHealthClient.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/DashboardPresentation.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/NotificationDelivery.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/RunDetailPresentation.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/RunChangesPresentation.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/FoundationStatusBadge.swift" \
    "$REPOSITORY_ROOT/apps/macos/Sources/FlitMac/FoundationViewController.swift" \
    "$REPOSITORY_ROOT/apps/macos/Tests/NativeHealthTests.swift" \
    "$REPOSITORY_ROOT/target/$host_target/release/libflit_bridge.a" \
    -framework AppKit \
    -framework SwiftUI \
    -framework UserNotifications \
    -o "$TEST_EXECUTABLE"

"$TEST_EXECUTABLE" "$REPOSITORY_ROOT"

if /usr/bin/grep -R -E 'AsyncStream|@_cdecl|dlopen|NSXPC' \
    "$REPOSITORY_ROOT/apps/macos" "$REPOSITORY_ROOT/crates/flit-bridge" >/dev/null; then
    echo "Native production source contains a forbidden asynchronous or manual bridge surface" >&2
    exit 1
fi

echo "Native macOS health validation passed"
