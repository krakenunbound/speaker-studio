"""CPU-only engine regressions; no downloaded models or audio devices required."""

import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import numpy as np


spec = importlib.util.spec_from_file_location("speech_engine", Path(__file__).with_name("engine.py"))
engine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)


def word(start, end, text="hello"):
    return {"start": start, "end": end, "text": text}


def full_segments(logits):
    """Independent whole-recording threshold oracle using integer frame counts."""
    rows = []
    for speaker in range(logits.shape[1]):
        start = None
        for frame in range(len(logits) + 1):
            active = frame < len(logits) and logits[frame, speaker] > 0
            if active and start is None:
                start = frame
            if not active and start is not None:
                if frame - start >= 5:
                    rows.append({"speaker": speaker, "start": round(start * .01, 3), "end": round(frame * .01, 3)})
                start = None
    return sorted(rows, key=lambda row: (row["start"], row["speaker"]))


class AudioBufferTests(unittest.TestCase):
    def test_absolute_slices_survive_partial_and_whole_block_trimming(self):
        buf = engine.AudioBuf()
        buf.append(np.arange(7, dtype=np.float32))
        buf.append(np.arange(7, 15, dtype=np.float32))
        buf.discard_before(5)
        np.testing.assert_array_equal(buf.slice(5, 13), np.arange(5, 13))
        self.assertEqual(buf.total, 15)
        self.assertEqual(buf.base, 5)
        with self.assertRaises(ValueError):
            buf.slice(4, 8)
        buf.discard_before(15)
        buf.append(np.arange(15, 18, dtype=np.float32))
        np.testing.assert_array_equal(buf.slice(15, 18), np.arange(15, 18))

    def test_long_microphone_retains_only_pending_asr_and_context(self):
        worker = engine.Engine()
        track = worker.new_track()
        for _ in range(1200):
            track["buf"].append(np.zeros(1600, dtype=np.float32))
            track["committed"] = max(0., track["buf"].total / 16000 - 4.)
            track["energy_sample"] = track["buf"].total
            worker.trim_track(track, mic=True)
        self.assertEqual(track["buf"].total, 120 * 16000)
        self.assertLessEqual(sum(map(len, track["buf"].blocks)), 5 * 16000)
        self.assertEqual(len(track["buf"].slice(115 * 16000, 120 * 16000)), 5 * 16000)

    def test_desktop_preserves_next_model_chunk_when_asr_is_ahead(self):
        worker = engine.Engine()
        worker.processor = SimpleNamespace(audio_chunk_start=lambda frame: frame * 160 - 256)
        track = worker.new_track()
        track["buf"].append(np.zeros(10 * 16000, dtype=np.float32))
        track.update(step=1, mel=72, committed=9.)
        worker.trim_track(track, mic=False)
        self.assertEqual(track["buf"].base, 72 * 160 - 256)


class RecognitionTests(unittest.TestCase):
    def test_commit_boundary_word_is_not_deleted(self):
        crossing = word(4.4, 4.8)
        self.assertEqual(engine.keep_recognized([crossing], [crossing], 5., 6., 6.), [crossing])

    def test_forced_tail_does_not_defer_words_to_a_nonexistent_retry(self):
        worker = engine.Engine()
        track = worker.new_track()
        track["buf"].append(np.full(3 * 16000, .1, dtype=np.float32))
        tail = word(2.5, 2.8, "last words")
        worker.transcribe = lambda audio, offset, **kwargs: [tail]
        worker.absorb_words(track, "test", True)
        self.assertEqual(track["words"], [tail])
        self.assertEqual(track["committed"], 3.)

    def test_late_microphone_skips_silence_and_preserves_prior_tail(self):
        worker = engine.Engine()
        state = worker.live_state("test")
        track = state["mic"]
        track["buf"].append(np.full(3 * 16000, .1, dtype=np.float32))
        calls = []
        def recognize(audio, offset, **kwargs):
            self.assertIsInstance(audio, np.ndarray)
            calls.append((offset, len(audio) / 16000))
            return [word(2.5, 2.8, "before mute")] if offset < 3 else [word(303.2, 303.6, "after mute")]
        worker.transcribe = recognize
        worker.mic_pad({"job": "test", "seconds": 300.})
        self.assertEqual(track["committed"], 303.)
        self.assertEqual(track["words"][0]["text"], "before mute")
        self.assertEqual(sum(map(len, track["buf"].blocks)), 16000)
        track["buf"].append(np.zeros(6 * 16000, dtype=np.float32))
        worker.absorb_words(track, "test", False)
        self.assertEqual(calls, [(0., 3.), (302., 7.)])
        self.assertEqual([item["text"] for item in track["words"]], ["before mute", "after mute"])

    def test_empty_microphone_padding_does_not_invoke_asr(self):
        worker = engine.Engine()
        track = worker.live_state("test")["mic"]
        worker.transcribe = lambda *args, **kwargs: self.fail("ASR should not run on the synthetic gap")
        worker.mic_pad({"job": "test", "seconds": 600.})
        self.assertEqual(track["committed"], 600.)
        self.assertEqual(track["buf"].base, 599 * 16000)

    def test_late_old_words_and_revisions_are_not_lost_from_publication(self):
        worker = engine.Engine()
        state = worker.live_state("test")
        desktop = state["desktop"]
        desktop["buf"].total = 30 * 16000
        state["mic"]["buf"].total = 90 * 16000
        worker.party_segments = lambda track: [{"speaker": 1, "start": 0., "end": 30.}]
        worker.mic_activity = lambda track: []
        desktop["words"] = [word(5., 5.4, "old"), word(25., 25.4, "recent")]
        with patch.object(engine, "emit") as emit:
            worker.publish_merged("test")
            self.assertEqual([row["text"] for row in emit.call_args.args[0]["turns"]], ["old", "recent"])
            desktop["words"] = [word(5., 5.4, "corrected"), word(8., 8.4, "late")]
            worker.publish_merged("test")
            self.assertEqual([row["text"] for row in emit.call_args.args[0]["turns"]], ["corrected", "late"])
            self.assertEqual(len(desktop["assigned_cache"]), 2)


class DiarizationTests(unittest.TestCase):
    def test_stream_segments_match_whole_recording_across_chunk_boundaries(self):
        worker = engine.Engine()
        track = worker.new_track()
        rng = np.random.default_rng(48)
        logits = np.repeat(rng.choice([-2., 0., 2.], size=(100, 8)), 7, axis=0)
        position = 0
        for size in [1, 3, 4, 71, 2, 13, 100, 6, 49, 451]:
            end = min(len(logits), position + size)
            track["logits"].append(logits[None, position:end])
            actual = worker.party_segments(track)
            self.assertEqual(actual, full_segments(logits[:end]))
            self.assertEqual(track["logits"], [])
            self.assertNotIn("logit_cat", track)
            position = end
        self.assertEqual(position, len(logits))

    def test_open_short_run_is_retained_until_it_reaches_minimum_duration(self):
        worker = engine.Engine()
        track = worker.new_track()
        track["logits"] = [np.ones((1, 3, 1))]
        self.assertEqual(worker.party_segments(track), [])
        track["logits"] = [np.ones((1, 3, 1))]
        self.assertEqual(worker.party_segments(track), [{"speaker": 0, "start": 0., "end": .06}])
        track["logits"] = [-np.ones((1, 3, 1))]
        self.assertEqual(worker.party_segments(track), [{"speaker": 0, "start": 0., "end": .06}])

    def test_stream_ignores_masked_padding(self):
        import torch
        worker = engine.Engine()
        track = worker.new_track()
        worker.featurize = lambda *args, **kwargs: {"attention_mask": torch.tensor([[1] * 6 + [0] * 4])}
        worker.move_inputs = lambda inputs: inputs
        worker.model = lambda **kwargs: SimpleNamespace(logits=torch.ones(1, 10, 1), speaker_cache=None)
        worker.forward_stream(track, np.zeros(1600), True, True)
        self.assertEqual(worker.party_segments(track), [{"speaker": 0, "start": 0., "end": .06}])

    def test_adjacent_speakers_are_ambiguous_not_simultaneous(self):
        segments = [{"speaker": 0, "start": 0., "end": 1.}, {"speaker": 1, "start": 1., "end": 2.}]
        self.assertEqual(engine.speaker_owner(.9, 1.1, segments), engine.UNASSIGNED)
        self.assertEqual(engine.speaker_owner(0., 1., [dict(row, start=0., end=1.) for row in segments]), engine.OVERLAP)


if __name__ == "__main__":
    unittest.main()
