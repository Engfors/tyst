"""ASR engines, VAD and language ID on top of sherpa-onnx (primary) and onnx-asr (fallback).

The sherpa-onnx path mirrors what the Rust implementation will do through sherpa-onnx's C API,
so numbers measured here should be reproducible in Phase 1.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from math import gcd
from pathlib import Path

import numpy as np

from .config import ModelSpec

SAMPLE_RATE = 16_000


# --------------------------------------------------------------------------- audio


def load_audio(path: Path) -> np.ndarray:
    """Any soundfile-readable file -> mono float32 at 16 kHz."""
    import soundfile as sf
    from scipy.signal import resample_poly

    samples, sr = sf.read(str(path), dtype="float32", always_2d=True)
    mono = samples.mean(axis=1)
    if sr != SAMPLE_RATE:
        g = gcd(sr, SAMPLE_RATE)
        mono = resample_poly(mono, SAMPLE_RATE // g, sr // g).astype(np.float32)
    return np.ascontiguousarray(mono, dtype=np.float32)


# --------------------------------------------------------------------------- results


@dataclass
class AsrResult:
    text: str
    tokens: list[str] = field(default_factory=list)
    logprobs: list[float] = field(default_factory=list)
    seconds: float = 0.0  # wall-clock decode time

    @property
    def confidence(self) -> float:
        """Length-normalized mean token log-probability (SPEC 6.3 strategy 2). 0 is best."""
        if not self.logprobs:
            return float("-inf")
        return float(np.mean(self.logprobs))


# --------------------------------------------------------------------------- model files


def _pick(directory: Path, *patterns: str) -> Path | None:
    """First match, preferring int8 files."""
    for pattern in patterns:
        matches = sorted(directory.glob(pattern), key=lambda p: ("int8" not in p.name, p.name))
        if matches:
            return matches[0]
    return None


def detect_layout(directory: Path) -> str:
    """'sherpa' (encoder/decoder/joiner + tokens.txt) or 'onnx-asr' (encoder-model + decoder_joint-model)."""
    if _pick(directory, "decoder_joint-model*.onnx") and _pick(directory, "encoder-model*.onnx"):
        return "onnx-asr"
    if _pick(directory, "*joiner*.onnx") and (directory / "tokens.txt").is_file():
        return "sherpa"
    raise FileNotFoundError(f"unrecognized model layout in {directory}: {sorted(p.name for p in directory.iterdir())}")


def write_bpe_vocab(tokens_txt: Path, out: Path) -> Path:
    """sherpa-onnx hotwords need a sentencepiece-style vocab to tokenize hotword phrases.

    NeMo exports ship only tokens.txt, so derive one. Scores fall with token id, which makes the
    greedy tokenizer prefer the earlier (more frequent) pieces, close enough for short phrases.
    """
    lines = []
    for line in tokens_txt.read_text(encoding="utf-8").splitlines():
        tok, idx = line.rsplit(" ", 1)
        lines.append(f"{tok}\t{-int(idx) / 1000.0}")
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return out


# --------------------------------------------------------------------------- ASR


@dataclass
class DecodeConfig:
    name: str = "greedy"  # "greedy" | "beam" | "hotwords"
    hotwords: list[str] = field(default_factory=list)
    hotwords_score: float = 1.5
    max_active_paths: int = 4


class AsrEngine:
    def __init__(self, spec: ModelSpec, decode: DecodeConfig, num_threads: int, provider: str, work_dir: Path):
        self.spec = spec
        self.decode_cfg = decode
        self.layout = detect_layout(spec.path)
        self.backend = self.layout
        t0 = time.perf_counter()
        if self.layout == "sherpa":
            self._load_sherpa(decode, num_threads, provider, work_dir)
        else:
            if decode.name != "greedy":
                raise ValueError(f"{spec.id}: decode '{decode.name}' is only supported on the sherpa-onnx backend")
            self._load_onnx_asr(num_threads, provider)
        self.load_seconds = time.perf_counter() - t0

    @property
    def id(self) -> str:
        return f"{self.spec.id}/{self.decode_cfg.name}"

    def _load_sherpa(self, decode: DecodeConfig, num_threads: int, provider: str, work_dir: Path) -> None:
        import sherpa_onnx

        d = self.spec.path
        f = self.spec.files
        kwargs = dict(
            encoder=str(d / f.get("encoder", _pick(d, "encoder*.onnx").name)),
            decoder=str(d / f.get("decoder", _pick(d, "decoder*.onnx").name)),
            joiner=str(d / f.get("joiner", _pick(d, "joiner*.onnx").name)),
            tokens=str(d / "tokens.txt"),
            model_type="nemo_transducer",
            num_threads=num_threads,
            provider=provider,
            decoding_method="greedy_search",
        )
        if decode.name in ("beam", "hotwords"):
            kwargs.update(decoding_method="modified_beam_search", max_active_paths=decode.max_active_paths)
        if decode.name == "hotwords":
            work_dir.mkdir(parents=True, exist_ok=True)
            hw = work_dir / f"{self.spec.id}.hotwords.txt"
            hw.write_text("\n".join(decode.hotwords) + "\n", encoding="utf-8")
            vocab = write_bpe_vocab(d / "tokens.txt", work_dir / f"{self.spec.id}.bpe.vocab")
            kwargs.update(
                hotwords_file=str(hw),
                hotwords_score=decode.hotwords_score,
                modeling_unit="bpe",
                bpe_vocab=str(vocab),
            )
        self._rec = sherpa_onnx.OfflineRecognizer.from_transducer(**kwargs)

    def _load_onnx_asr(self, num_threads: int, provider: str) -> None:
        import onnx_asr
        import onnxruntime as rt

        opts = rt.SessionOptions()
        opts.intra_op_num_threads = num_threads
        providers = {"cpu": ["CPUExecutionProvider"], "cuda": ["CUDAExecutionProvider"], "coreml": ["CoreMLExecutionProvider"]}[provider]
        quant = "int8" if _pick(self.spec.path, "encoder-model.int8.onnx") else None
        model = onnx_asr.load_model("nemo-conformer-tdt", self.spec.path, quantization=quant, sess_options=opts, providers=providers)
        self._oa = model.with_timestamps()

    def transcribe(self, pcm: np.ndarray) -> AsrResult:
        t0 = time.perf_counter()
        if self.layout == "sherpa":
            stream = self._rec.create_stream()
            stream.accept_waveform(SAMPLE_RATE, pcm)
            self._rec.decode_stream(stream)
            r = stream.result
            res = AsrResult(text=r.text.strip(), tokens=list(r.tokens), logprobs=list(r.ys_log_probs or []))
        else:
            r = self._oa.recognize(pcm, sample_rate=SAMPLE_RATE)
            res = AsrResult(text=r.text.strip(), tokens=list(r.tokens or []), logprobs=list(r.logprobs or []))
        res.seconds = time.perf_counter() - t0
        return res


# --------------------------------------------------------------------------- VAD


@dataclass
class SegmentationParams:
    """SPEC 6.2 defaults. Decided in ADR 0002 after Phase 0."""

    threshold: float = 0.5
    min_silence: float = 0.5  # end of speech
    min_speech: float = 0.25
    max_speech: float = 15.0  # force cut
    drop_shorter_than: float = 0.3


@dataclass
class SpeechSegment:
    start: float
    end: float
    samples: np.ndarray

    @property
    def duration(self) -> float:
        return self.end - self.start


def vad_segments(pcm: np.ndarray, vad_model: Path, params: SegmentationParams) -> list[SpeechSegment]:
    import sherpa_onnx

    cfg = sherpa_onnx.VadModelConfig()
    cfg.silero_vad.model = str(vad_model)
    cfg.silero_vad.threshold = params.threshold
    cfg.silero_vad.min_silence_duration = params.min_silence
    cfg.silero_vad.min_speech_duration = params.min_speech
    cfg.silero_vad.max_speech_duration = params.max_speech
    cfg.sample_rate = SAMPLE_RATE
    window = cfg.silero_vad.window_size
    vad = sherpa_onnx.VoiceActivityDetector(cfg, buffer_size_in_seconds=max(60, int(len(pcm) / SAMPLE_RATE) + 5))

    out: list[SpeechSegment] = []

    def drain() -> None:
        while not vad.empty():
            seg = vad.front
            samples = np.asarray(seg.samples, dtype=np.float32)
            start = seg.start / SAMPLE_RATE
            out.append(SpeechSegment(start, start + len(samples) / SAMPLE_RATE, samples))
            vad.pop()

    for i in range(0, len(pcm), window):
        vad.accept_waveform(pcm[i : i + window])
        drain()
    vad.flush()
    drain()
    return [s for s in out if s.duration >= params.drop_shorter_than]


# --------------------------------------------------------------------------- language ID


class WhisperLid:
    """sherpa-onnx spoken-language-ID with a Whisper multilingual encoder/decoder."""

    def __init__(self, spec: ModelSpec, num_threads: int, provider: str):
        import sherpa_onnx

        d = spec.path
        size = spec.files.get("size", "tiny")
        cfg = sherpa_onnx.SpokenLanguageIdentificationConfig(
            whisper=sherpa_onnx.SpokenLanguageIdentificationWhisperConfig(
                encoder=str(_pick(d, f"{size}-encoder.int8.onnx", f"{size}-encoder.onnx")),
                decoder=str(_pick(d, f"{size}-decoder.int8.onnx", f"{size}-decoder.onnx")),
            ),
            num_threads=num_threads,
            provider=provider,
        )
        self.id = spec.id
        self._slid = sherpa_onnx.SpokenLanguageIdentification(cfg)

    def detect(self, pcm: np.ndarray) -> tuple[str, float]:
        t0 = time.perf_counter()
        stream = self._slid.create_stream()
        stream.accept_waveform(SAMPLE_RATE, pcm)
        lang = self._slid.compute(stream)
        return lang, time.perf_counter() - t0
