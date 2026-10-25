#!/usr/bin/env bash
# Times local transcription with the release `tern` CLI on a real speech
# clip (whisper.cpp's public-domain samples/jfk.wav, 11 s, converted to
# M4A and MP3 with ffmpeg) for Whisper small and Moonshine. Prints the
# per-stage timings `tern models transcribe` reports and writes a table to
# the job summary. Fails when a transcript misses the words or a model
# cannot run; a download that cannot be fetched (the network, GitHub or
# Hugging Face being down) is reported as a warning, not a failure.
#   speech-bench.sh <path to tern> <label>
set -u
tern="$1"; label="${2:-$(uname -s)}"
export TERN_HOME="${RUNNER_TEMP:-/tmp}/tern-speech-home"
mkdir -p speech-bench && cd speech-bench
infra() { echo "::warning::speech benchmark ($label): $1 (infrastructure, not Tern)"; echo "> $1 (infrastructure)" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }
if ! curl -sSfL --retry 5 --retry-all-errors -o jfk.wav https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav; then
  infra "could not download the sample clip"; exit 0
fi
command -v ffmpeg >/dev/null || { infra "ffmpeg is not installed"; exit 0; }
ffmpeg -y -loglevel error -i jfk.wav -c:a aac -b:a 64k jfk.m4a
ffmpeg -y -loglevel error -i jfk.wav -c:a libmp3lame -b:a 64k jfk.mp3 || true
# A 12 s 44.1 kHz stereo M4A, like a phone voice memo.
ffmpeg -y -loglevel error -i jfk.wav -af "apad=pad_dur=1" -t 12 -ar 44100 -ac 2 -c:a aac -b:a 96k memo-44k.m4a
ls -la
echo "### Speech benchmark ($label)" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
echo "| model | file | wall s | tern's timings |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
echo "| --- | --- | --- | --- |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
fail=0
for m in whisper-small-q5_1 moonshine-streaming-small; do
  s=$(date +%s)
  ok=0
  for try in 1 2 3; do
    if "$tern" models download "$m" > "dl-$m.txt" 2>&1; then ok=1; break; fi
    sleep $((try * 10))
  done
  if [ $ok -ne 1 ]; then
    tail -5 "dl-$m.txt"
    infra "download of $m failed three times"
    continue
  fi
  echo "download $m: $(( $(date +%s) - s )) s"
  for f in memo-44k.m4a jfk.m4a jfk.mp3 jfk.wav; do
    [ -f "$f" ] || continue
    s=$(date +%s%N)
    out=$(timeout 600 "$tern" models transcribe "$f" --model "$m" --language en 2> err.txt); rc=$?
    ms=$(( ($(date +%s%N) - s) / 1000000 ))
    info=$(tr '\n' ' ' < err.txt)
    echo "== $m $f rc=$rc wall=${ms}ms"; echo "   text: $out"; echo "   $info"
    echo "| $m | $f | $(awk "BEGIN{printf \"%.1f\", $ms/1000}") | ${info//|//} |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
    if [ $rc -ne 0 ] || ! echo "$out" | grep -qi "country"; then echo "   MISSING WORDS"; fail=1; fi
  done
done
exit $fail
