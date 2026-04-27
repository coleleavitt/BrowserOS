# balpha

Internal BrowserOS alpha dogfooding CLI.

## Install

```bash
cd packages/browseros-agent/tools/alpha
make install
```

## Configure

```bash
balpha init
```

Config lives at `~/.config/balpha/config.yaml`.

## Run

```bash
balpha start
```

`start` uses the currently checked-out repo state. It does not pull.
Each start runs the existing `tools/dev/setup.sh` setup flow and the WXT extension build. Bun/WXT caches keep repeated starts fast.

To refresh the repo explicitly:

```bash
balpha pull
```

To re-copy the source BrowserOS profile:

```bash
balpha refresh-profile
```

## Notes

- `balpha` launches the configured BrowserOS app with `--disable-browseros-server`.
- The local Bun server runs from `packages/browseros-agent/apps/server`.
- The dev extension is built from `packages/browseros-agent/apps/agent`.
- The dev profile defaults to `~/.config/balpha/profile` and is separate from the real BrowserOS profile.
- Generated `apps/server/.env.production` and `apps/cli/.env.production` files come from `production_env` in config.
- Default ports are CDP `9015`, server `9115`, and extension `9315`; if a port is busy, `balpha start` auto-increments and saves the resolved ports.
- Do not point `dev_user_data_dir` at the real BrowserOS profile.
- `balpha` does not pass `--use-mock-keychain`; the default signed app path is required for copied login data to decrypt reliably.
