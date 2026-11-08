#!/usr/bin/env python3
"""Turns a raw stereo render into a 24-bit WAV file.

The choir harness (`archs/choirmojogpubatch.mojo`, mode `render`) writes its
stereo mix as raw interleaved native-endian 32-bit floats. This script writes
it as a 24-bit PCM WAV file, its peak brought to -1 dBFS (the renders have no
fixed level: it depends on the number of singers and on the hall).

    f32_to_wav.py <in.f32> <out.wav> [<rate>]

Standard library only.
"""

import array
import sys
import wave


def main():
    src, dst = sys.argv[1], sys.argv[2]
    rate = int(sys.argv[3]) if len(sys.argv) > 3 else 48000
    samples = array.array("f")
    with open(src, "rb") as f:
        samples.frombytes(f.read())
    if len(samples) % 2:
        sys.exit("f32_to_wav.py: odd sample count, not a stereo render")
    peak = max((abs(x) for x in samples), default=0.0)
    if peak == 0.0:
        sys.exit("f32_to_wav.py: silent render")
    scale = 10 ** (-1 / 20) / peak * 8388607
    out = bytearray()
    for x in samples:
        v = max(-8388608, min(8388607, round(x * scale)))
        out += (v & 0xFFFFFF).to_bytes(3, "little")
    with wave.open(dst, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(rate)
        w.writeframes(bytes(out))
    print(f"{dst}: {len(samples) // 2 / rate:.1f} s, peak {peak:.4f} brought to -1 dBFS")


if __name__ == "__main__":
    main()
