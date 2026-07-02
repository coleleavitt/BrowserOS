# bos_build — BrowserOS Chromium build system

One engine, many products, many hosts. Every axis of variation —
product, platform, arch, host type — is data, not a copied config file.

## Layout

Three toolsets — BUILD (`steps/` on the `core/` engine), RELEASE
(`release/`), DEV (`patchkit/`) — over shared plumbing (`lib/`) and
product data (`products/`):

```
bos_build/
  browseros.py  entry — the `browseros` Typer app (also `python -m bos_build`)
  cli/          thin Typer wrappers (build, source, product, dev, release, ota)
  core/         engine: context, step registry, planner, runner, pipeline,
                resolver, events, product descriptor model — zero domain
                knowledge
  lib/          cross-cutting plumbing: env, utils, logger, paths, notify,
                sparkle, versions, r2 client, test fixtures
  products/     one package per product: define() call + server bundles
  steps/        BUILD toolset — pipeline steps registered via @step
                (source, setup, resources, patches, extensions, compile,
                sign, package, storage)
  release/      RELEASE toolset — list, publish, download, github, appcast;
                release/ota/ ships server OTA updates
  patchkit/     DEV toolset — the Python patch surface: dev extract,
                non-interactive batch-apply, features.yaml IO, and the
                read-only patch-stack doctor (interactive apply/sync
                lives in the Go tool: tools/patch, `bpatch`)
  profiles/     saved switch sets (flat yaml; a local profile may opt into
                an explicit modules: list — shipped profiles never do)
  config/       data: gn flags, resource yamls, appcast templates, offset
```

## How a build is composed

Pipeline shapes live in `core/planner.py` as one pure function:

```
plan(preset, platform, arch, switches) -> [step names]
```

- **Presets** (`release`, `debug`) encode step composition, including
  platform variance (sparkle vs winsparkle, mini_installer on unsigned
  Windows) — once, in code, golden-tested against the YAML matrix this
  replaced.
- **Switches** are flat toggles: `product`, `arch`, `clean`,
  `provision` (none/full/shallow), `download`, `sign`, `upload`.
  Resolution: CLI > profile file > preset default.
- **Steps** self-register with `@step(name, phase, platforms, notify,
  env, optional)`. Required env vars derive from the selected steps and
  are preflighted before anything runs; within-phase order is the
  import order in `steps/__init__.py`.

```bash
# Local signed release build
browseros build --preset release --product browserclaw --arch arm64

# What nightly CI runs (profile = saved switches)
browseros build --profile nightly-ci --arch x64

# Power users: explicit steps
browseros build --modules clean,compile,sign_macos --product browseros
```

### Seeing and tweaking a plan

The composed plan is always a projection of `plan()` — generated, never
hand-copied, so it can't drift:

```bash
# Print the composed steps + required env vars and exit
# (works without a chromium checkout)
browseros build --preset release --show-plan

# Comment out steps, as an operation: subtract from the composed plan
browseros build --preset release --skip upload,series_patches

# Resume the tail after a failure without recompiling
browseros build --preset release --from sign_macos

# One-off GN overrides while iterating (appended last, so they win)
browseros build --preset debug --gn-arg symbol_level=2 --gn-arg dcheck_always_on=true
```

- `--skip` (and a `skip:` list in profiles) subtracts **after**
  composition — it never re-triggers composition rules. CLI `--skip`
  and profile `skip:` union. Unknown step names fail loudly; a valid
  step absent from this plan is a no-op, so a saved `skip:` keeps
  working as presets evolve — subtraction from the canonical plan,
  never a copy of it.
- `--from` resumes the composed (post-skip) run timeline at a step:
  earlier runs are dropped, the first run containing the step is
  sliced, later runs stay whole. A failed universal merge resumes with
  just `--arch universal --from merge_universal` — no recompiles.
  CLI-only: resume is a one-off, so there is no `from:` profile key.
- `--gn-arg key=value` (repeatable, any mode) appends GN overrides
  **after** the flags file and product args — last write wins, so
  `configure` honors them without edits to committed `config/gn/*.gn`
  files. Values are verbatim GN: bools/ints bare, strings with embedded
  quotes (`--gn-arg 'target_cpu="arm64"'`). CLI-only by design — a
  profile wanting different flags should use a different flags file.
  Only `configure` writes args.gn: a plan that skips it (e.g.
  `--from compile`) reuses the existing file untouched, including any
  overrides a previous invocation wrote there.

### Modules profiles — "you own this list now"

For the rare run that genuinely wants an arbitrary sequence, a profile
may carry `modules:` as an explicit opt-in — a local, commentable file
that bypasses the planner entirely:

```yaml
# my-tail.yaml (local only — never shipped)
modules: [compile, sign_macos, package_macos]
build_type: release   # only valid with modules:; defaults to debug
arch: arm64           # single arch only
```

Planner-owned keys (`preset`, `clean`, `provision`, `download`, `sign`,
`upload`, `skip`) and the `--skip`/`--from` flags are rejected alongside
`modules:` — you own the list, edit it directly. Shipped profiles stay
switch-based (drift-tested).

## Remote / ephemeral runners

A fresh machine needs nothing outside this package:

```bash
uv sync
uv run browseros source ensure --root "$CHROMIUM_ROOT" --step checkout
uv run browseros build --modules clean --chromium-src "$CHROMIUM_ROOT/src" -t release
uv run browseros source ensure --root "$CHROMIUM_ROOT" --step sync
uv run browseros build --profile nightly-ci --chromium-src "$CHROMIUM_ROOT/src"
```

(checkout/sync are split because `clean` must run between them — it
deletes hook-managed toolchains that sync restores. `browseros source
cache restore|save` handles the R2 checkout cache on runners without
WarpCache.)

## Products

A product is one file: `products/<id>/product.py` with a
`ProductDescriptor.define()` call (~5 irreducible inputs, ~40 fields
derived by convention, keyword overrides for deviations) plus its
server bundle definitions. Verify with:

```bash
browseros product doctor          # identity uniqueness + branding assets
```

## Patch stack

`features.yaml` maps each feature to its patches under
`chromium_patches/`. The dev doctor keeps that map honest — read-only,
so it can run in CI and before a chromium bump:

```bash
browseros dev doctor                            # features.yaml ↔ patches on disk
browseros dev doctor --against ~/chromium/src   # + which patches fail, by feature
browseros dev doctor --feature llm-chat --json  # filtered / machine-readable
```

Exit 0 healthy / 1 findings / 2 usage or environment errors. `--against`
only ever dry-runs (`git apply --check`); the chromium tree is never
modified. The dry-run is stricter than the build's apply step (which
falls back to `--ignore-whitespace`/`--3way`), so a doctor failure means
"needs attention", not necessarily "won't build".

## Tests

```bash
uv run python -m unittest discover -s bos_build -t . -p "*_test.py"
uv run ruff check bos_build
```
