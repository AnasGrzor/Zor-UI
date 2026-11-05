"""Speech to text for the primary buddy: `end4_bridge.py buddy-listen [--wav file] [--model dir] [--mic name]`.

Records from the default microphone until you stop talking (or 12 s), then transcribes it on this PC
with the Parakeet model that Orca downloads (sherpa-onnx). Prints JSON lines:
  {"state": "starting"} (mic opening), {"state": "listening"} (mic open) then {"state": "thinking"} then {"text": "..."} or {"error": "..."}.
The model folder is --model (config.json buddies.model), or Orca's own copy under %APPDATA%\\orca\\speech-models.
"""
import json
import os
import sys
import threading
import wave

RATE = 16000
MODEL = "parakeet-tdt-0.6b-v3-int8"


def out(o):
    print(json.dumps(o, ensure_ascii=False), flush=True)


def model_dir(cfg_path=""):
    for d in (cfg_path, os.path.join(os.environ.get("APPDATA", ""), "orca", "speech-models", MODEL)):
        if d and os.path.isfile(os.path.join(d, "tokens.txt")):
            return d
    return ""


def recognizer(d):
    import sherpa_onnx
    return sherpa_onnx.OfflineRecognizer.from_transducer(
        encoder=os.path.join(d, "encoder.int8.onnx"), decoder=os.path.join(d, "decoder.int8.onnx"),
        joiner=os.path.join(d, "joiner.int8.onnx"), tokens=os.path.join(d, "tokens.txt"),
        num_threads=4, sample_rate=RATE, feature_dim=128, model_type="nemo_transducer")


def read_wav(path):
    import numpy as np
    with wave.open(path) as w:
        ch, rate = w.getnchannels(), w.getframerate()
        a = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
    a = a.reshape(-1, ch).mean(axis=1)
    if rate != RATE:  # linear resample, good enough for speech
        a = np.interp(np.linspace(0, len(a) - 1, int(len(a) * RATE / rate)), np.arange(len(a)), a).astype(np.float32)
    return a


def pick_mic(devices, name):
    """Index of the first input device whose name contains `name` (any case); None for the Windows default."""
    n = name.strip().lower()
    return next((i for i, d in enumerate(devices) if n and d["max_input_channels"] > 0 and n in d["name"].lower()), None) if n else None


def threshold(noise):
    """Loudness that counts as speech: 3x the quietest of the first chunks (the mic's own noise), at least 0.006."""
    # capped: starting to talk during the first 0.3 s must not raise the bar above speech itself, but a
    # steadily noisy room (floor above 0.02) still gets a bar above its own noise
    if not noise:
        return 0.015
    low = min(noise)
    return min(max(0.03, 1.5 * low), max(0.006, 3 * low))


def record(wait=6.0, quiet=1.0, longest=12.0, mic="", on_open=None):
    """Mono 16 kHz float32 audio: waits up to `wait` s for speech, stops after `quiet` s of silence.
    `on_open` is called once the microphone is open. The chunk before the first loud one is kept so word starts survive."""
    import numpy as np
    import sounddevice as sd
    chunk = RATE // 10
    got, silent, heard, n, noise, level, prev = [], 0.0, False, 0, [], 0.015, None
    with sd.InputStream(device=pick_mic(sd.query_devices(), mic), samplerate=RATE, channels=1, dtype="float32") as stream:
        if on_open:
            on_open()
        while True:
            a = stream.read(chunk)[0][:, 0]
            n += 1
            rms = float(np.sqrt(np.mean(a * a)))
            if len(noise) < 3:  # the first 0.3 s measures the mic's noise floor
                noise.append(rms)
                level = threshold(noise)
            out({"level": round(rms, 4)})  # the pill's waveform
            loud = rms > level and len(noise) >= 3
            if loud:
                if not heard and prev is not None:
                    got.append(prev)
                heard, silent = True, 0.0
            elif heard:
                silent += 0.1
            if heard:
                got.append(a)
            prev = a
            if (not heard and n * 0.1 >= wait) or (heard and (silent >= quiet or len(got) * 0.1 >= longest)):
                break
    return np.concatenate(got) if got else None


def main(argv):
    wav = argv[argv.index("--wav") + 1] if "--wav" in argv else ""
    d = model_dir(argv[argv.index("--model") + 1] if "--model" in argv else "")
    if not d:
        return out({"error": "Parakeet model not found. Open Orca once and enable voice dictation, or set buddies.model."})
    try:
        out({"state": "starting"})
        # the model takes seconds to load: do it while the microphone is already recording
        box = {}

        def load():
            try:
                box["rec"] = recognizer(d)
            except Exception as e:
                box["err"] = e
        loader = threading.Thread(target=load, daemon=True)
        loader.start()
        if wav:
            out({"state": "listening"})
        audio = read_wav(wav) if wav else record(mic=argv[argv.index("--mic") + 1] if "--mic" in argv else "", on_open=lambda: out({"state": "listening"}))
        if audio is None:
            return out({"text": ""})
        out({"state": "thinking"})
        loader.join()
        if "err" in box:
            raise box["err"]
        rec = box["rec"]
        s = rec.create_stream()
        s.accept_waveform(RATE, audio)
        rec.decode_stream(s)
        out({"text": s.result.text.strip()})
    except Exception as e:  # missing mic, missing package
        out({"error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main(sys.argv[1:])
