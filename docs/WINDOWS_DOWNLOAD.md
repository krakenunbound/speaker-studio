# Run the Windows download

Download **Speaker-Studio-0.1.2-windows-x64.zip** from the [GitHub release](https://github.com/krakenunbound/speaker-studio/releases/tag/v0.1.2). This contains the compiled app and its support files. You do not need Rust, Cargo, or Visual Studio Build Tools for this download.

## First setup

1. Use Windows x64 and install these prerequisites if they are missing:
   - [Python 3.11 for Windows](https://www.python.org/downloads/windows/), including the `py` launcher. In a terminal, `py -3.11 --version` should report Python 3.11.
   - [Git for Windows](https://git-scm.com/download/win), available on PATH for dependency installation.
   - [FFmpeg](https://ffmpeg.org/download.html), with its `bin` directory on PATH for media conversion.
   - The [Microsoft Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist), if it is not already installed. This is a runtime, not the C++ build tools.
2. Right-click the ZIP and choose **Extract All**. Put the extracted folder somewhere writable, such as your Documents folder. Do not run the app from inside the ZIP or put it under Program Files.
3. Open the extracted folder and double-click **Start Speaker Studio.bat**.
4. Allow the first setup to finish. It creates a local `.engine` environment and downloads large speech dependencies. The app then downloads its models as needed. Keep an internet connection available and allow several gigabytes of free space, plus room for your recordings.
5. Wait for **Nemotron** in the app, then follow the [user guide](USER_GUIDE.md). Future launches reuse the installed engine and compiled executable.

Keep `engine/`, `.engine/`, `start.ps1`, and the launcher alongside **Speaker Studio.exe**. The ZIP is a portable app package with first-run dependency setup, not an offline installer. Python, Git, FFmpeg, and the Microsoft runtime are installed separately. GPU acceleration requires compatible NVIDIA hardware and drivers; CPU mode can be too slow for live recording.

## Which download should I use?

- **Speaker-Studio-0.1.2-windows-x64.zip:** recommended for a new installation; includes the executable, engine scripts, launchers, and guides.
- **Speaker-Studio-0.1.2-windows-x64.exe:** the same compiled executable, for an existing compatible installation. Rename it to **Speaker Studio.exe** in that installation. It does not include the support files, so prefer the ZIP when updating between releases.
- **SHA256SUMS.txt:** checksums for verifying the ZIP and EXE.
- GitHub's **Source code** downloads contain source files and require the build tools.

The executable is not code-signed, so Windows may show an unknown-publisher warning. Verify that your download came from this project's release page. To compare a downloaded file with `SHA256SUMS.txt`, use:

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath '.\Speaker-Studio-0.1.2-windows-x64.zip'
```

## Updates, repair, and removal

Close Speaker Studio before updating. Extract the new ZIP to a new folder, complete its setup, then copy your old `sessions/` folder if you want your recordings. Keep the old folder until the new version works. Do not copy `.engine/` between installation folders; Python environments contain installation-specific paths.

For dependency repair, open PowerShell in the extracted folder and run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\start.ps1
```

This keeps the included executable. The `-Build` option is for developers with a source checkout and Rust/C++ build tools.

To remove the portable app, close it and delete its extracted folder after backing up any recordings you want to keep. Separately installed prerequisites and model caches outside the folder remain installed.
