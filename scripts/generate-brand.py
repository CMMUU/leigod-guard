#!/usr/bin/env python3
"""Generate all brand assets from assets/brand.json; --check needs only Python stdlib.

Generation: python -m pip install -r scripts/brand-requirements.txt
            python scripts/generate-brand.py
Verification: python scripts/generate-brand.py --check
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "assets/brand.json"
RECORD = ROOT / "assets/brand.generated.json"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def target(name):
    path = (ROOT / name).resolve()
    if not path.is_relative_to(ROOT):
        raise ValueError("Brand paths must be inside the repository")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    config_bytes = CONFIG.read_bytes().replace(b"\r\n", b"\n")
    config = json.loads(config_bytes)
    source = target(config["source"]).read_bytes()
    paths = [name for kind in ("png", "rgba", "ico") for name in config[kind]]
    if len(set(paths)) != len(paths) or config["source"] in paths:
        raise ValueError("Brand outputs must be unique and cannot overwrite the source")
    identity = {"source_sha256": digest(source), "config_sha256": digest(config_bytes)}
    if args.check:
        record = json.loads(RECORD.read_text(encoding="utf-8"))
        if any(record.get(key) != value for key, value in identity.items()):
            raise ValueError("Brand source/config changed; regenerate all assets")
        if set(record["outputs"]) != set(paths):
            raise ValueError("Brand output list differs from manifest")
        for name in paths:
            if digest(target(name).read_bytes()) != record["outputs"][name]:
                raise ValueError(f"Brand asset is stale or edited separately: {name}")
        print(f"Brand verified: one source, {len(paths)} outputs (desktop/tray/website/server)")
        return

    from PIL import Image, __version__
    if __version__ != "11.3.0":
        raise ValueError("Use the pinned scripts/brand-requirements.txt for reproducible resizing")
    with Image.open(io.BytesIO(source)) as image:
        original = image.convert("RGBA")
    if original.width != original.height or original.width < 256:
        raise ValueError("Brand source must be a square PNG of at least 256px")

    def resized(size):
        return original.resize((size, size), Image.Resampling.LANCZOS)

    def png(size):
        output = io.BytesIO()
        resized(size).save(output, format="PNG", compress_level=9)
        return output.getvalue()

    outputs = {}
    for name, size in config["png"].items():
        outputs[name] = png(size)
    for name, size in config["rgba"].items():
        outputs[name] = resized(size).tobytes()
    for name, sizes in config["ico"].items():
        frames = [png(size) for size in sizes]
        header = struct.pack("<HHH", 0, 1, len(sizes))
        offset = 6 + 16 * len(sizes)
        for size, frame in zip(sizes, frames):
            header += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(frame), offset)
            offset += len(frame)
        outputs[name] = header + b"".join(frames)
    for name, data in outputs.items():
        target(name).parent.mkdir(parents=True, exist_ok=True)
        target(name).write_bytes(data)
    record = {**identity, "generator": "Pillow 11.3.0 / Lanczos", "outputs": {name: digest(data) for name, data in outputs.items()}}
    RECORD.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Generated {len(outputs)} brand outputs; commit source, manifest, outputs and record together")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print(f"Brand check failed: {error}", file=sys.stderr)
        sys.exit(1)
