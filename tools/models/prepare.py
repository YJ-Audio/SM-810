"""Download pinned public CLAP assets; no audio is uploaded."""

import argparse
import hashlib
import json
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parent
manifest = json.loads((ROOT / "clap-manifest.json").read_text())
parser = argparse.ArgumentParser()
parser.add_argument("destination", type=Path)
args = parser.parse_args()
for asset in manifest["assets"]:
    target = args.destination / asset["path"]
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and target.stat().st_size == asset["bytes"]:
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        if not asset["sha256"] or digest == asset["sha256"]:
            print("Verified", asset["path"], flush=True)
            continue
    url = f"https://huggingface.co/{manifest['repository']}/resolve/{manifest['revision']}/{asset['path']}"
    temp = target.with_suffix(target.suffix + ".part")
    print("Downloading", asset["path"], asset["bytes"], flush=True)
    urllib.request.urlretrieve(url, temp)
    if temp.stat().st_size != asset["bytes"]:
        raise RuntimeError(f"Incorrect size: {target}")
    digest = hashlib.sha256(temp.read_bytes()).hexdigest()
    if asset["sha256"] and digest != asset["sha256"]:
        raise RuntimeError(f"Incorrect digest: {target}")
    temp.replace(target)
(args.destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(args.destination, flush=True)
