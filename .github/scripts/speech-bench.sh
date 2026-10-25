#!/usr/bin/env bash
# Times local transcription with the release `tern` CLI on a real speech
# clip (whisper.cpp's public-domain samples/jfk.wav, 11 s, converted to
# M4A and MP3 with ffmpeg) for Whisper small and Moonshine. Prints the
# per-stage timings `tern models transcribe` reports and writes a table to
# the job summary. Fails only when a transcript misses the words.
#   speech-bench.sh <path to tern> <label>
set -u
tern="$1"; label="${2:-$(uname -s)}"
export TERN_HOME="${RUNNER_TEMP:-/tmp}/tern-speech-home"
mkdir -p speech-bench && cd speech-bench
curl -sSfL --retry 3 -o jfk.wav https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav
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
  "$tern" models download "$m" > "dl-$m.txt" 2>&1 || { echo "download $m failed"; tail -5 "dl-$m.txt"; fail=1; continue; }
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
