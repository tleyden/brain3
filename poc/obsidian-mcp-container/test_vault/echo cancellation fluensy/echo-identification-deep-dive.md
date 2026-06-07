# Echo Identification with Mic + Loopback Audio

Deep dive on tagging audio chunks as near-end (you), far-end speaker bleed, or a mix, with a confidence score. Context: recording a Zoom / Google Meet call where both the mic stream and the system (loopback) audio are captured. On macOS the system audio comes from the ScreenCaptureKit audio-only tap.

## Core reframe

This is not echo *cancellation*, it is echo *classification*. The goal is to label each chunk, not clean the mic. That is the well-studied problem of double-talk detection (DTD), normally used as a side gate to freeze an adaptive filter during overlap. Here DTD becomes the main output, which makes the job easier than full AEC.

Mic model: `mic = your_voice + (echo_path * system_audio) + noise`. You have `system_audio` (the loopback reference), so the whole game is: how much of the energy in this mic frame is explained by the reference?

- Explained almost entirely by reference: pure speaker bleed
- Reference silent but mic has energy: pure near-end (you)
- Reference active AND leftover mic energy unexplained by reference: mix (double-talk)

The confidence score falls out of "how well does the reference explain the mic," measured two ways: coherence and residual energy after adaptive filtering.

## Step 0: align the streams (do not skip)

The mic (AVAudioEngine) and the system audio tap (ScreenCaptureKit) run on separate clocks and buffers, so there is an unknown offset plus slow drift over a long call. Every correlation/filter assumes alignment.

Standard tool: GCC-PHAT (generalized cross-correlation with phase transform). Take the cross-power spectrum, normalize away magnitude so only phase survives, inverse-FFT, peak location is the delay. Run on a sliding window (every few seconds) to track drift.

- Canonical ~30-line implementation: respeaker/mic_array `gcc_phat.py`
- `pyroomacoustics` also ships one

## Step 1: cheap and surprisingly good (coherence)

Afternoon-sized, numpy/scipy, no adaptive filter:

1. Align with GCC-PHAT
2. Frame both signals (20 ms frames, ~50% overlap)
3. Compute magnitude-squared coherence between mic and aligned reference, averaged over the speech band (~300 to 3400 Hz). `scipy.signal.coherence` does this directly.

Coherence is a normalized cross-spectral measure on a 0 to 1 scale. That value IS the confidence score, no calibration:

- Coherence near 1 across speech band: mic dominated by reference, speaker bleed
- Coherence near 0 while reference active: your voice overpowering the echo, near-end or mix
- Reference frame energy below a floor: no echo possible, tag near-end/silence immediately (trivial VAD on the reference kills a lot of false positives)

This alone gives a solid three-way tag with a real confidence number. Prototype this first.

## Step 2: more accurate (adaptive filter + ERLE)

Coherence gives correlation, not energy share. Model the echo path with an adaptive filter (NLMS is the workhorse; RLS converges faster, costs more). The filter models the transfer function from loudspeaker input to mic output; subtract the echo estimate to get an error signal.

Per frame:
- `echo_estimate` = filter applied to reference
- `residual` = mic minus echo_estimate
- ERLE = 10 * log10(mic_power / residual_power)

High ERLE: reference explained the frame well (bleed). Low ERLE with active reference: near-end energy the filter could not predict (mix). A converged linear filter typically hits 20 to 40 dB ERLE; under 10 dB means it is not tracking, so ERLE doubles as an alignment sanity check.

Classic DTD statistics like normalized cross-correlation (NCC) formalize the same comparison and are treated as the robust reference method in the literature, versus the older Geigel algorithm (level comparison only, weak).

Python adaptive filter libs: `padasip`, `adaptfilt` (NLMS in a few lines).

## Build vs buy

**WebRTC AEC3** is the pragmatic shortcut. Pure C++, no platform deps, battle-tested across billions of calls. Already does alignment, adaptive filter, and a residual estimator (estimates remaining echo power per frequency band, uses ERLE to calibrate suppression). Repurpose its internal per-band echo estimate / suppression gain as the "how much is echo" score rather than using the cleaned output.
- Bindings: `python-webrtc-audio-processing`; commercial Switchboard SDK wraps AEC3 as a node
- SpeexDSP: older, simpler echo canceller, lighter

**Deep learning** is current SOTA, worth it for hard overlap cases:
- DTLN-aec (breizhn/DTLN-aec): dual-signal LSTM, 3rd in Microsoft AEC Challenge, pretrained TF-Lite (128/256/512 LSTM units), MIT licensed, real-time. Inputs `*_mic.wav` + `*_lpb.wav` (loopback), outputs cleaned near-end. Derive an echo mask by comparing input to output: frames where it stripped lots of energy were echo-heavy. Robust to variable delay and even works after other preprocessing, which sidesteps some alignment pain.
- Newer ICASSP 2022 complex-valued / F-T-LSTM nets beat it but are less cleanly packaged.
- `microsoft/AEC-Challenge` repo has datasets and baselines.

## Recommended path for Fluensy

1. Start: coherence + reference VAD (Step 1) + GCC-PHAT alignment. One day, interpretable, coherence is a clean confidence score.
2. If mixed frames get mislabeled: add NLMS filter, use ERLE as a second axis.
   - Coherence answers: is it correlated with the reference
   - ERLE answers: how much of the energy is explained
   - Disagreements between the two are exactly the low-confidence mix frames
3. Reach for DTLN-aec only if the classic approach cannot separate overlapping speech well enough.

## Open items

- Compare against Granola's actual approach (their pipeline is not published; the above is general state of the art).
- Do a search on open source projects and investigate their architecture.
