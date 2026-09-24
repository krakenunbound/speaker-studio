# Speaker Studio: how to use it

For version **0.1.1**. Open **Help** in the app, or press **F1**, for a quick guide. Hover over controls to see tips.

## Start the app

Double-click **Start Speaker Studio.bat** in the main folder. The first setup installs the local speech engine and builds the app; downloads can take a while. Later launches open the executable directly.

Wait for the engine indicator to say **Nemotron**. Models and linked media may need an internet connection to download. Speech analysis runs on this computer. This app still uses its `engine` and `.engine` folders, so keep them alongside the executable.

## Record a meeting, game, or browser audio

1. Start the audio in your meeting, game, or browser.
2. Under **Output**, select the same headphones or speakers that application uses. This chooses what gets recorded; it does not change Windows playback routing.
3. Enable **Me** if you also want to record the Windows default microphone. Leave it off for desktop audio only.
4. Press **Live dictation**. Speaker lanes and transcript lines appear as audio is processed. Text normally follows speech by several seconds.
5. Press **Stop dictation** when finished. Wait for the full analysis to finish before reviewing or re-analyzing the recording.

Use headphones when possible: otherwise your microphone may pick up the desktop voices too. If you need another microphone, select it as the default input device in Windows before recording.

You can turn **Me** off and back on during a recording. Earlier microphone speech stays saved and visible. Switching to a different saved session does not stop the active recording; **Stop dictation** still finishes that recording.

Closing the window while recording finalizes the audio. The full transcript pass may need to be rerun after reopening. After an interruption, the app attempts to rebuild playback audio from its saved desktop and microphone tracks.

## Import a recording or video link

- Drop an audio/video file into the window, or press **Choose a file**. Analysis starts automatically.
- Paste a complete video URL into **Link**, then press **Open** to download and analyze it.
- To transcribe audio currently playing in a browser, use **Live dictation** and the appropriate output device.

An imported file is copied into its session folder. Your original file stays where it was. Some imported formats have audio playback without a video preview.

## Play and select audio

| Control | What it does |
| --- | --- |
| Play / Pause | Start, resume, or pause playback. **Space** does the same when you are not typing. |
| Stop | Stop playback and return to the selection start. This does **not** stop recording. |
| Click waveform | Seek to that time and reset the selection to the full recording. |
| Click transcript text | Seek to that line. If paused, press Play to listen. |
| Drag main waveform | Select a range for copying or re-analysis. In split view, use the Party waveform. |
| Start / End | Adjust the selected range in seconds. Drag a value or type a number. |

Playback can continue beyond the end of a selected range. Start and End control copying and re-analysis, not a playback loop.

## Re-analyze a section

Stop recording and wait for any current analysis to finish. Select a range, then press **Diarize selection**.

The app expands the analysis range to complete existing transcript lines. This avoids cutting away words at the edges of a sentence. Speaker activity outside that range is preserved. Speaker names are matched using the existing timing evidence; check names after a difficult re-analysis because this is not voice identity verification.

For a live session, partial re-analysis processes the desktop track and preserves the separate **Me** transcript. To re-analyze microphone speech too, select the **whole recording** and run Diarize selection. For an imported recording, the selected source contains all recorded voices.

## Name speakers and add portraits

Click a speaker name in **Speaker activity**, type the label, then click away to save. You can include a player, character, and role, for example **Greg - Ruthgar - Barbarian**. Escape cancels an edit.

To add a portrait:

1. Copy an image to the clipboard.
2. Press **Paste** beside the intended speaker.

Alternatively, click a speaker's portrait or colored circle and drop an image file into the app. With a portrait selected, **Ctrl+V** also pastes an image when you are not typing. Labels and portraits belong to the current session.

## Understand speaker labels

- **Speaker 1–8:** anonymous voices detected in the desktop or imported audio. Rename them after listening.
- **Me:** audio from your microphone, kept separately in live sessions.
- **Overlap:** simultaneous speaker activity makes attribution uncertain. This does not separate mixed voices into independent recordings.
- **Unassigned:** the app cannot confidently attribute those words, including ambiguous speaker transitions.

**Show speakers** filters the first N numbered speakers in the display and exports. It does not change how many voices the model detects. **Me**, **Overlap**, and **Unassigned** stay included. Keep it at **8** for the most complete export.

## Copy, save, and share

| Control | Output |
| --- | --- |
| Copy text | Copies complete transcript lines that touch the selected time range. |
| Download | Saves the full text transcript for the shown speakers, including Me and uncertain labels. |
| Export page | Saves a standalone HTML transcript with names and portraits. Open it in a browser or share the file. Audio is not embedded. |

Names and timestamps are included. Read the transcript before sharing, particularly during interruptions, music, or simultaneous speech.

## Saved sessions and backups

Open **Sessions** to revisit recordings. **Delete** permanently removes a session and its local media; there is no undo. A currently recording session cannot be deleted.

Recordings, transcripts, and portraits live in the `sessions` folder beside the app. To back them up, stop recording, close the app, and copy that folder somewhere safe. Keep it out of source-control commits if it contains private recordings.

## Troubleshooting

**No desktop speech appears:** check that Output matches the device used by your meeting/browser and that sound is actually playing. Stop dictation, correct the device, and start a new recording if necessary.

**No microphone speech appears:** enable Me, check the Windows default input device, and allow desktop applications to use the microphone in Windows privacy settings.

**Words are delayed:** several seconds of delay is expected. Check Log for errors and allow a previous file analysis to finish. The final pass after Stop dictation can take longer than a live update.

**A person's speech is missing from the view or export:** set Show speakers to 8. Also inspect Unassigned and Overlap. Turning Me off does not remove earlier microphone speech.

**The engine will not load:** open Log for the error. Run the following from the app folder to verify/repair dependencies and rebuild:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\start.ps1
```

Setup requires Python 3.11 (the script uses `py -3.11`), Rust/Cargo, the Windows C++ build tools and SDK, Git for the Transformers installation, and FFmpeg for media conversion. The launcher retries setup after an incomplete installation.

**A recovered recording has unfinished text:** open it, select the full recording, and press Diarize selection. If recovery reports an error, keep the session folder; its separate audio tracks may still be usable.

**The Windows icon still looks old:** Windows may cache executable and shortcut icons. Close the old app and recreate a pinned shortcut if needed. The v0.1.1 executable and window contain the Kraken artwork.
