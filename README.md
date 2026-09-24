# Speaker Studio

Version: **0.1.1**. The source version is recorded in `VERSION` and `Cargo.toml`.

Read the [how-to guide](docs/USER_GUIDE.md), use **Help / F1** in the app, or hover over controls for tips.

A local app for live dictation and for diarizing a video, audio file, or link. NVIDIA Nemotron 3 decides who spoke when. Whisper writes the words. Speaker names can carry a player, a character, and a role, such as `Greg - Ruthgar the Invincible - Barbarian`.

## Run

Double-click `Start Speaker Studio.bat`. Speaker Studio opens its own window.

The first launch builds the Rust app and installs the speech engine into `.engine`. That download is large. Later launches open the window directly. Speech inference runs on this computer; downloading models and linked media uses the internet.

Nemotron 3 was not already in the Hugging Face cache on this machine. The first diarization downloads `nvidia/Nemotron-3-Diarization`. Whisper large-v3-turbo is used from the local cache when it is present.

## What you can do

- Drop a video or audio file, or choose one.
- Paste a YouTube link or another video URL. The file is downloaded locally and then played back.
- Select the output device and press **Live dictation**. Enable **Me** to include the Windows default microphone. Lanes and lines update while audio plays. Stopping runs a full pass over the recording.
- Drag across the waveform to choose a range, then **Diarize selection**. Ranges expand to complete existing transcript lines so words outside a cut are preserved. Live sessions analyze the desktop track while preserving the separate microphone transcript.
- Click a speaker name and type the longer label. It is saved with the session.
- Copy or download the transcript.

Sessions are stored in `sessions/`. The speech models run from `engine/engine.py` inside the `.engine` environment.

Closing the window during capture finalizes the audio. Interrupted live recordings are recovered from their separate audio tracks on the next launch. Turning off **Me** stops microphone capture without hiding or removing earlier speech.

For dependency repair, run `powershell -NoProfile -ExecutionPolicy Bypass -File .\start.ps1`. The launcher retries setup if its completion marker or executable is missing.

## Development

The local Git history starts at tag `v0.1.0`. Recordings, downloaded dependencies, build output, and executables are excluded from source control.

- Rust checks: `cargo test --locked`
- Engine checks (no models or audio devices needed): `.\.engine\Scripts\python.exe -B -m unittest discover -s engine -p test_engine.py -v`
- Release build: `cargo build --release --locked`, then copy `target\release\speaker-studio.exe` to `Speaker Studio.exe`.

The supplied artwork is stored in `assets/`; `build.rs` embeds the Windows icon and package version using the Windows SDK resource compiler.
