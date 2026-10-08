"""Generate synthetic CLAP fixtures and compare the original model with ONNX."""

import argparse
import json
from pathlib import Path
import numpy as np
import onnxruntime as ort
import torch
from transformers import ClapModel, ClapProcessor

parser = argparse.ArgumentParser()
parser.add_argument("model_dir", type=Path)
parser.add_argument("--output", type=Path, default=Path("crates/embed/tests/fixtures"))
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
torch.set_num_threads(2)
revision = "8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a"
processor = ClapProcessor.from_pretrained("laion/clap-htsat-unfused", revision=revision)
model = ClapModel.from_pretrained("laion/clap-htsat-unfused", revision=revision).eval()
options = ort.SessionOptions()
options.intra_op_num_threads = 2
options.inter_op_num_threads = 1
audio_model = ort.InferenceSession(
    str(args.model_dir / "onnx/audio_model.onnx"),
    options,
    providers=["CPUExecutionProvider"],
)
text_model = ort.InferenceSession(
    str(args.model_dir / "onnx/text_model.onnx"),
    options,
    providers=["CPUExecutionProvider"],
)
print(
    "Audio inputs",
    [(i.name, i.shape, i.type) for i in audio_model.get_inputs()],
    flush=True,
)
print(
    "Text inputs",
    [(i.name, i.shape, i.type) for i in text_model.get_inputs()],
    flush=True,
)
rng = np.random.default_rng(810)
t = np.arange(5760, dtype=np.float32) / 48000
fixtures = {
    "short_kick": (np.sin(2 * np.pi * (95 * t - 130 * t * t)) * np.exp(-32 * t)).astype(
        np.float32
    ),
    "noise_hit": (
        rng.normal(size=16800) * np.exp(-np.arange(16800) / 2400) * 0.2
    ).astype(np.float32),
    "tone": (np.sin(2 * np.pi * 440 * np.arange(57600) / 48000) * 0.3).astype(
        np.float32
    ),
    "silence": np.zeros(12000, dtype=np.float32),
}


def cosine(a, b):
    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b)))


records = []
with torch.no_grad():
    for name, waveform in fixtures.items():
        values = processor(audios=waveform, sampling_rate=48000, return_tensors="pt")
        reference = model.get_audio_features(**values).cpu().numpy()[0]
        feeds = {i.name: values[i.name].cpu().numpy() for i in audio_model.get_inputs()}
        converted = audio_model.run(["audio_embeds"], feeds)[0][0]
        score = cosine(reference, converted)
        print(name, "ONNX/reference cosine", score, flush=True)
        assert score >= 0.99
        waveform.astype("<f4").tofile(args.output / f"{name}.pcm")
        values["input_features"].numpy().astype("<f4").tofile(
            args.output / f"{name}.mel"
        )
        records.append(
            {"name": name, "embedding": reference.tolist(), "onnx_cosine": score}
        )
    texts = [
        "a deep punchy kick drum",
        "a bright metallic hi hat",
        "soft rain and wind",
    ]
    for text in texts:
        values = processor(text=text, return_tensors="pt", padding=True)
        reference = model.get_text_features(**values).cpu().numpy()[0]
        converted = text_model.run(
            ["text_embeds"],
            {i.name: values[i.name].numpy() for i in text_model.get_inputs()},
        )[0][0]
        score = cosine(reference, converted)
        print(text, "ONNX/reference cosine", score, flush=True)
        assert score >= 0.99
        records.append(
            {"text": text, "embedding": reference.tolist(), "onnx_cosine": score}
        )
(args.output / "reference.json").write_text(
    json.dumps(
        {
            "model": "laion/clap-htsat-unfused",
            "revision": revision,
            "transformers": "4.57.3",
            "records": records,
        },
        indent=2,
    )
    + "\n"
)
