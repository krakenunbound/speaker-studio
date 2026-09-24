"""Local Nemotron 3 diarization and Whisper transcription worker.

Reads one JSON command per line on stdin. Writes one JSON event per line on stdout.
"""

from __future__ import annotations

import json
import os
import sys
import traceback
from pathlib import Path

os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")
os.environ.setdefault("TQDM_DISABLE", "1")
os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

DIAR_ID = "nvidia/Nemotron-3-Diarization"
HOP = 0.01


def emit(payload: dict) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


class AudioBuf:
    def __init__(self) -> None:
        self.blocks: list = []
        self.total = 0

    def append(self, pcm) -> None:
        import numpy as np

        pcm = np.ascontiguousarray(pcm, dtype=np.float32).reshape(-1)
        if pcm.size == 0:
            return
        self.blocks.append(pcm)
        self.total += int(pcm.size)
        if len(self.blocks) > 48:
            joined = np.concatenate(self.blocks)
            self.blocks = [joined]

    def slice(self, start: int, end: int):
        import numpy as np

        start = max(0, int(start))
        end = min(self.total, int(end))
        if end <= start:
            return np.zeros(0, dtype=np.float32)
        out = np.empty(end - start, dtype=np.float32)
        pos = 0
        filled = 0
        for block in self.blocks:
            b0 = pos
            b1 = pos + len(block)
            pos = b1
            if b1 <= start or b0 >= end:
                continue
            take = block[max(start, b0) - b0 : min(end, b1) - b0]
            out[filled : filled + len(take)] = take
            filled += len(take)
        return out


def find_asr() -> str:
    env = os.environ.get("SPEAKER_ASR_PATH", "").strip()
    if env and Path(env).is_dir():
        return env
    hub = Path.home() / ".cache" / "huggingface" / "hub"
    names = [
        "models--mobiuslabsgmbh--faster-whisper-large-v3-turbo",
        "models--Systran--faster-whisper-large-v3",
        "models--Systran--faster-whisper-medium",
        "models--Systran--faster-whisper-small",
        "models--Systran--faster-whisper-base",
        "models--openai--whisper-base",
    ]
    for name in names:
        snaps = hub / name / "snapshots"
        if not snaps.is_dir():
            continue
        found = sorted((p for p in snaps.iterdir() if p.is_dir()), key=lambda p: p.stat().st_mtime)
        if found and (found[-1] / "model.bin").exists():
            return str(found[-1])
    return "base"


def threshold_segments(logits) -> list[dict]:
    import numpy as np

    probs = logits
    if hasattr(probs, "sigmoid"):
        probs = probs.sigmoid()
    if hasattr(probs, "detach"):
        probs = probs.detach().float().cpu().numpy()
    probs = np.asarray(probs)
    if probs.ndim == 3:
        probs = probs[0]
    segments = []
    speakers = min(8, probs.shape[1] if probs.ndim == 2 else 0)
    frames = probs.shape[0] if probs.ndim == 2 else 0
    for speaker in range(speakers):
        column = probs[:, speaker]
        start = None
        for index, value in enumerate(column):
            if value >= 0.5:
                if start is None:
                    start = index
            elif start is not None:
                if (index - start) * HOP >= 0.08:
                    segments.append({"speaker": speaker, "start": round(start * HOP, 3), "end": round(index * HOP, 3)})
                start = None
        if start is not None and (frames - start) * HOP >= 0.08:
            segments.append({"speaker": speaker, "start": round(start * HOP, 3), "end": round(frames * HOP, 3)})
    segments.sort(key=lambda item: (item["start"], item["speaker"]))
    return segments


def normalize_segments(raw) -> list[dict]:
    segments = []
    for item in raw or []:
        if not isinstance(item, dict):
            continue
        speaker = item.get("Speaker", item.get("speaker", 0))
        start = item.get("Start", item.get("start", 0))
        end = item.get("End", item.get("end", 0))
        try:
            speaker_i = int(speaker)
            start_f = float(start)
            end_f = float(end)
        except (TypeError, ValueError):
            continue
        if 0 <= speaker_i < 8 and end_f - start_f >= 0.05:
            segments.append({"speaker": speaker_i, "start": round(start_f, 3), "end": round(end_f, 3)})
    segments.sort(key=lambda item: (item["start"], item["speaker"]))
    return segments


UNASSIGNED = 255
OVERLAP = 254
LIVE_RETAIN = 20.0


def speaker_owner(start: float, end: float, segments: list[dict]) -> int:
    """Nearest speaker only counts inside one second. Heavy overlap stays ambiguous."""
    if not segments:
        return UNASSIGNED
    if end < start:
        end = start
    totals: dict[int, float] = {}
    nearest = 1e9
    nearest_speaker = None
    mid = (start + end) / 2.0
    for segment in segments:
        seg_start = float(segment["start"])
        seg_end = float(segment["end"])
        overlap = min(end, seg_end) - max(start, seg_start)
        speaker = int(segment["speaker"])
        if overlap > 0.0:
            totals[speaker] = totals.get(speaker, 0.0) + overlap
        else:
            distance = min(abs(mid - seg_start), abs(mid - seg_end))
            if distance < nearest:
                nearest = distance
                nearest_speaker = speaker
    if not totals:
        if nearest_speaker is None or nearest > 1.0:
            return UNASSIGNED
        return int(nearest_speaker)
    ranked = sorted(totals.items(), key=lambda item: item[1], reverse=True)
    best_speaker, best_overlap = ranked[0]
    if len(ranked) > 1:
        second = ranked[1][1]
        if second >= 0.05 and best_overlap < 0.7 * (best_overlap + second):
            return OVERLAP
    return int(best_speaker)


def assign_speakers(words: list[dict], segments: list[dict]) -> list[dict]:
    ordered = sorted(words, key=lambda word: (float(word["start"]), float(word["end"])))
    usable = sorted(segments, key=lambda segment: (float(segment["start"]), float(segment["end"])))
    left = 0
    rows = []
    for word in ordered:
        start = float(word["start"])
        end = float(word["end"])
        while left < len(usable) and float(usable[left]["end"]) < start - 1.0:
            left += 1
        right = left
        limit = end + 1.0
        while right < len(usable) and float(usable[right]["start"]) <= limit:
            right += 1
        rows.append(
            {
                "speaker": speaker_owner(start, end, usable[left:right]),
                "start": start,
                "end": end,
                "text": word["text"],
            }
        )
    return rows


def rows_to_turns(rows: list[dict]) -> list[dict]:
    turns = []
    for row in rows:
        text = str(row["text"]).strip()
        if not text:
            continue
        speaker = int(row["speaker"])
        start = float(row["start"])
        end = float(row["end"])
        if turns and turns[-1]["speaker"] == speaker and start - float(turns[-1]["end"]) < 0.85:
            turns[-1]["end"] = round(end, 3)
            turns[-1]["text"] = f"{turns[-1]['text']} {text}".strip()
        else:
            turns.append({"speaker": speaker, "start": round(start, 3), "end": round(end, 3), "text": text})
    return turns


def keep_recognized(old: list[dict], recognized: list[dict], committed: float, cursor: float, cutoff: float) -> list[dict]:
    """A word that crosses the commit boundary stays when recognition still returns it."""
    lo = committed - 0.45
    fresh = [
        word
        for word in recognized
        if float(word["end"]) > lo and float(word["start"]) < cutoff + 0.05 and float(word["end"]) <= cutoff + 0.05
    ]
    fresh.sort(key=lambda word: float(word["start"]))
    kept = [word for word in old if float(word["end"]) <= lo or float(word["start"]) >= cursor]
    kept.extend(word for word in fresh if float(word["end"]) <= cursor + 0.05)
    kept.sort(key=lambda word: float(word["start"]))
    return kept


def _self_check() -> None:
    word = {"start": 4.4, "end": 4.8, "text": "hello"}
    kept = keep_recognized([dict(word)], [dict(word)], 5.0, 5.0, 8.0)
    if not any(item["text"] == "hello" for item in kept):
        raise SystemExit(f"boundary word dropped: {kept}")
    if speaker_owner(0.0, 0.2, []) != UNASSIGNED:
        raise SystemExit("empty audio was assigned")
    if speaker_owner(0.0, 0.4, [{"speaker": 1, "start": 0.0, "end": 0.4}]) != 1:
        raise SystemExit("overlap missed")
    if speaker_owner(0.0, 0.2, [{"speaker": 1, "start": 5.0, "end": 5.2}]) != UNASSIGNED:
        raise SystemExit("distant speaker was forced")
    if speaker_owner(0.0, 0.2, [{"speaker": 1, "start": 0.5, "end": 0.7}]) != 1:
        raise SystemExit("nearby speaker missed")
    both = speaker_owner(
        0.0,
        1.0,
        [{"speaker": 0, "start": 0.0, "end": 1.0}, {"speaker": 1, "start": 0.0, "end": 1.0}],
    )
    if both != OVERLAP:
        raise SystemExit(f"overlap collapsed to {both}")


class Engine:
    def __init__(self) -> None:
        self.model = None
        self.processor = None
        self.asr = None
        self.device = "cpu"
        self.gpu = ""
        self.asr_name = ""
        self.live: dict[str, dict] = {}

    def load(self) -> None:
        import numpy  # noqa: F401
        import torch
        from faster_whisper import WhisperModel
        from transformers import AutoModelForAudioFrameClassification, AutoProcessor

        model_dir = Path(torch.__file__).parent
        del model_dir
        diar_code = Path(__import__("transformers").__file__).parent / "models" / "nemotron3_diarization"
        if not diar_code.is_dir():
            raise RuntimeError(
                "This Python environment does not include Nemotron 3 diarization. Run Start Speaker Studio.bat again so it can update the local engine."
            )

        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        if self.device == "cuda":
            self.gpu = torch.cuda.get_device_name(0)
        dtype = torch.bfloat16 if self.device == "cuda" else torch.float32
        emit({"event": "status", "message": "Loading Nemotron 3 Diarization"})
        self.processor = AutoProcessor.from_pretrained(DIAR_ID)
        try:
            self.processor.set_streaming_mode("low_latency")
        except Exception as exc:
            log(f"streaming mode: {exc}")
        for attr in ("num_samples_first_audio_chunk", "num_samples_per_audio_chunk"):
            if not hasattr(self.processor, attr):
                raise RuntimeError("Nemotron 3 streaming is missing from this transformers build.")
        try:
            model = AutoModelForAudioFrameClassification.from_pretrained(DIAR_ID, dtype=dtype)
        except TypeError:
            model = AutoModelForAudioFrameClassification.from_pretrained(DIAR_ID, torch_dtype=dtype)
        self.model = model.to(self.device).eval()

        asr_path = find_asr()
        self.asr_name = Path(asr_path).name if os.path.isdir(asr_path) else asr_path
        emit({"event": "status", "message": "Loading Whisper"})
        try:
            self.asr = WhisperModel(asr_path, device="cuda" if self.device == "cuda" else "cpu", compute_type="float16" if self.device == "cuda" else "int8")
        except Exception as exc:
            log(f"whisper cuda fallback: {exc}")
            self.asr = WhisperModel(asr_path, device="cpu", compute_type="int8")
            if self.device == "cuda":
                self.asr_name += " (cpu)"

        emit(
            {
                "event": "ready",
                "device": self.device,
                "gpu": self.gpu,
                "diar": DIAR_ID,
                "asr": self.asr_name,
                "message": "Nemotron 3 ready",
            }
        )

    def featurize(self, wave, **kwargs):
        import numpy as np

        wave = np.asarray(wave, dtype=np.float32).reshape(-1)
        kwargs.setdefault("sampling_rate", 16000)
        try:
            return self.processor(wave, **kwargs)
        except Exception:
            return self.processor(wave[None, :], **kwargs)

    def move_inputs(self, inputs) -> dict:
        import torch

        moved = {}
        for key, value in dict(inputs).items():
            if torch.is_tensor(value):
                if value.is_floating_point():
                    moved[key] = value.to(device=self.model.device, dtype=self.model.dtype)
                else:
                    moved[key] = value.to(device=self.model.device)
            else:
                moved[key] = value
        return moved

    def segments_from_logits(self, logits, attention_mask=None) -> list[dict]:
        try:
            logits_cpu = logits.float().cpu() if hasattr(logits, "float") else logits
            mask = None
            if attention_mask is not None and hasattr(attention_mask, "cpu"):
                mask = attention_mask.cpu()
            if mask is not None:
                try:
                    raw = self.processor.extract_speaker_dict(logits_cpu, mask)[0]
                except TypeError:
                    raw = self.processor.extract_speaker_dict(logits_cpu)[0]
            else:
                raw = self.processor.extract_speaker_dict(logits_cpu)[0]
            parsed = normalize_segments(raw)
            if parsed:
                return parsed
        except Exception as exc:
            log(f"extract_speaker_dict: {exc}")
        return threshold_segments(logits)

    def transcribe(self, target, offset: float = 0.0, *, live: bool = False, prompt: str = "") -> list[dict]:
        options = {
            "word_timestamps": True,
            "beam_size": 1 if live else 5,
            "vad_filter": not live,
            "condition_on_previous_text": False,
        }
        if prompt:
            options["initial_prompt"] = prompt
        segments, _info = self.asr.transcribe(target, **options)
        words = []
        for segment in segments:
            timed = getattr(segment, "words", None) or []
            if timed:
                for word in timed:
                    text = (word.word or "").strip()
                    if text:
                        words.append({"start": float(word.start) + offset, "end": float(word.end) + offset, "text": text})
            else:
                text = (segment.text or "").strip()
                if text:
                    words.append({"start": float(segment.start) + offset, "end": float(segment.end) + offset, "text": text})
        return words

    def align(self, words: list[dict], segments: list[dict], max_speakers: int) -> list[dict]:
        usable = [segment for segment in segments if int(segment["speaker"]) < max_speakers]
        return rows_to_turns(assign_speakers(words, usable))

    def owner(self, start: float, end: float, segments: list[dict]) -> int:
        return speaker_owner(start, end, segments)

    def read_wav(self, path: str, start: float | None, end: float | None):
        import numpy as np
        import soundfile as sf

        info = sf.info(path)
        if info.samplerate != 16000:
            raise RuntimeError(f"Expected 16 kHz audio and got {info.samplerate}.")
        offset = 0.0 if not start or start < 0 else float(start)
        stop = float(info.duration) if end is None else float(end)
        if stop < offset:
            stop = offset
        a = min(int(info.frames), int(offset * 16000))
        b = min(int(info.frames), int(round(stop * 16000)))
        audio, sample_rate = sf.read(path, start=a, stop=b, dtype="float32", always_2d=False)
        audio = np.asarray(audio, dtype=np.float32)
        if audio.ndim > 1:
            audio = audio.mean(axis=1)
        if sample_rate != 16000:
            raise RuntimeError(f"Expected 16 kHz audio and got {sample_rate}.")
        return audio, offset

    def diarize_array(self, audio) -> list[dict]:
        import torch

        inputs = self.featurize(audio)
        batch = self.move_inputs(inputs)
        with torch.inference_mode():
            logits = self.model(**batch).logits
        mask = batch.get("attention_mask")
        return self.shift(self.segments_from_logits(logits, mask), 0.0)

    def shift(self, segments: list[dict], offset: float) -> list[dict]:
        if not offset:
            return segments
        return [
            {"speaker": item["speaker"], "start": round(item["start"] + offset, 3), "end": round(item["end"] + offset, 3)}
            for item in segments
        ]

    def diarize(self, cmd: dict) -> None:
        import soundfile as sf

        job = cmd["job"]
        wav = cmd["wav"]
        start = cmd.get("start")
        end = cmd.get("end")
        emit({"event": "status", "job": job, "message": "Diarizing with Nemotron 3"})
        audio, offset = self.read_wav(wav, start, end)
        if audio.size < 1600:
            payload = {"event": "result", "job": job, "segments": [], "turns": [], "message": "No speech in that range"}
            if start is not None or end is not None:
                payload["start"] = 0.0 if start is None else float(start)
                payload["end"] = float(end) if end is not None else offset
            emit(payload)
            return
        segments = self.shift(self.diarize_array(audio), offset)
        emit({"event": "status", "job": job, "message": "Transcribing"})
        full = offset <= 0.001 and end is None
        target = wav
        temp = None
        if not full:
            temp = str(Path(wav).with_suffix(".slice.wav"))
            sf.write(temp, audio, 16000)
            target = temp
        try:
            words = self.transcribe(target, 0.0 if full else offset)
        finally:
            if temp and os.path.exists(temp):
                try:
                    os.remove(temp)
                except OSError:
                    pass
        turns = self.align(words, segments, 8)
        payload = {"event": "result", "job": job, "segments": segments, "turns": turns, "message": f"{len(turns)} lines"}
        if not full:
            payload["start"] = 0.0 if start is None else float(start)
            payload["end"] = float(end) if end is not None else offset + (audio.size / 16000.0)
        emit(payload)

    def new_track(self) -> dict:
        return {
            "buf": AudioBuf(),
            "cache": None,
            "step": 0,
            "mel": None,
            "logits": [],
            "words": [],
            "committed": 0.0,
            "asr_at": 0.0,
            "shown_at": 0.0,
        }

    def live_state(self, job: str) -> dict:
        state = self.live.get(job)
        if state is None or "desktop" not in state:
            state = {"desktop": self.new_track(), "mic": self.new_track()}
            self.live[job] = state
        return state

    def forward_stream(self, state: dict, piece, first: bool, last: bool) -> None:
        import torch

        inputs = self.featurize(
            piece,
            is_streaming=True,
            is_first_audio_chunk=first,
            is_last_audio_chunk=last,
        )
        batch = self.move_inputs(inputs)
        kwargs = {}
        if state["cache"] is not None:
            kwargs["speaker_cache"] = state["cache"]
        with torch.inference_mode():
            outputs = self.model(**batch, **kwargs)
        state["cache"] = getattr(outputs, "speaker_cache", None)
        state["logits"].append(outputs.logits.detach().float().cpu())
        state["step"] += 1

    def drain_live(self, state: dict, last: bool) -> None:
        total = state["buf"].total
        if state["step"] == 0:
            need = int(self.processor.num_samples_first_audio_chunk)
            if total < need and not last:
                return
            if total <= 0:
                return
            end = need if total >= need else total
            self.forward_stream(state, state["buf"].slice(0, end), True, last and total < need)
            if total < need:
                return
            state["mel"] = int(self.processor.num_mel_frames_per_step)
        step = int(self.processor.num_samples_per_audio_chunk)
        while state["mel"] is not None:
            start = int(self.processor.audio_chunk_start(state["mel"]))
            end = start + step
            if end <= total:
                self.forward_stream(state, state["buf"].slice(start, end), False, False)
                state["mel"] += int(self.processor.num_mel_frames_per_step)
                continue
            if last and start < total:
                self.forward_stream(state, state["buf"].slice(start, total), False, True)
                state["mel"] = None
            break

    def region_loud(self, track: dict, start: float, end: float) -> bool:
        import numpy as np

        samples = track["buf"].slice(int(start * 16000), int(end * 16000))
        if samples.size < 160:
            return False
        return float(np.sqrt(np.mean(samples * samples))) >= 0.02

    def absorb_words(self, track: dict, tag: str, force: bool) -> bool:
        import numpy as np
        import soundfile as sf

        duration = track["buf"].total / 16000
        lag = 4.0
        window = 16.0
        if not force and duration - track["asr_at"] < 2.0:
            return False
        if duration < 1.0 and not force:
            return False
        start_t = max(0.0, track["committed"] - 1.0)
        end_t = duration if force else min(duration, start_t + window)
        cutoff = end_t if force else min(end_t, max(start_t, duration - lag))
        if cutoff <= start_t + 0.25:
            return False
        track["asr_at"] = duration
        clip = track["buf"].slice(int(start_t * 16000), int(end_t * 16000))
        if clip.size < 1600:
            return False
        temp = Path(os.environ.get("TEMP", ".")) / f"speaker-live-{tag}.wav"
        sf.write(temp, np.asarray(clip, dtype=np.float32), 16000)
        prompt = " ".join(word["text"] for word in track["words"][-20:])[-180:]
        try:
            words = self.transcribe(str(temp), start_t, live=True, prompt=prompt)
        finally:
            try:
                temp.unlink(missing_ok=True)
            except TypeError:
                if temp.exists():
                    temp.unlink()
        fresh = [
            word
            for word in words
            if float(word["end"]) > track["committed"] - 0.45
            and float(word["start"]) < cutoff + 0.05
            and float(word["end"]) <= cutoff + 0.05
        ]
        fresh.sort(key=lambda word: word["start"])
        cursor = track["committed"]
        retry = False
        for word in fresh:
            if word["start"] - cursor > 1.5 and self.region_loud(track, cursor, word["start"]):
                if track.get("gap_retries", 0) < 1:
                    track["gap_retries"] = track.get("gap_retries", 0) + 1
                    retry = True
                break
            cursor = max(cursor, word["end"])
        if not retry and cutoff - cursor > 1.5 and self.region_loud(track, cursor, cutoff):
            if track.get("gap_retries", 0) < 1:
                track["gap_retries"] = track.get("gap_retries", 0) + 1
                retry = True
            else:
                cursor = cutoff
        elif not retry:
            cursor = cutoff
            track["gap_retries"] = 0
        if retry:
            cursor = track["committed"]
        track["words"] = keep_recognized(track["words"], fresh, track["committed"], cursor, cutoff)
        track["committed"] = max(track["committed"], cursor)
        return True

    def mic_activity(self, track: dict) -> list[dict]:
        import numpy as np

        total = int(track["buf"].total)
        hop = 800
        resume = int(track.get("energy_sample", 0))
        if resume > total:
            resume = 0
            track["energy_segments"] = []
            track["energy_open"] = None
        samples = track["buf"].slice(resume, total)
        usable = samples.size - (samples.size % hop)
        segments = track.setdefault("energy_segments", [])
        start = track.get("energy_open")
        for index in range(0, usable, hop):
            chunk = samples[index : index + hop]
            level = float(np.sqrt(np.mean(chunk * chunk))) if chunk.size else 0.0
            time = (resume + index) / 16000.0
            next_time = (resume + index + hop) / 16000.0
            if level >= 0.015:
                if start is None:
                    start = time
            elif start is not None:
                if next_time - start >= 0.08:
                    segments.append({"speaker": 8, "start": round(start, 3), "end": round(next_time, 3)})
                start = None
        track["energy_sample"] = resume + usable
        track["energy_open"] = start
        shown = list(segments)
        if start is not None and total / 16000.0 - float(start) >= 0.08:
            shown.append({"speaker": 8, "start": round(float(start), 3), "end": round(total / 16000.0, 3)})
        return shown

    def party_segments(self, track: dict) -> list[dict]:
        import torch

        step = track["step"]
        if track.get("seg_step") == step and track.get("seg_cache") is not None:
            return track["seg_cache"]
        pending = track["logits"]
        logits = track.get("logit_cat")
        if pending:
            piece = torch.cat(pending, dim=1) if len(pending) > 1 else pending[0]
            logits = piece if logits is None else torch.cat([logits, piece], dim=1)
            track["logits"] = []
            track["logit_cat"] = logits
        if logits is None:
            segments = []
        else:
            segments = [segment for segment in self.segments_from_logits(logits) if int(segment["speaker"]) < 8]
        track["seg_cache"] = segments
        track["seg_step"] = step
        return segments

    def turns_for_speaker(self, words: list[dict], speaker: int) -> list[dict]:
        turns = []
        for word in words:
            text = word["text"].strip()
            if not text:
                continue
            if turns and word["start"] - turns[-1]["end"] < 0.85:
                turns[-1]["end"] = round(float(word["end"]), 3)
                turns[-1]["text"] = f"{turns[-1]['text']} {text}".strip()
            else:
                turns.append({"speaker": speaker, "start": round(float(word["start"]), 3), "end": round(float(word["end"]), 3), "text": text})
        return turns

    def publish_merged(self, job: str) -> None:
        state = self.live[job]
        desktop = state["desktop"]
        mic = state["mic"]
        party = self.party_segments(desktop)
        segments = list(party)
        segments.extend(self.mic_activity(mic))
        duration = max(desktop["buf"].total, mic["buf"].total) / 16000.0
        edge = max(0.0, duration - LIVE_RETAIN)
        prior = [row for row in desktop.get("assigned", []) if float(row["end"]) < edge]
        recent = [word for word in desktop["words"] if float(word["end"]) >= edge]
        recent_segments = [segment for segment in party if float(segment["end"]) >= edge - 1.0]
        prior.extend(assign_speakers(recent, recent_segments))
        desktop["assigned"] = prior
        turns = rows_to_turns(prior)
        turns.extend(self.turns_for_speaker(mic["words"], 8))
        turns.sort(key=lambda turn: (turn["start"], turn["speaker"]))
        emit({"event": "live", "job": job, "segments": segments, "turns": turns, "duration": round(duration, 3)})

    def mic_pad(self, cmd: dict) -> None:
        import numpy as np

        samples = int(round(float(cmd.get("seconds") or 0.0) * 16000))
        if samples <= 0:
            return
        state = self.live.get(cmd.get("job"))
        if state is None or "mic" not in state:
            return
        state["mic"]["buf"].append(np.zeros(samples, dtype=np.float32))

    def live_pcm(self, cmd: dict) -> None:
        import base64
        import numpy as np

        job = cmd["job"]
        source = cmd.get("source") or "desktop"
        raw = base64.b64decode(cmd.get("pcm") or "")
        if len(raw) < 2:
            return
        pcm = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
        state = self.live_state(job)
        track = state["mic"] if source == "mic" else state["desktop"]
        track["buf"].append(pcm)
        duration = track["buf"].total / 16000
        if source == "mic":
            self.absorb_words(track, f"{job}-mic", False)
            if duration - track["shown_at"] >= 1.0:
                track["shown_at"] = duration
                self.publish_merged(job)
            return
        before = track["step"]
        self.drain_live(track, False)
        self.absorb_words(track, f"{job}-desktop", False)
        if track["step"] != before or duration - track["shown_at"] >= 1.0:
            track["shown_at"] = duration
            self.publish_merged(job)

    def live_stop(self, cmd: dict) -> None:
        job = cmd["job"]
        state = self.live.get(job)
        if state is not None and "desktop" in state:
            self.drain_live(state["desktop"], True)
            self.absorb_words(state["desktop"], f"{job}-desktop", True)
            self.absorb_words(state["mic"], f"{job}-mic", True)
            self.publish_merged(job)
            self.live.pop(job, None)
        emit({"event": "live_done", "job": job})

    def refine_sources(self, cmd: dict) -> None:
        job = cmd["job"]
        emit({"event": "status", "job": job, "message": "Separating you from the party"})
        segments = []
        turns = []
        desktop = cmd.get("desktop") or ""
        mic = cmd.get("mic") or ""
        if desktop and os.path.exists(desktop) and os.path.getsize(desktop) > 1000:
            audio, _offset = self.read_wav(desktop, None, None)
            if audio.size >= 1600:
                segments = [segment for segment in self.diarize_array(audio) if segment["speaker"] < 8]
                turns = self.align(self.transcribe(desktop, 0.0), segments, 8)
        if mic and os.path.exists(mic) and os.path.getsize(mic) > 1000:
            words = self.transcribe(mic, 0.0)
            mic_turns = self.turns_for_speaker(words, 8)
            segments.extend({"speaker": 8, "start": turn["start"], "end": turn["end"]} for turn in mic_turns)
            turns.extend(mic_turns)
        turns.sort(key=lambda turn: (turn["start"], turn["speaker"]))
        emit({"event": "result", "job": job, "segments": segments, "turns": turns, "message": f"{len(turns)} lines"})

    def handle(self, cmd: dict) -> None:
        name = cmd.get("cmd")
        if name == "diarize":
            self.diarize(cmd)
        elif name == "live_start":
            self.live[cmd["job"]] = {"desktop": self.new_track(), "mic": self.new_track()}
            emit({"event": "status", "job": cmd["job"], "message": "Listening"})
        elif name == "live_pcm":
            self.live_pcm(cmd)
        elif name == "live_stop":
            self.live_stop(cmd)
        elif name == "mic_enable":
            return
        elif name == "mic_pad":
            self.mic_pad(cmd)
        elif name == "refine_sources":
            self.refine_sources(cmd)
        elif name == "shutdown":
            raise SystemExit(0)
        else:
            emit({"event": "error", "message": f"Unknown command {name}"})


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8", line_buffering=True)
    sys.stderr.reconfigure(encoding="utf-8")
    engine = Engine()
    try:
        engine.load()
    except Exception as exc:
        emit({"event": "error", "message": str(exc)})
        log(traceback.format_exc())
        return
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            cmd = json.loads(line)
            engine.handle(cmd)
        except SystemExit:
            break
        except Exception as exc:
            job = None
            try:
                job = json.loads(line).get("job")
            except Exception:
                pass
            emit({"event": "error", "job": job, "message": str(exc)})
            log(traceback.format_exc())


if __name__ == "__main__":
    main()
