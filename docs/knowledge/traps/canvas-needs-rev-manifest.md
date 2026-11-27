---
tags: [trap, canvas, ruby, docker]
---
# Trap: the brand-CSS migrations abort without `public/dist/rev-manifest.json`

**Symptom.** `db:migrate` stops at
`RegenerateBrandFilesBasedOnNewDefaultsPredeploy` with
`StandardError: you must run "gulp rev" first`
(`lib/canvas/cdn/registry/gulp.rb:54`).

**Cause.** In production, `Canvas::Cdn::Registry::Gulp#load_manifest_from_disk`
**raises** when `public/dist/rev-manifest.json` is missing. The brand migration
calls `BrandConfig#save_all_files!`, which calls `image_path`, which goes
through the CDN registry.

**Fix.** The file is a build artifact (`/public/dist/` is gitignored upstream), so
creating a minimal one does not touch tracked source:

```sh
mkdir -p canvas-src/public/dist && echo '{}' > canvas-src/public/dist/rev-manifest.json
```

An empty manifest is enough — the API parity work does not need fingerprinted
assets. `canvas-boot/bootstrap/entrypoint.sh` now does this automatically.
