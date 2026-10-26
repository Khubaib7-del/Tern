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
# Whisper small on the 12 s memo with 2 and 4 threads, flash attention on
# and off (TERN_WHISPER_THREADS / TERN_WHISPER_FLASH), for the thread and
# attention defaults (#66). On the CPU (TERN_SPEECH_DEVICE=cpu).
if [ -f memo-44k.m4a ]; then
  echo "| whisper threads / flash | file | wall s | tern's timings |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
  # 6 and 8 threads on these 4-vCPU runners (more threads than logical
  # cores) took 54-93 s on Linux and 266-575 s on Windows in #66, so they
  # are no longer timed.
  for th in 2 4; do
    for fa in 1 0; do
      s=$(date +%s%N)
      out=$(TERN_SPEECH_DEVICE=cpu TERN_WHISPER_THREADS=$th TERN_WHISPER_FLASH=$fa timeout 600 "$tern" models transcribe memo-44k.m4a --model whisper-small-q5_1 --language en 2> err.txt); rc=$?
      ms=$(( ($(date +%s%N) - s) / 1000000 ))
      info=$(tr '\n' ' ' < err.txt)
      echo "== threads=$th flash=$fa rc=$rc wall=${ms}ms"; echo "   $info"
      echo "| $th / $fa | memo-44k.m4a | $(awk "BEGIN{printf \"%.1f\", $ms/1000}") | ${info//|//} |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
    done
  done
fi
# Where Whisper ran (#67): Auto on this runner, then GPU asked for. With
# no GPU (and on Windows no vulkan-1.dll) both must run on the CPU and say
# why; on Linux with GGML_VK_VISIBLE_DEVICES=0 Mesa's lavapipe (software
# Vulkan) stands in for a GPU, which proves the Vulkan path end to end
# (its time is a CPU emulating a GPU, not a GPU's).
if [ -f jfk.wav ]; then
  echo "| whisper device | file | wall s | tern's timings |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
  runs="auto gpu"
  if [ "$(uname -s)" = Linux ] && ls /usr/share/vulkan/icd.d/lvp_icd*.json >/dev/null 2>&1; then runs="$runs lavapipe"; fi
  for dev in $runs; do
    s=$(date +%s%N)
    case $dev in
      lavapipe)
        icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json | head -n 1)
        out=$(VK_ICD_FILENAMES="$icd" VK_DRIVER_FILES="$icd" GGML_VK_VISIBLE_DEVICES=0 TERN_SPEECH_DEVICE=gpu timeout 900 "$tern" models transcribe jfk.wav --model whisper-small-q5_1 --language en 2> err.txt); rc=$? ;;
      *)
        out=$(TERN_SPEECH_DEVICE=$dev timeout 600 "$tern" models transcribe jfk.wav --model whisper-small-q5_1 --language en 2> err.txt); rc=$? ;;
    esac
    ms=$(( ($(date +%s%N) - s) / 1000000 ))
    info=$(tr '\n' ' ' < err.txt)
    echo "== device=$dev rc=$rc wall=${ms}ms"; echo "   text: $out"; echo "   $info"
    echo "| $dev | jfk.wav | $(awk "BEGIN{printf \"%.1f\", $ms/1000}") | ${info//|//} |" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
    if [ $rc -ne 0 ] || ! echo "$out" | grep -qi "country"; then
      if [ $dev = lavapipe ]; then echo "::warning::lavapipe run failed (rc=$rc)"; else echo "   MISSING WORDS"; fail=1; fi
    fi
  done
fi
exit $fail
