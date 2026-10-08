# CLAP model and reference fixtures

The application runs the pinned [Xenova/clap-htsat-unfused](https://huggingface.co/Xenova/clap-htsat-unfused) ONNX conversion locally. `clap-manifest.json` records every asset's size, SHA-256 and revision. The upstream [LAION model](https://huggingface.co/laion/clap-htsat-unfused) is Apache-2.0 licensed. Model weights are downloaded into application data, not committed here. No audio is sent to the model host.

`prepare.py` is an optional developer downloader, equivalent to the application's Enable button:

```sh
python3 tools/models/prepare.py "$HOME/Library/Application Support/studio.poti.sampler/models/clap-htsat-unfused-c28f288"
```

The Rust processor uses mono 48 kHz, a 10-second window, repeatpad for short inputs, periodic Hann FFT 1024 / hop 480, centered reflection padding, 64 Slaney-normalized mel filters from 50 to 14,000 Hz, and log-power decibels. These follow the pinned [Transformers 4.57.3 CLAP processor](https://github.com/huggingface/transformers/blob/v4.57.3/src/transformers/models/clap/feature_extraction_clap.py). For reproducibility, long recordings use the first 10 seconds; the model identity includes this crop policy.

`reference.py` synthesizes a decaying kick, a noise burst, a tone, and silence. It compares the original pinned PyTorch model with ONNX, then saves PCM, log-mel features and original embeddings to `crates/embed/tests/fixtures`. These generated sounds have no third-party audio content. Regenerate with a development-only Python environment:

```sh
uv venv --python 3.12 tools/models/.venv
uv pip install --python tools/models/.venv/bin/python -r tools/models/requirements-reference.txt
tools/models/.venv/bin/python tools/models/reference.py /absolute/path/to/model-directory
```

Routine Rust tests compare the log-mel fixtures without downloading weights. The integration test also compares normalized audio/text vectors with the original model; each must have cosine similarity at least 0.99:

```sh
SAMPLER_MODEL_DIR=/absolute/path/to/model-directory cargo test -p sampler-embed -- --include-ignored --nocapture
SAMPLER_TEST_ACCELERATION=1 SAMPLER_MODEL_DIR=/absolute/path/to/model-directory cargo test -p sampler-embed --test reference rust_audio -- --ignored --nocapture
```

The second command requests CoreML on macOS or DirectML on Windows, with ONNX Runtime CPU fallback. The application links the ONNX Runtime supplied by `ort-sys`. It does not need Python, PyTorch or a separately installed ONNX Runtime.
