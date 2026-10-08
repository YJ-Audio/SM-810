"""Compare Rust and Python UMAP on stored vectors without opening source audio."""

import argparse
import json
from pathlib import Path
import sqlite3
import struct
import subprocess
import time
import numpy as np
import umap

parser = argparse.ArgumentParser()
parser.add_argument("database", type=Path)
parser.add_argument("--rust", type=Path, default=Path("target/release/sampler-layout"))
parser.add_argument("--output", type=Path, default=Path("tools/layout/.cache"))
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
connection = sqlite3.connect(args.database.resolve().as_uri() + "?mode=ro", uri=True)
model = connection.execute(
    "SELECT model FROM embeddings GROUP BY model ORDER BY count(*) DESC LIMIT 1"
).fetchone()[0]
records = connection.execute(
    "SELECT sample_id,dim,vec FROM embeddings WHERE model=? ORDER BY sample_id",
    (model,),
).fetchall()
ids = np.array([row[0] for row in records], dtype="<i8")
matrix = np.stack([np.frombuffer(row[2], dtype="<f4") for row in records])
matrix /= np.linalg.norm(matrix, axis=1, keepdims=True)
n, dimensions = matrix.shape
input_path = args.output / "input.bin"
with input_path.open("wb") as file:
    file.write(b"SMLYT001" + struct.pack("<II", n, dimensions))
    for sample, vector in zip(ids, matrix):
        file.write(struct.pack("<q", sample) + vector.astype("<f4").tobytes())
start = time.perf_counter()
subprocess.run(
    [str(args.rust.resolve()), str(input_path), str(args.output / "rust.bin")],
    check=True,
)
rust_seconds = time.perf_counter() - start
output = (args.output / "rust.bin").read_bytes()
rust_xy = np.array(
    [struct.unpack_from("<ff", output, 16 + i * 16 + 8) for i in range(n)],
    dtype=np.float32,
)
mask = (1 << 64) - 1


def random(sample, salt):
    value = (int(sample) + salt + 0x9E3779B97F4A7C15) & mask
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & mask
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & mask
    return ((value ^ (value >> 31)) >> 40) / (1 << 24)


initial = np.array(
    [
        [(0.1 + random(sample, salt) * 0.8 - 0.5) * 20 for salt in (0, 810)]
        for sample in ids
    ],
    dtype=np.float32,
)
distances = np.sqrt(np.maximum(0, 2 * (1 - matrix @ matrix.T)))
k = min(15, n - 1)
np.fill_diagonal(distances, -1)
indices = np.argsort(distances, axis=1)[:, :k].astype(np.int32)
knn_distances = np.take_along_axis(distances, indices, axis=1)
knn_distances[:, 0] = 0
start = time.perf_counter()
python_xy = umap.UMAP(
    n_neighbors=k,
    n_components=2,
    n_epochs=500,
    min_dist=0.1,
    metric="euclidean",
    init=initial,
    random_state=810,
    n_jobs=1,
    force_approximation_algorithm=True,
    precomputed_knn=(indices, knn_distances, None),
).fit_transform(matrix)
python_seconds = time.perf_counter() - start
start = time.perf_counter()
umap.UMAP(
    n_neighbors=k,
    n_components=2,
    n_epochs=500,
    min_dist=0.1,
    metric="euclidean",
    init=initial.copy(),
    random_state=810,
    n_jobs=1,
    force_approximation_algorithm=True,
    precomputed_knn=(indices, knn_distances, None),
).fit_transform(matrix)
warm_seconds = time.perf_counter() - start


def overlap(xy):
    distances = np.sum((xy[:, None] - xy[None, :]) ** 2, axis=2)
    np.fill_diagonal(distances, np.inf)
    nearest = np.argsort(distances, axis=1)[:, :5]
    return float(
        np.mean([len(set(a) & set(b)) / 5 for a, b in zip(nearest, indices[:, 1:6])])
    )


report = {
    "samples": n,
    "dimensions": dimensions,
    "model": model,
    "neighbors": k,
    "epochs": 500,
    "rust_version": "umap-rs 0.4.5",
    "python_version": f"umap-learn {umap.__version__}",
    "rust_seconds": rust_seconds,
    "python_cold_seconds": python_seconds,
    "python_warm_seconds": warm_seconds,
    "rust_top5_overlap": overlap(rust_xy),
    "python_top5_overlap": overlap(python_xy),
    "note": "One small subset, shared normalized vectors, exact neighbors and initialization; overlap is not a listening-quality score.",
}
(args.output / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")
np.savez(args.output / "comparison.npz", ids=ids, rust=rust_xy, python=python_xy)
print(json.dumps(report, indent=2), flush=True)
