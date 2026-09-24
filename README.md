# Speaker Studio

Version: **0.1.0**. The source version is recorded in `VERSION` and `Cargo.toml`.

A local app for live dictation and for diarizing a video, audio file, or link. NVIDIA Nemotron 3 decides who spoke when. Whisper writes the words. Speaker names can carry a player, a character, and a role, such as `Greg - Ruthgar the Invincible - Barbarian`.

## Run

Double-click `Start Speaker Studio.bat`. Speaker Studio opens its own window.

The first launch builds the Rust app and installs the speech engine into `.engine`. That download is large. Later launches open the window directly. The window talks only to this computer.

Nemotron 3 was not already in the Hugging Face cache on this machine. The first diarization downloads `nvidia/Nemotron-3-Diarization`. Whisper large-v3-turbo is used from the local cache when it is present.

## What you can do

- Drop a video or audio file, or choose one.
- Paste a YouTube link or another video URL. The file is downloaded locally and then played back.
- Press **Live dictation** and allow the microphone. Lanes and lines update while you talk. Stopping runs a full pass over the recording.
- Drag across the waveform to choose a range, then **Diarize selection**.
- Click a speaker name and type the longer label. It is saved with the session.
- Copy or download the transcript.

Sessions are stored in `sessions/`. The speech models run from `engine/engine.py` inside the `.engine` environment.
