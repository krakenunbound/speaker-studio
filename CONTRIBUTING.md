# Contributing

Use Windows x64 with the tools listed in the [README](README.md#requirements-for-building-from-source). Keep changes focused and include a regression test when fixing behavior that can be tested without audio hardware or model downloads.

## Run checks

From the project folder:

```powershell
cargo test --locked
.\.engine\Scripts\python.exe -B -m unittest discover -s engine -p test_engine.py -v
cargo build --release --locked
```

The Python command uses an engine environment created by `start.ps1`. To run only the Python tests without installing the full speech engine, create a separate test environment:

```powershell
py -3.11 -m venv .venv
.\.venv\Scripts\python.exe -m pip install "numpy>=1.26,<3"
.\.venv\Scripts\python.exe -m pip install torch --index-url https://download.pytorch.org/whl/cpu
.\.venv\Scripts\python.exe -B -m unittest discover -s engine -p test_engine.py -v
```

Tests use synthetic data and CPU tensors; no speech models or recording devices are required. The [Windows workflow](.github/workflows/ci.yml) runs the same checks on pushes and pull requests. These checks do not replace a manual test of GPU inference, desktop/microphone capture, and media import when those paths change.

## Build the local executable

Close Speaker Studio, then run:

```powershell
cargo build --release --locked
Copy-Item -LiteralPath 'target\release\speaker-studio.exe' -Destination 'Speaker Studio.exe' -Force
```

The root executable is ignored by Git. Keep `Cargo.lock` committed. New engine installations pin Transformers to a tested revision in `start.ps1`; validate Nemotron loading and streaming before changing that revision.

The setup script reuses an existing executable. To force a rebuild during setup, use `powershell -NoProfile -ExecutionPolicy Bypass -File .\start.ps1 -Build`. To build the downloadable ZIP, EXE, and checksums, run `powershell -NoProfile -ExecutionPolicy Bypass -File .\package-release.ps1`; outputs go to the ignored `dist/` folder. See the [publication guide](docs/PUBLISHING.md).

## Report a problem

Include the app version, Windows version, CPU/GPU, steps to reproduce, expected result, and the relevant error from **Log**. For a capture issue, say which output device was selected and whether **Me** was enabled. Use a short synthetic or shareable sample where possible. Remove private transcript text and local paths from logs before posting them.

Do not commit `sessions/`, `.engine/`, virtual environments, model weights, executables, or private credentials. The source, supplied app icons, tests, and documentation belong in Git.
