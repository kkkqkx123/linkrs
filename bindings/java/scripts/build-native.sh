#!/bin/sh
# Build the JNI bridge against the workspace Rust dynamic library.
# Usage: sh scripts/build-native.sh [debug|release]   (run from bindings/java)
set -eu

here=$(dirname "$0")
root=$(cd "$here/../../.." && pwd)
profile=${1:-debug}

if [ ! -f "$root/target/$profile/liblinkrs.so" ] && [ ! -f "$root/target/$profile/liblinkrs.dylib" ]; then
  echo "liblinkrs not found; run \`cargo build --features c_api\` first." >&2
  exit 1
fi

if [ -n "${JAVA_HOME:-}" ]; then
  java_home=$JAVA_HOME
else
  javac_bin=$(readlink -f "$(command -v javac)")
  java_home=$(cd "$(dirname "$javac_bin")/.." && pwd)
fi

os=$(uname -s)
case $os in
  Darwin) platform=darwin; libname=liblinkrs_jni.dylib; rustlib=liblinkrs.dylib ;;
  MINGW*|MSYS*|CYGWIN*) platform=win32; libname=linkrs_jni.dll; rustlib=linkrs.dll ;;
  *) platform=linux; libname=liblinkrs_jni.so; rustlib=liblinkrs.so ;;
esac

out="$root/target/$profile/$libname"
cc=${CC:-cc}

# shellcheck disable=SC2086
$cc -shared -fPIC -O2 \
  -I"$java_home/include" -I"$java_home/include/$platform" \
  -I"$root/include" \
  "$root/bindings/java/src/main/c/linkrs_jni.c" \
  -L"$root/target/$profile" -llinkrs \
  -Wl,-rpath,"\$ORIGIN" -Wl,-rpath,"$root/target/$profile" \
  -o "$out"

echo "wrote $out (linked against $root/target/$profile/$rustlib)"
