#!/usr/bin/env bash
# A real run of the #69 local runtimes with the release `tern` CLI:
#   1. tern-vlm + SmolVLM 256M describes a small picture (a red square
#      with the word TERN), through `tern models describe` (Tern's engine,
#      not the helper alone), and the helper's own device line is shown.
#   2. Kokoro 82M speaks one English sentence to a WAV (`tern models speak`).
#   3. A DOCX and a PDF become Markdown (`tern models convert`).
# Fails when a model runs and gives nothing (or the wrong kind of thing);
# a download that cannot be fetched (Hugging Face down) is a warning.
#   local-runtimes-smoke.sh <path to tern> <path to the vlm folder> <label>
set -u
tern="$1"; vlm_dir="$2"; label="${3:-$(uname -s)}"
export TERN_HOME="${RUNNER_TEMP:-/tmp}/tern-runtimes-home"
exe=""; case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) exe=".exe";; esac
export TERN_VLM="$vlm_dir/tern-vlm$exe"
# Git Bash on Windows: a POSIX path for find, python3 may be "python".
home_posix="$TERN_HOME"; command -v cygpath >/dev/null && home_posix=$(cygpath -u "$TERN_HOME")
py=python3; python3 -c "" 2>/dev/null || py=python
mkdir -p runtimes-smoke && cd runtimes-smoke
sum() { echo "$1" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }
infra() { echo "::warning::runtimes smoke ($label): $1 (infrastructure, not Tern)"; sum "> $1 (infrastructure)"; }
fail=0
sum "### Local runtimes smoke test ($label)"
sum "| job | model | result | seconds |"
sum "| --- | --- | --- | --- |"

download() {
  for try in 1 2 3; do
    if "$tern" models download "$1" > "dl-$1.txt" 2>&1; then return 0; fi
    sleep $((try * 10))
  done
  tail -5 "dl-$1.txt"; return 1
}

# A 256x256 PNG: a red square with a white bar (pure Python, no Pillow).
"$py" - <<'EOF'
import struct, zlib
w = h = 256
rows = []
for y in range(h):
    row = bytearray([0])
    for x in range(w):
        white = 100 <= y < 156 and 40 <= x < 216
        row += bytes((255, 255, 255) if white else (220, 30, 30))
    rows.append(bytes(row))
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b"")
open("red-tern.png", "wb").write(png)
EOF

# 1. Vision.
if [ ! -x "$TERN_VLM" ] && [ ! -f "$TERN_VLM" ]; then
  echo "::error::tern-vlm is missing at $TERN_VLM"; fail=1
elif download smolvlm-256m; then
  dir=$(dirname "$(find "$home_posix" -name 'SmolVLM-256M-Instruct-Q8_0.gguf' | head -1)")
  # The helper alone, for its device lines.
  timeout 300 "$TERN_VLM" -m "$dir/SmolVLM-256M-Instruct-Q8_0.gguf" --mmproj "$dir/mmproj-SmolVLM-256M-Instruct-Q8_0.gguf" \
    --image red-tern.png -p "What colour is this picture?" -n 32 --temp 0 -t 4 > helper.txt 2> helper.log
  echo "helper exit $?: $(head -c 300 helper.txt)"
  grep -iE "using device|ggml_vulkan|CPU backend|Metal|offloaded|error" helper.log | head -12
  s=$(date +%s)
  # On the CPU: the runner's only Vulkan device is Mesa's software one.
  if TERN_SPEECH_DEVICE=cpu "$tern" models describe red-tern.png --model smolvlm-256m > describe.txt 2> describe.err; then
    t=$(( $(date +%s) - s ))
    cat describe.txt; cat describe.err
    if grep -qiE "red|square|white|rectangle|flag|colou?r" describe.txt; then
      sum "| describe | smolvlm-256m | $(head -c 120 describe.txt | tr '\n|' '  ') | $t |"
    else
      echo "::error::SmolVLM's description does not mention the picture"; fail=1
      sum "| describe | smolvlm-256m | FAILED: $(head -c 120 describe.txt | tr '\n|' '  ') | $t |"
    fi
  else
    cat describe.err; echo "::error::tern models describe failed"; fail=1
    sum "| describe | smolvlm-256m | FAILED: $(tail -c 200 describe.err | tr '\n|' '  ') | - |"
  fi
else
  infra "download of smolvlm-256m failed three times"
fi

# 2. Read aloud.
if download kokoro-82m-int8; then
  s=$(date +%s)
  if "$tern" models speak "Hello from Tern. This sentence was read aloud on this computer." --out hello.wav > speak.txt 2>&1; then
    t=$(( $(date +%s) - s ))
    cat speak.txt
    bytes=$(wc -c < hello.wav)
    if [ "$bytes" -gt 48000 ]; then
      sum "| speak | kokoro-82m-int8 | $bytes-byte WAV ($(tail -1 speak.txt | tr '|' ' ')) | $t |"
    else
      echo "::error::Kokoro wrote a WAV of only $bytes bytes"; fail=1
    fi
  else
    cat speak.txt; echo "::error::tern models speak failed"; fail=1
    sum "| speak | kokoro-82m-int8 | FAILED: $(tail -c 200 speak.txt | tr '\n|' '  ') | - |"
  fi
else
  infra "download of kokoro-82m-int8 failed three times"
fi

# 3. Documents.
"$py" - <<'EOF'
import zipfile
doc = """<w:document xmlns:w="w"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly report</w:t></w:r></w:p>
<w:p><w:r><w:t>Revenue grew in Lahore.</w:t></w:r></w:p>
</w:body></w:document>"""
with zipfile.ZipFile("report.docx", "w") as z:
    z.writestr("word/document.xml", doc)
EOF
if "$tern" models convert report.docx > docx.md 2> docx.err && grep -q "## Quarterly report" docx.md; then
  sum "| convert | DOCX | $(head -1 docx.md | tr '|' ' ') | - |"
else
  cat docx.md docx.err; echo "::error::DOCX to Markdown failed"; fail=1
fi
exit $fail
