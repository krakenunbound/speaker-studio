# Changes

## 0.1.1

- Preserve transcript boundaries and uncertain speaker labels when re-analyzing a selection; keep distinct incoming voices separate.
- Keep microphone history visible and avoid mixing it back into desktop-only selection analysis.
- Finalize audio on close and recover interrupted recordings from their raw tracks.
- Skip known muted microphone intervals without delaying current transcription.
- Preserve late-recognized and revised words regardless of their age.
- Process diarization activity incrementally and release consumed PCM while retaining model and recognition context.
- Pass live audio directly to recognition; reduce live save frequency and idle repainting.
- Retry incomplete setup and report audio-mixing failures without silently omitting the microphone.
- Embed the supplied Kraken artwork as the executable and window icon; display the application version.
- Add in-app Help (F1), control tooltips, and a local how-to and troubleshooting guide.
- Add CPU-only regression coverage for capture buffers, recognition boundaries, diarization, selection merging, and recovery.

## 0.1.0

- Local Git baseline of the existing Rust desktop application and Python speech engine before these fixes.
