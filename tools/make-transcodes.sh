#!/usr/bin/env bash
# Build a calibration set for `flint check`: each source album encoded with several lossy encoders and
# turned back into FLAC, plus an upsampled and a bit-padded copy. Nothing here is a real lossless file
# except `*-original.flac`, so every verdict is known in advance.
#
#   tools/make-transcodes.sh <output folder> <source.flac> [<source.flac> ...]
#
# FFMPEG may point at a particular FFmpeg; otherwise the one on PATH is used.
set -eu
out=${1:?output folder}; shift
ff=${FFMPEG:-ffmpeg}
mkdir -p "$out"
i=0
for src in "$@"; do
  i=$((i + 1))
  q() { "$ff" -hide_banner -loglevel error -y "$@"; }
  q -i "$src" -map 0:a -c:a libmp3lame -b:a 128k "$out/t.mp3"; q -i "$out/t.mp3"  -c:a flac -sample_fmt s16 "$out/$i-mp3-128.flac"
  q -i "$src" -map 0:a -c:a libmp3lame -q:a 0    "$out/t.mp3"; q -i "$out/t.mp3"  -c:a flac -sample_fmt s16 "$out/$i-mp3-v0.flac"
  q -i "$src" -map 0:a -c:a libmp3lame -b:a 320k "$out/t.mp3"; q -i "$out/t.mp3"  -c:a flac -sample_fmt s16 "$out/$i-mp3-320.flac"
  q -i "$src" -map 0:a -c:a aac        -b:a 256k "$out/t.m4a"; q -i "$out/t.m4a"  -c:a flac -sample_fmt s16 "$out/$i-aac-256.flac"
  q -i "$src" -map 0:a -c:a libopus    -b:a 160k "$out/t.opus"; q -i "$out/t.opus" -c:a flac -sample_fmt s16 -ar 44100 "$out/$i-opus-160.flac"
  q -i "$src" -map 0:a -c:a libvorbis  -q:a 6    "$out/t.ogg"; q -i "$out/t.ogg"  -c:a flac -sample_fmt s16 "$out/$i-vorbis-q6.flac"
  q -i "$src" -map 0:a -c:a flac -ar 96000 -sample_fmt s32 -bits_per_raw_sample 24 "$out/$i-up96-24.flac"
  q -i "$src" -map 0:a -c:a flac        -sample_fmt s32 -bits_per_raw_sample 24 "$out/$i-pad24.flac"
  q -i "$src" -map 0:a -c:a flac "$out/$i-original.flac"
  echo "source $i done"
done
rm -f "$out"/t.mp3 "$out"/t.m4a "$out"/t.opus "$out"/t.ogg
