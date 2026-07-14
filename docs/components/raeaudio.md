# AthAudio

Low-latency audio engine. Sub-3ms round-trip on certified hardware.

## Goals

- Single audio model. No ASIO / WASAPI / WDM split; no PulseAudio / PipeWire / JACK confusion.
- Real-time mixer that runs in its own scheduling class (paired with `SCHED_BODY` from AthKernel)
- Per-app routing, hot-pluggable virtual devices (VoiceMeeter-class, built in, properly)
- Surround, spatial audio (Atmos-class metadata), and headphone HRTF without third-party shims
- Hardware-accelerated where supported (e.g., on GPU compute or DSP cores)

## Non-goals

- Pro studio DAW timing fidelity. We target sub-3ms; a DAW will still want a dedicated stack.

## Latency model

The pipeline from input to output:

```
mic ADC → ring buffer (hw) → kernel ISR → mixer thread (RT class) → app callback
                                              ↓
                                      hw ring buffer (out) → DAC → speaker
```

Budget at 48 kHz, 64-frame buffer: ~1.33 ms per buffer. Target round-trip ≤ 3 ms
on certified hardware (two buffers + scheduling jitter).

## Surface sketch

```rust
let stream = athaudio::OutputStream::builder()
    .format(Format::F32, 48_000, 2)
    .frames_per_buffer(64)
    .build(|out, info| {
        // Real-time callback, must not allocate or block.
        for frame in out.chunks_exact_mut(2) {
            frame[0] = ...;
            frame[1] = ...;
        }
    })?;
stream.start()?;
```

## Open design questions

- App routing UX: implicit (last-used device) vs. per-app sticky?
- Voice chat noise suppression: built in, or app-provided?
- HRTF profile distribution: ship a default, ship a scan tool, or both?
