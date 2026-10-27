#!/usr/bin/env bash
# A real run of the #69 local runtimes with the release `tern` CLI:
#   1. tern-vlm + SmolVLM 256M describes a small picture (a red square
#      with the word TERN), through `tern models describe` (Tern's engine,
#      not the helper alone), and the helper's own device line is shown.
#   2. Kokoro 82M speaks one English sentence to a WAV (`tern models speak`).
#   3. A DOCX and a PDF become Markdown (`tern models convert`).
#   4. (#70) `tern-vlm --list-devices` runs (no crash without a GPU) and
#      SmolVLM's answer stays short (capped at 200 tokens).
#   5. (#70) A scan whose only text is a CamScanner watermark goes to OCR
#      (PP-OCRv6 small), never "PDF text"; an old Excel .xls becomes a table.
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
  [ -s helper.txt ] || { echo "(the helper printed nothing on stdout; the end of its log:)"; tail -8 helper.log; }
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
# 4. (#70) The vision helper lists its GPUs without crashing (no GPU on
#    the runners: "(none)"), and SmolVLM's answer is capped.
if [ -f "$TERN_VLM" ]; then
  timeout 60 "$TERN_VLM" --list-devices > devices.txt 2>&1; rc=$?
  echo "tern-vlm --list-devices exit $rc:"; cat devices.txt | head -12
  if [ $rc -ne 0 ]; then
    echo "::error::tern-vlm --list-devices failed (exit $rc)"; fail=1
    sum "| list GPUs | tern-vlm | FAILED exit $rc | - |"
  else
    sum "| list GPUs | tern-vlm | $(grep -iE 'vulkan|metal|none' devices.txt | head -2 | tr '\n|' '  ') | - |"
  fi
fi
if [ -s describe.txt ]; then
  n=$(wc -c < describe.txt)
  if [ "$n" -gt 1500 ]; then
    echo "::error::SmolVLM 256M wrote $n characters; the cap (200 tokens) should keep it near 1000"; fail=1
    sum "| describe length | smolvlm-256m | FAILED: $n chars | - |"
  else
    sum "| describe length | smolvlm-256m | $n chars (capped) | - |"
  fi
fi

# 5. (#70) A scan whose only text is a CamScanner watermark goes to OCR,
#    never "PDF text"; an old Excel .xls becomes a table.
"$py" - <<'EOF'
import struct, zlib
# A 5x7 block font, drawn 12x, for "TERN 4250" on a white page.
F = {
 'T': ["11111","00100","00100","00100","00100","00100","00100"],
 'E': ["11111","10000","10000","11110","10000","10000","11111"],
 'R': ["11110","10001","10001","11110","10100","10010","10001"],
 'N': ["10001","11001","10101","10011","10001","10001","10001"],
 '4': ["00010","00110","01010","10010","11111","00010","00010"],
 '2': ["01110","10001","00001","00010","00100","01000","11111"],
 '5': ["11111","10000","11110","00001","00001","10001","01110"],
 '0': ["01110","10001","10011","10101","11001","10001","01110"],
 ' ': ["00000"]*7,
}
text, s = "TERN 4250", 12
w, h = (len(text) * 6 + 4) * s, 11 * s
px = bytearray([255]) * (w * h)
for k, ch in enumerate(text):
    for r, row in enumerate(F[ch]):
        for c, bit in enumerate(row):
            if bit == "1":
                for y in range((r + 2) * s, (r + 3) * s):
                    for x in range((k * 6 + c + 2) * s, (k * 6 + c + 3) * s):
                        px[y * w + x] = 0
img = zlib.compress(bytes(px))
objs = [
 b"<< /Type /Catalog /Pages 2 0 R >>",
 b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
 b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
 b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>",
]
content = b"q 560 0 0 %d 26 500 cm /Im1 Do Q BT /F1 8 Tf 72 40 Td (Scanned with CamScanner) Tj ET" % int(560 * h / w)
objs.append(b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream")
objs.append(b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length %d >>\nstream\n" % (w, h, len(img)) + img + b"\nendstream")
out = bytearray(b"%PDF-1.4\n"); offs = []
for i, o in enumerate(objs):
    offs.append(len(out)); out += b"%d 0 obj\n" % (i + 1) + o + b"\nendobj\n"
x = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for o in offs: out += b"%010d 00000 n \n" % o
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, x)
open("scan.pdf", "wb").write(bytes(out))

# An Excel 97-2003 workbook: a compound file with a BIFF8 Workbook stream.
def rec(t, d): return struct.pack("<HH", t, len(d)) + d
wb = rec(0x0809, struct.pack("<HHHH", 0x600, 5, 0, 0))
wb += rec(0x0085, struct.pack("<IBB", 0, 0, 0) + bytes([5, 0]) + b"Costs")
sst = struct.pack("<II", 3, 3)
for t in ["Item", "Total", "Tea"]: sst += struct.pack("<HB", len(t), 0) + t.encode()
wb += rec(0x00FC, sst) + rec(0x000A, b"")
wb += rec(0x0809, struct.pack("<HHHH", 0x600, 0x10, 0, 0))
for r, c, i in [(0, 0, 0), (0, 1, 1), (1, 0, 2)]: wb += rec(0x00FD, struct.pack("<HHHI", r, c, 0, i))
wb += rec(0x0203, struct.pack("<HHHd", 1, 1, 0, 4250.0)) + rec(0x000A, b"")
wb = wb.ljust(4096, b"\0"); n = len(wb) // 512 + (len(wb) % 512 > 0); wb = wb.ljust(n * 512, b"\0")
END, FREE = 0xFFFFFFFE, 0xFFFFFFFF
hdr = bytearray(512)
hdr[0:8] = bytes([0xD0,0xCF,0x11,0xE0,0xA1,0xB1,0x1A,0xE1])
struct.pack_into("<HHHHH", hdr, 0x18, 0x3E, 3, 0xFFFE, 9, 6)
struct.pack_into("<I", hdr, 0x2C, 1); struct.pack_into("<I", hdr, 0x30, 1); struct.pack_into("<I", hdr, 0x38, 4096)
struct.pack_into("<I", hdr, 0x3C, END); struct.pack_into("<I", hdr, 0x44, END)
for k in range(109): struct.pack_into("<I", hdr, 0x4C + 4 * k, 0 if k == 0 else FREE)
fat = [FREE] * 128; fat[0] = 0xFFFFFFFD; fat[1] = END
for j in range(n): fat[2 + j] = END if j == n - 1 else 3 + j
d = bytearray(512)
def entry(k, name, kind, start, size):
    u = name.encode("utf-16-le"); e = 128 * k
    d[e:e + len(u)] = u; struct.pack_into("<HB", d, e + 0x40, len(u) + 2, kind)
    struct.pack_into("<III", d, e + 0x44, FREE, FREE, FREE); struct.pack_into("<II", d, e + 0x74, start, size)
entry(0, "Root Entry", 5, END, 0); entry(1, "Workbook", 2, 2, len(wb))
open("costs.xls", "wb").write(bytes(hdr) + struct.pack("<128I", *fat) + bytes(d) + wb)
EOF
if "$tern" models convert costs.xls > xls.md 2> xls.err && grep -q "| Tea | 4250 |" xls.md; then
  sum "| convert | XLS | $(head -1 xls.md | tr '|' ' ') | - |"
else
  cat xls.md xls.err; echo "::error::.xls to Markdown failed"; fail=1
  sum "| convert | XLS | FAILED: $(tail -c 160 xls.err | tr '\n|' '  ') | - |"
fi
# Without an OCR model the scan is an error naming what to download.
"$tern" models convert scan.pdf > scan0.md 2> scan0.err
if head -1 scan0.md | grep -qE "\(PDF text(,|\))"; then
  echo "::error::the CamScanner scan was read as PDF text"; cat scan0.md; fail=1
else
  echo "scan without OCR: $(head -c 300 scan0.err)"
fi
if download pp-ocrv6-small; then
  if "$tern" models convert scan.pdf > scan.md 2> scan.err; then
    cat scan.md
    if head -1 scan.md | grep -q "OCR"; then
      if grep -qiE "TERN|4250" scan.md; then
        sum "| convert | scanned PDF | $(head -1 scan.md | tr '|' ' ') ($(sed -n 2,4p scan.md | tr '\n|' '  ')) | - |"
      else
        echo "::warning::the scan went to OCR, but OCR did not read TERN 4250"
        sum "| convert | scanned PDF | OCR path, text not read: $(sed -n 2,4p scan.md | tr '\n|' '  ') | - |"
      fi
    else
      echo "::error::the CamScanner scan did not go to OCR"; fail=1
      sum "| convert | scanned PDF | FAILED: $(head -1 scan.md | tr '|' ' ') | - |"
    fi
  else
    cat scan.err; echo "::error::converting the scanned PDF failed"; fail=1
  fi
else
  infra "download of pp-ocrv6-small failed three times"
fi
exit $fail
