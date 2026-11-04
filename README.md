# f0ckm

Happy to finally bring you f0ckm! The long awaited imageboard solution that you can host yourself. It is designed to be a real alternative to big tech platforms that you can modify to suit your communities needs.

It features extensive tagging, searching, filtering and a variety of options.

The software is mostly generic, it can be modified easily via `config.yaml` to suit your communities needs. Most things can be enabled/disabled very easily and modified to needs.

The software comes without any warranties or entitlements of any kind! The developer is not responsible for anything you do with the use of this software.

## Software Requirements

- Docker (https://docs.docker.com/engine/install/debian/)

For development without Docker (except for the database): a Rust toolchain (1.90 or newer,
https://rustup.rs) and the media tools the server calls: `ffmpeg`, `ffmpegthumbnailer`, ImageMagick 7
(`magick`), `ghostscript`, `exiftool`, `file`, `yt-dlp` with `deno` (YouTube), `curl`.
Node.js is no longer needed.

## prod

```bash
git clone <repo> f0ckm && cd f0ckm
./install.sh
```

It asks for the domain, whether to install the bundled nginx (with automatic Let's Encrypt) or use
your own reverse proxy, an email for Let's Encrypt and the admin account, then writes `.env` and
`config.yaml` with generated secrets, builds and starts everything and creates the admin. Leave the
domain empty for a local install on http://localhost:1337. With the bundled nginx the domain's DNS has
to point at the server and ports 80/443 must be open for the certificate. With your own proxy, forward
the domain to `127.0.0.1:1337`; [config/nginx/own-proxy.example.conf](config/nginx/own-proxy.example.conf)
shows what it needs (Host/IP headers, WebSocket upgrade, big uploads). Unattended:
`DOMAIN=f0ck.example EMAIL=you@example.com ADMIN_PASSWORD=... ./install.sh` (add `PROXY=own` to skip
the bundled nginx).

The image is built with BuildKit when the `docker-buildx` plugin is installed, which caches the Rust
build between rebuilds. Without it (or with the old `docker-compose` v1) it still builds, just from
scratch every time.

Running it again is safe: existing `.env`/`config.yaml` are kept and no second admin is created.
The database schema and default settings are created on the first start.

Moving an existing site: `./install.sh --import-db /path/to/dump.sql` (or `IMPORT_DB=...`) loads a
`pg_dump` of its database before f0ckm starts (plain SQL, also `.gz`/`.zst`/`.xz`/`.bz2`, or
`pg_dump -Fc`); a fresh install also asks for one. f0ckm brings it up to the current schema on its
first start. The dump is loaded into a separate database and only swapped in when it loaded
completely, so a failed import changes nothing; replacing a database that already has tables asks
first (unattended: `IMPORT_DB_REPLACE=1`). Uploaded files aren't in the dump: copy them into
`f0ckm-data/` (the folders are listed in `docker-compose.yml`).

Postgres (`127.0.0.1:5454`) and the app (`127.0.0.1:1337`) only listen on the server itself; public
traffic goes through nginx-proxy or your own proxy.

For regular building after making changes to the source code

`bash build.bash master`

### Manual setup

`cp .env.example .env` and set `POSTGRES_PASSWORD` and `IP_HASH_SECRET` (`openssl rand -hex 32`); for a
public domain also `COMPOSE_PROFILES=f0ckm-nginx`, `VIRTUAL_HOST`, `LETSENCRYPT_HOST`, `LETSENCRYPT_EMAIL`.
`cp config_example.yaml config.yaml` and set `main.url` and `invite_secret`. Then:

```bash
docker compose up -d
docker exec -it f0ckm f0ckm create-admin admin   # asks for the password; at least 20 characters
```

## dev

`docker compose up -d f0ckm-db`

on dev machine:

```bash
cp config_example.yaml config.yaml          # main.development: true uses localhost:5454 for the database
cargo build --release --manifest-path server/Cargo.toml
npm ci && npm run vendor                    # browser libraries (Ruffle, Cap, marked, JSZip) into public/s
./server/target/release/f0ckm seed
./server/target/release/f0ckm create-admin admin
STORAGE_DIR=f0ckm-data ./server/target/release/f0ckm
```

now visit http://localhost:1337 in your browser. Templates (`views/`), translations (`locales/`),
`public/` and `config.yaml` are read from disk and picked up without a restart; Rust changes need
`cargo build` and a restart (`cargo run --manifest-path server/Cargo.toml` does both).
`public/s/css/bundle.css` (the production stylesheet) is rebuilt automatically at start when
`f0ckm.css` or `v0ck.css` changed, or by hand with `f0ckm build-css`.

### Environment

| Variable | Meaning |
|---|---|
| `STORAGE_DIR` | Directory with the media and runtime data (`b/`, `t/`, `ca/`, `pending/`, `deleted/`, `tmp/`, `logs/`, `import/`, …). Without it they live under `public/` and the project root. |
| `DB_HOST`, `DB_PORT`, `DB_USER`, `DB_PASS`, `DB_NAME` | Override the `sql:` section of `config.yaml` (also `POSTGRES_*`/`PG*`). |
| `F0CKM_PORT` | Port to listen on (default: `websrv.port`). |
| `F0CKM_BASE` | Project root with `config.yaml`, `views/`, `public/`, `locales/`, `migrations/` (default: current directory). |
| `IP_HASH_SECRET` | Secret for hashing user IPs (see `.env.example`). |
| `GIT_HASH` | Version shown in the footer (set by `build.bash`). |
| `RUST_LOG` | Log filter, e.g. `info` or `debug,sqlx=warn`. |

### Command line

The `f0ckm` binary also runs the maintenance tools (`f0ckm help` lists them; in Docker:
`docker exec -it f0ckm f0ckm <command>`). They read `config.yaml` and the variables above like the
server does.

| Command | |
|---|---|
| `f0ckm seed` | Default site settings and the rating tags |
| `f0ckm create-admin <user> [<password>]` | Create the admin (or reset an existing one) |
| `f0ckm users list` / `create <user> [--admin] [--mod]` / `passwd <user>` | Accounts |
| `f0ckm hash-password` | Print an Argon2id hash |
| `f0ckm import [--user u] [--rating r] [--tags a,b] [--dir d] [--keep] …` | Bulk import from `f0ckm-data/import` (`import --help`) |
| `f0ckm regen <ids> \| --all \| --audio \| --pdf \| --youtube [--blur] [--missing] [--from id] [--resume]` | Regenerate thumbnails, cover art, blurred thumbnails |
| `f0ckm rehash [--phash] [--dry-run]` | Recompute checksums/sizes, or fill in missing perceptual hashes |
| `f0ckm find-duplicates` | Visual duplicates by perceptual hash |
| `f0ckm autotag <from_id> [<to_id>] [--dry-run]` | Rate untagged images with the configured NSFW APIs |
| `f0ckm clean [--dry-run] [--yes]` | Report missing media, delete unreferenced files |
| `f0ckm build-css [--check]` | Build `public/s/css/bundle.css` |
| `f0ckm check-templates` | Compile all templates |

Passwords left out on the command line are read from stdin.

### Browser libraries

JSZip, marked, the Cap captcha (@cap.js/widget, @cap.js/wasm) and Ruffle come from npm:
`package.json` names them, `package-lock.json` pins exact versions and checksums. `npm ci` installs
them and `npm run vendor` (`scripts/vendor.mjs`) copies the files the site serves into `public/s/`
(`js/jszip.min.js`, `js/marked.min.js`, `cap/`, `ruffle/`). These copies are not in git; the Docker
build does both steps itself.

Updates: merge the Dependabot PR (it changes both files), then run `npm ci && npm run vendor` locally
or rebuild the image. Major versions can change APIs, so try those before merging.

### Fonts

The built-in website fonts (Manrope, SIL OFL 1.1; Hack, MIT + Bitstream Vera License) are in
`public/s/fonts-builtin/` together with their license texts and credits (`README.md` there).
Font Awesome Free 6.7.2 (`public/s/fa/`) carries its license in the stylesheet header. The VCR OSD Mono
font (`public/s/vcr.ttf`) is by Riciery Leal (MrManet), free per https://www.dafont.com/vcr-osd-mono.font.
UnifrakturCook (`public/s/fonts/UnifrakturCook.woff2`, the `[reich]` comment effect) is by j. 'mach' wust
and Peter Wiegel, under the SIL Open Font License 1.1; the license text is in
`public/s/fonts/LICENSE-UnifrakturCook-OFL.txt` (source: https://fonts.google.com/specimen/UnifrakturCook/license).

### Browser security

- **Content-Security-Policy** (`server/src/csp.rs`): scripts run only from this site or with the
  per-request nonce, so inline `<script>` tags need `nonce="{{ csp_nonce }}"` and markup can't use
  inline handlers (`onclick="…"`). Bind events with `data-on="click:action"` instead
  (`public/s/js/csp-actions.js`). Violations are logged as `[CSP]`; `websrv.csp.report_only: true`
  logs without blocking.
- **Flash** plays in a sandboxed iframe (`/flash-frame`, `public/s/js/flash-embed.js`): uploaded SWFs
  run without the viewer's session and can only reach this site.
- **Captcha**: Cap (`cap:` in the config), a self-hosted proof-of-work captcha, on registration and
  guest reports.

## Configuration

All configuration lives in one file, `config.yaml` (start from `config_example.yaml`). Edits are picked up while the app is running; settings that are only read at startup need a restart. Every option is documented in the [wiki](https://github.com/licentiapoetica/f0ckm/wiki/Configuration).

## NGINX

uncomment in .env # COMPOSE_PROFILES=f0ckm-nginx to enable nginx proxy with automatic lets encrypt.
Leave it out to run your own reverse proxy in front of `127.0.0.1:1337`; see
[config/nginx/own-proxy.example.conf](config/nginx/own-proxy.example.conf).

## Tor Hidden Service (.onion)

Tor is bundled in `docker-compose.yml` and always runs next to f0ckm. It is the SOCKS proxy for outgoing fetches (`main.socks: socks5h://tor:9050` in `config.yaml`) and serves a hidden service pointing to the application (`f0ckm:1337`).

When running `docker compose up -d`, Tor automatically generates a `.onion` address for your node. You can find your onion address by running:

```bash
cat ./f0ckm-data/tor/f0ckm_hs/hostname
```

### Automatic Onion-Location Header

To advertise your `.onion` address to Tor Browser users visiting your clearnet site, add your `.onion` address to `config.yaml`:

```yaml
main:
  onion: http://yourgeneratedaddress.onion
```

## Credits

f0ckm grew out of f0ckv2, which in turn grew out of the original f0ck. f0ck was born out of the need for an open-source alternative to pr0gramm.

Built with every human emotion you can think of by Flummi and Kibi starting in 2016 still going strong.
