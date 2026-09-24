param([switch]$Build)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$app = Join-Path $projectRoot 'Speaker Studio.exe'
$envRoot = Join-Path $projectRoot '.engine'
$pythonExe = Join-Path $envRoot 'Scripts\python.exe'
if (-not (Test-Path $pythonExe)) {
    py -3.11 -m venv $envRoot
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the speech engine. Install Python 3.11 and try again.' }
}

$readyMarker = Join-Path $envRoot 'speaker-studio-ready'
if (Test-Path -LiteralPath $readyMarker) { Remove-Item -LiteralPath $readyMarker -Force }
$ready = $false
try {
    & $pythonExe -c "import torch,faster_whisper,yt_dlp,soundfile,librosa,transformers,os; p=os.path.join(os.path.dirname(transformers.__file__),'models','nemotron3_diarization'); assert os.path.isdir(p), 'diarization model code missing'"
    if ($LASTEXITCODE -eq 0) { $ready = $true }
} catch { $ready = $false }

if (-not $ready) {
    Write-Host "Installing the local speech engine. The first run downloads PyTorch and Nemotron support."
    & $pythonExe -m pip install --upgrade pip
    if ($LASTEXITCODE -ne 0) { throw 'Could not update pip.' }
    & $pythonExe -m pip install torch torchaudio --index-url https://download.pytorch.org/whl/cu126
    if ($LASTEXITCODE -ne 0) { throw 'Could not install the CUDA audio runtime.' }
    & $pythonExe -m pip install -r (Join-Path $projectRoot 'engine\requirements.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Could not install the speech packages.' }
    # Use the revision tested with this app rather than a changing development branch.
    & $pythonExe -m pip install "transformers @ git+https://github.com/huggingface/transformers.git@98d39824ed30e684e5122d04a2d9564efffc4965"
    if ($LASTEXITCODE -ne 0) { throw 'Could not install Nemotron 3 support from transformers.' }
}

# Only mark setup complete after imports work. A partial installation must be retried.
& $pythonExe -c "import torch,faster_whisper,yt_dlp,soundfile,librosa,transformers,os; p=os.path.join(os.path.dirname(transformers.__file__),'models','nemotron3_diarization'); assert os.path.isdir(p), 'diarization model code missing'"
if ($LASTEXITCODE -ne 0) { throw 'Speech engine verification failed. Run start.ps1 again to repair it.' }

if ($Build -or -not (Test-Path -LiteralPath $app)) {
    Write-Host "Building Speaker Studio."
    Push-Location $projectRoot
    try {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'Could not build Speaker Studio.' }
        Copy-Item (Join-Path $projectRoot 'target\release\speaker-studio.exe') $app -Force
    } finally {
        Pop-Location
    }
} else {
    Write-Host "Using the included Speaker Studio executable."
}

Set-Content -LiteralPath $readyMarker -Value 'ready' -Encoding Ascii
Start-Process $app -WorkingDirectory $projectRoot -WindowStyle Hidden
