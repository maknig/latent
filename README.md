# Latent

A schema-driven archive for analog film: catalogue rolls, sheets and darkroom enlargements with their scans. Rust and egui, native and web, plain JSON files, no database.

## Features

- Tables, forms and filters are generated from a JSON schema (`schemas/latent.json` is the default)
- Image strips with thumbnails, image view with zoom, crop, rotate and dust removal (originals are backed up)
- Enlargements are linked to the film image they were printed from
- PDF printing of tables and film cards
- Local similar-image search, auto tags and people grouping with ONNX models (optional)

## Run

```
nix run github:maknig/latent -- [archive_dir]   # or `nix run . -- [archive_dir]` in a checkout
cargo run -p latent [archive_dir]               # without Nix
cd apps/web && trunk serve                      # web build
```

The first `nix run` compiles the app, so it takes a while and needs network access for the crates.

## Archive folder

Default `<Documents>/Latent`. It holds `latent.schema.json` (copied from the default if missing), `latent.data.json` and `images/`. Saves are atomic and keep a `.bak`.

## Vision models

The vision features need ONNX models in `<archive>/models/`; see `CLAUDE.md` for the expected files. The InsightFace weights used for face recognition are licensed for non-commercial use only.
