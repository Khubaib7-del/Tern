#!/usr/bin/env bash
# Builds tern-vlm, Tern's local vision helper (#69): llama.cpp's
# multimodal CLI (`llama-mtmd-cli`, MIT) at a pinned release, renamed.
#
#   .github/scripts/build-vlm.sh <out-dir>
#
# Windows and Linux: shared ggml with its backends loaded at run time
# (GGML_BACKEND_DL): the CPU backend and Vulkan. A PC without a Vulkan
# driver simply gets no Vulkan device and runs on the CPU. macOS: one
# static binary with Metal (the shaders embedded).
#
# The x86-64 CPU code is built for any CPU from 2013 on (AVX2, FMA, F16C),
# like whisper.cpp in Tern's own build (GGML_NATIVE off).
set -euo pipefail

TAG="${LLAMA_TAG:-b11900}"
OUT="${1:?usage: build-vlm.sh <out-dir>}"
SRC="${RUNNER_TEMP:-/tmp}/llama.cpp-$TAG"
mkdir -p "$OUT"

if [ ! -d "$SRC/.git" ]; then
  git clone --quiet --depth 1 --branch "$TAG" https://github.com/ggml-org/llama.cpp "$SRC"
fi

common=(
  -DCMAKE_BUILD_TYPE=Release
  -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF
  -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=OFF
  -DGGML_NATIVE=OFF
)
targets=(llama-mtmd-cli)
case "$(uname -s)" in
  Darwin)
    extra=(-DBUILD_SHARED_LIBS=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON -DGGML_BLAS=OFF)
    ;;
  Linux)
    extra=(-DBUILD_SHARED_LIBS=ON -DGGML_BACKEND_DL=ON -DGGML_VULKAN=ON
      -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON "-DCMAKE_INSTALL_RPATH=\$ORIGIN")
    targets+=(ggml-cpu ggml-vulkan)
    ;;
  *)
    extra=(-DBUILD_SHARED_LIBS=ON -DGGML_BACKEND_DL=ON -DGGML_VULKAN=ON)
    targets+=(ggml-cpu ggml-vulkan)
    ;;
esac

cmake -S "$SRC" -B "$SRC/build" "${common[@]}" "${extra[@]}" > "$SRC/configure.log" 2>&1 \
  || { tail -60 "$SRC/configure.log"; exit 1; }
grep -iE "vulkan|metal|GGML_AVX2|CPU backend" "$SRC/configure.log" | head -20 || true
for t in "${targets[@]}"; do
  cmake --build "$SRC/build" --config Release --target "$t" -j 4 > "$SRC/build-$t.log" 2>&1 \
    || { tail -80 "$SRC/build-$t.log"; exit 1; }
done

# Collect the program and its libraries.
case "$(uname -s)" in
  Darwin)
    cp "$SRC/build/bin/llama-mtmd-cli" "$OUT/tern-vlm"
    ;;
  Linux)
    cp "$SRC/build/bin/llama-mtmd-cli" "$OUT/tern-vlm"
    find "$SRC/build" -name '*.so*' \( -type f -o -type l \) -exec cp -P {} "$OUT/" \;
    ;;
  *)
    bin="$SRC/build/bin/Release"
    [ -d "$bin" ] || bin="$SRC/build/bin"
    cp "$bin/llama-mtmd-cli.exe" "$OUT/tern-vlm.exe"
    cp "$bin"/*.dll "$OUT/"
    ;;
esac
cp "$SRC/LICENSE" "$OUT/LICENSE-llama.cpp.txt"
echo "llama.cpp $TAG ($(git -C "$SRC" rev-parse --short HEAD))" > "$OUT/VERSION.txt"
ls -la "$OUT"
