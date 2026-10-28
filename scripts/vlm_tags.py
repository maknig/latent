#!/usr/bin/env python3
"""One-time script: ask a vision LLM (OpenAI-compatible API) for 5 tags per archive image
and build a tag vocabulary from the result.

  LLM_API_KEY=... python3 scripts/vlm_tags.py <archive_dir> --url http://hub/v1 --model qwen2.5-vl --dry-run 3
  LLM_API_KEY=... python3 scripts/vlm_tags.py <archive_dir> --url http://hub/v1 --model qwen2.5-vl

Raw answers are kept in <archive>/vlm_tags.raw.json (a rerun continues where it stopped).
The vocabulary is written to <archive>/default_tags.json as [{"tag": "Berge", "prompt": "mountains"}].

  python3 scripts/vlm_tags.py <archive_dir> --stats [--top 40]

prints how often each tag occurs (histogram, tags by frequency, effect of --min-count/--max-share)
without LLM calls and without writing anything (add --csv to write <archive>/tag_distribution.csv).
A normal run prints it at the end too and writes the CSV.
"""
import argparse
import base64
import collections
import csv
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

PROMPT = (
    "Give exactly 5 short tags for this photo, most important first. "
    "Describe only what is depicted (subjects, places, objects, activities, mood, animals, architecture). "
    "Never describe the photo itself: no medium, colour, film, technique or style. "
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


def distribution(raw):
    """Per tag: number of images using it (case-insensitive, Swiss spelling) and its CLIP prompt."""
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
    return count, spelling, prompts


def print_distribution(args, raw, write_csv=True):
    count, spelling, prompts = distribution(raw)
    total = len(raw)
    if not total or not count:
        print("no tags")
        return
    drop_above = args.max_share * total
    print(
        f"\n{total} images, {len(count)} distinct tags, "
        f"{sum(1 for n in count.values() if n == 1)} used once, "
        f"{sum(count.values()) / total:.1f} tags/image"
    )
    ranked = count.most_common()
    widest = ranked[0][1]
    print(f"top {min(args.top, len(ranked))} (* = dropped by --max-share {args.max_share}):")
    for k, n in ranked[: args.top]:
        mark = "*" if n > drop_above else " "
        print(f"{n:5d} {100 * n / total:5.1f}% {mark} {'█' * max(1, round(30 * n / widest)):<30} {spelling[k]}")
    print("tags by number of images:")
    for label, lo, hi in [("1", 1, 1), ("2", 2, 2), ("3-5", 3, 5), ("6-10", 6, 10), (">10", 11, None)]:
        tags = sum(1 for n in count.values() if n >= lo and (hi is None or n <= hi))
        print(f"  {label:>4}: {tags:5d}")
    kept = sum(1 for n in count.values() if args.min_count <= n <= drop_above)
    print(f"--min-count {args.min_count} / --max-share {args.max_share} keep {kept} tags (before --max {args.max})")

    if not write_csv:
        return
    out = os.path.join(args.archive, "tag_distribution.csv")
    with open(out, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["tag", "count", "share", "prompt"])
        for k, n in ranked:
            w.writerow([spelling[k], n, f"{n / total:.4f}", prompts[k].most_common(1)[0][0]])
    print(f"full distribution written to {out}")


def vocabulary(raw, min_count, limit, max_share):
    count, spelling, prompts = distribution(raw)
    # A tag on almost every image (e.g. Schwarzweiss in a b/w archive) cannot tell images apart.
    too_common = {k for k, n in count.items() if n > max_share * len(raw)}
    if too_common:
        print("dropped (too common):", ", ".join(spelling[k] for k in sorted(too_common)))
    return [
        {"tag": spelling[k], "prompt": prompts[k].most_common(1)[0][0]}
        for k, n in count.most_common()
        if n >= min_count and k not in too_common
    ][:limit]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("archive")
    p.add_argument("--url", help="API base, e.g. http://hub:4000/v1")
    p.add_argument("--model")
    p.add_argument("--min-count", type=int, default=2, help="keep tags used by at least this many images")
    p.add_argument("--max", type=int, default=150, help="vocabulary size limit")
    p.add_argument("--max-share", type=float, default=0.3, help="drop tags used by more than this share of images")
    p.add_argument("--timeout", type=int, default=120)
    p.add_argument("--dry-run", type=int, metavar="N", help="query N random images, print the tags, write nothing")
    p.add_argument("--rebuild", action="store_true", help="only rebuild default_tags.json from vlm_tags.raw.json (no LLM calls)")
    p.add_argument("--stats", action="store_true", help="only print the tag distribution from vlm_tags.raw.json (no LLM calls, writes nothing)")
    p.add_argument("--csv", action="store_true", help="with --stats: also write tag_distribution.csv")
    p.add_argument("--top", type=int, default=40, help="number of tags listed in the distribution")
    args = p.parse_args()
    offline = args.rebuild or args.stats
    if not offline and not (args.url and args.model):
        p.error("--url and --model are required unless --rebuild or --stats")
    key = os.environ.get("LLM_API_KEY", "")

    if offline:
        raw_path = os.path.join(args.archive, "vlm_tags.raw.json")
        if not os.path.exists(raw_path):
            sys.exit(f"{raw_path} not found")
        with open(raw_path) as f:
            raw = json.load(f)
        print_distribution(args, raw, write_csv=args.csv or not args.stats)
        if args.rebuild:
            write_vocabulary(args, raw)
        return

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

    print_distribution(args, raw)
    write_vocabulary(args, raw)


def write_vocabulary(args, raw):
    vocab = vocabulary(raw, args.min_count, args.max, args.max_share)
    out = os.path.join(args.archive, "default_tags.json")
    with open(out, "w") as f:
        json.dump(vocab, f, ensure_ascii=False, indent=1)
    print(f"{len(vocab)} tags written to {out}")


if __name__ == "__main__":
    main()
