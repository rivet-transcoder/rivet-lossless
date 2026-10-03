#!/bin/sh
# Builds `alacconvert`, Apple's ALAC reference encoder and decoder, from
# Apple's open-source ALAC release (https://github.com/macosforge/alac,
# Apache 2.0), for tests/oracle.rs to use as a black box.
#
#   tools/build-alacconvert.sh [DEST_DIR]     (default: ./target/alac)
#
# Prints the path of the binary; point ALACCONVERT at it or put it on PATH.
# Needs git, make and a C++ compiler (g++ or clang++; CXX overrides). The
# release is pinned to a commit. Only build flags are set here: the
# release's makefile links the library before the objects, which GNU ld
# rejects, so the final link is done by hand.
set -eu
REV=c38887c5c5e64a4b31108733bd79ca9b2496d987
DEST=${1:-target/alac}
CXX=${CXX:-g++}
if [ ! -d "$DEST/.git" ]; then
  rm -rf "$DEST"
  git clone --quiet https://github.com/macosforge/alac.git "$DEST"
fi
git -C "$DEST" -c advice.detachedHead=false checkout --quiet "$REV"
make --no-print-directory -s -C "$DEST/codec" CC="$CXX" CFLAGS="-O2 -c -w" >&2
(
  cd "$DEST/convert-utility"
  "$CXX" -O2 -w -I ../codec -c main.cpp CAFFileALAC.cpp
  "$CXX" main.o CAFFileALAC.o -L../codec -lalac -o alacconvert
)
echo "$(cd "$DEST/convert-utility" && pwd)/alacconvert"
