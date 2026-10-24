#!/usr/bin/env python3
"""One-time script: ask a vision LLM (OpenAI-compatible API) for 5 tags per archive image
and build a tag vocabulary from the result.

  LLM_API_KEY=... python3 scripts/vlm_tags.py <archive_dir> --url http://hub/v1 --model qwen2.5-vl --dry-run 3
  LLM_API_KEY=... python3 scripts/vlm_tags.py <archive_dir> --url http://hub/v1 --model qwen2.5-vl

Raw answers are kept in <archive>/vlm_tags.raw.json (a rerun continues where it stopped).
The vocabulary is written to <archive>/default_tags.json as [{"tag": "Berge", "prompt": "mountains"}].
"""
import argparse
import base64
import collections
import io
import json
import os
import random
import re
import sys
import urllib.request

try:
    from PIL import Image
except ImportError:
    Image = None

PROMPT_orig = (
    "Give exactly 5 short tags for this photo, most important first. "
    "Each tag is a single German noun or short noun phrase (e.g. Berge, Porträt, Sonnenuntergang) "
    'plus an English phrase describing it for CLIP (e.g. "mountains"). '
    'Answer only with JSON: {"tags": [{"de": "Berge", "en": "mountains"}, ...]}'
)
PROMPT = (
    "Give exactly 5 short tags for this photo, most important first. "
    "Each tag is a single German noun or short noun phrase (e.g. Berge, Porträt, Sonnenuntergang) "
    'plus an English phrase describing it for CLIP (e.g. "mountains"). '
    'Answer only with JSON: {"tags": [{"de": "Berge", "en": "mountains"}, ...]}'
)

EXTENSIONS = {".jpg", ".jpeg", ".png", ".tif", ".tiff"}
MAX_EDGE = 768


def data_url(path):
    ext = os.path.splitext(path)[1].lower()
    if Image is not None:
        im = Image.open(path)
        im.thumbnail((MAX_EDGE, MAX_EDGE))
        buf = io.BytesIO()
        im.convert("RGB").save(buf, "JPEG", quality=85)
        return "data:image/jpeg;base64," + base64.b64encode(buf.getvalue()).decode()
    if ext in (".tif", ".tiff"):
        raise ValueError("TIFF needs Pillow (pip install pillow)")
    mime = "image/png" if ext == ".png" else "image/jpeg"
    with open(path, "rb") as f:
        return f"data:{mime};base64," + base64.b64encode(f.read()).decode()


def ask(args, key, path):
    body = {
        "model": args.model,
        "temperature": 0.2,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": PROMPT},
                {"type": "image_url", "image_url": {"url": data_url(path)}},
            ],
        }],
    }
    req = urllib.request.Request(
        args.url.rstrip("/") + "/chat/completions",
        json.dumps(body).encode(),
        {"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
    )
    with urllib.request.urlopen(req, timeout=args.timeout) as r:
        text = json.load(r)["choices"][0]["message"]["content"]
    m = re.search(r"\{.*\}", text, re.S)  # tolerate prose or code fences around the JSON
    tags = json.loads(m.group(0))["tags"] if m else []
    return [
        {"de": t["de"].strip(), "en": t.get("en", t["de"]).strip()}
        for t in tags
        if isinstance(t, dict) and t.get("de", "").strip()
    ][:5]


def images(archive):
    root = os.path.join(archive, "images")
    found = []
    for d, _, files in os.walk(root):
        for f in files:
            if os.path.splitext(f)[1].lower() in EXTENSIONS:
                found.append(os.path.relpath(os.path.join(d, f), archive))
    return sorted(found)


def vocabulary(raw, min_count, limit):
    count = collections.Counter()
    spelling = {}
    prompts = collections.defaultdict(collections.Counter)
    swiss = lambda t: t["de"].replace("ß", "ss")  # Swiss spelling: no sharp s
    for tags in raw.values():
        for key in {swiss(t).lower() for t in tags}:
            count[key] += 1
        for t in tags:
            key = swiss(t).lower()
            spelling.setdefault(key, swiss(t))
            prompts[key][t["en"].lower()] += 1
    return [
        {"tag": spelling[k], "prompt": prompts[k].most_common(1)[0][0]}
        for k, n in count.most_common()
        if n >= min_count
    ][:limit]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("archive")
    p.add_argument("--url", required=True, help="API base, e.g. http://hub:4000/v1")
    p.add_argument("--model", required=True)
    p.add_argument("--min-count", type=int, default=2, help="keep tags used by at least this many images")
    p.add_argument("--max", type=int, default=150, help="vocabulary size limit")
    p.add_argument("--timeout", type=int, default=120)
    p.add_argument("--dry-run", type=int, metavar="N", help="query N random images, print the tags, write nothing")
    args = p.parse_args()
    key = os.environ.get("LLM_API_KEY", "")

    files = images(args.archive)
    if not files:
        sys.exit(f"No images in {args.archive}/images")

    if args.dry_run:
        for rel in random.sample(files, min(args.dry_run, len(files))):
            try:
                print(rel, [f"{t['de']} ({t['en']})" for t in ask(args, key, os.path.join(args.archive, rel))])
            except Exception as e:
                print(rel, "ERROR", e)
        return

    raw_path = os.path.join(args.archive, "vlm_tags.raw.json")
    raw = {}
    if os.path.exists(raw_path):
        with open(raw_path) as f:
            raw = json.load(f)
    todo = [r for r in files if r not in raw]
    print(f"{len(files)} images, {len(todo)} to do")
    for i, rel in enumerate(todo, 1):
        try:
            raw[rel] = ask(args, key, os.path.join(args.archive, rel))
        except KeyboardInterrupt:
            break
        except Exception as e:
            print(f"[{i}/{len(todo)}] {rel}: {e}", file=sys.stderr)
            continue
        print(f"[{i}/{len(todo)}] {rel}: {', '.join(t['de'] for t in raw[rel])}")
        if i % 10 == 0:
            with open(raw_path, "w") as f:
                json.dump(raw, f, ensure_ascii=False)
    with open(raw_path, "w") as f:
        json.dump(raw, f, ensure_ascii=False, indent=1)

    vocab = vocabulary(raw, args.min_count, args.max)
    out = os.path.join(args.archive, "default_tags.json")
    with open(out, "w") as f:
        json.dump(vocab, f, ensure_ascii=False, indent=1)
    print(f"{len(vocab)} tags written to {out}")


if __name__ == "__main__":
    main()
