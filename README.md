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
git clone https://github.com/licentiapoetica/frust.git f0ckm && cd f0ckm
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
first (unattended: `IMPORT_DB_REPLACE=1`). The database user then defaults to the one the dump was
made with instead of `f0ckm` (`POSTGRES_USER`/`POSTGRES_DB`/`POSTGRES_PASSWORD` set it explicitly,
also without a dump). Uploaded files aren't in the dump: copy them into
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
npm ci && npm run vendor                    # browser libraries (Ruffle, Cap, marked, JSZip, Bad Apple) into public/s
./server/target/release/f0ckm seed
./server/target/release/f0ckm create-admin admin
STORAGE_DIR=f0ckm-data ./server/target/release/f0ckm
```

now visit http://localhost:1337 in your browser. Templates (`views/`), translations (`locales/`),
`public/` and `config.yaml` are read from disk and picked up without a restart; Rust changes need
`cargo build` and a restart (`cargo run --manifest-path server/Cargo.toml` does both).
For day-to-day work, `cargo build --profile fast --manifest-path server/Cargo.toml` (binary:
`server/target/fast/f0ckm`) is the quickest: dependencies are optimised like `--release`, the f0ckm
crate itself is built unoptimised and incrementally, so a change rebuilds in seconds instead of
about a minute and a half. Use `--release` for production.
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
| `GIT_HASH` | Version shown in the footer. Baked into the image as a build arg by `build.bash` / `install.sh`; hidden when unknown. |
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

JSZip, marked, the Cap captcha (@cap.js/widget, @cap.js/wasm) and Ruffle come from npm, and
bad-apple-virus (the Konami code easter egg, ↑↑↓↓←→←→BA) straight from GitHub
(`github:licentiapoetica/bad_apple_virus`; `.npmrc` allows git dependencies for this project's own
dependencies only). `package.json` names them, `package-lock.json` pins exact versions, checksums and
the GitHub commit. `npm ci` installs them and `npm run vendor` (`scripts/vendor.mjs`) copies the files
the site serves into `public/s/` (`js/jszip.min.js`, `js/marked.min.js`, `cap/`, `ruffle/`,
`bad-apple/`). These copies are not in git; the Docker build does both steps itself.
To take a newer commit of bad-apple-virus: `npm install github:licentiapoetica/bad_apple_virus && npm run vendor`.
Its setting is `websrv.bad_apple` in config.yaml (`enabled`, `theme`, `sound`).

Updates: merge the Dependabot PR (it changes both files), then run `npm ci && npm run vendor` locally
or rebuild the image. Major versions can change APIs, so try those before merging.

### Fonts

The built-in website fonts (Manrope, SIL OFL 1.1; Hack, MIT + Bitstream Vera License) are in
`public/s/fonts-builtin/` together with their license texts and credits (`README.md` there).
Font Awesome Free 6.7.2 (`public/s/fa/`) carries its license in the stylesheet header. The VCR OSD Mono
font (`public/s/vcr.ttf`) is by Riciery Leal (MrManet), free per https://www.dafont.com/vcr-osd-mono.font.
UnifrakturCook (`public/s/fonts-ui/UnifrakturCook.woff2`, the `[reich]` comment effect) is by j. 'mach' wust
and Peter Wiegel, under the SIL Open Font License 1.1; the license text is in
`public/s/fonts-ui/LICENSE-UnifrakturCook-OFL.txt` (source: https://fonts.google.com/specimen/UnifrakturCook/license).

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

All configuration lives in one file, `config.yaml` (start from `config_example.yaml`). Edits are picked up while the app is running; settings that are only read at startup need a restart. Every option is documented in the [wiki](https://github.com/licentiapoetica/frust/wiki/Configuration).

## NGINX

uncomment in .env # COMPOSE_PROFILES=f0ckm-nginx to enable nginx proxy with automatic lets encrypt.
Leave it out to run your own reverse proxy in front of `127.0.0.1:1337`; see
[config/nginx/own-proxy.example.conf](config/nginx/own-proxy.example.conf).

## Bot protection (Anubis)

[Anubis](https://anubis.techaro.lol) can sit between the proxy and f0ckm. Every client that calls itself a
browser solves a small proof of work once (well under a second, no clicking); a cookie then lets it in
for 7 days, only from the IP that solved it. Scraper swarms on rotating IPs pay again for every IP.
**It needs JavaScript:** without it the clearnet site can't be reached. The `.onion` address goes
straight to f0ckm and keeps working without JavaScript.

The installer asks once on every install that hasn't decided yet, also on an existing one (the answer
is kept as `ANUBIS=1`/`ANUBIS=0` in `.env`). `ANUBIS=1 ./install.sh` turns it on, `ANUBIS=0` off again.
With your own nginx on the same server the installer switches it too: in the `server {}` blocks for the
domain it sets `proxy_pass` to `:8923` (back to `:1337` when turned off), adds the check page's look
and a missing `X-Real-IP`, and puts all their `server_name`s into `ANUBIS_DOMAINS`. It shows the change
as a diff and asks first, checks it with `nginx -t` (puts the old file back if that fails) and reloads
nginx once Anubis answers. Unattended: `NGINX_EDIT=1` applies it, `NGINX_EDIT=0` never touches nginx.
A site that proxies through an `upstream {}` block is left alone with a hint. By hand, in `.env`:

```bash
COMPOSE_FILE=docker-compose.yml:docker-compose.anubis.yml
ANUBIS_KEY=...   # openssl rand -hex 32; signs the pass cookies, so they survive restarts
ANUBIS_DOMAINS=f0ck.example,www.f0ck.example   # where Anubis may send visitors back; default VIRTUAL_HOST
```

Every domain the site is reached under has to be in `ANUBIS_DOMAINS` (or `VIRTUAL_HOST`), or
visitors get "redirect domain not allowed" after passing the check.

then `docker compose up -d`. With the bundled nginx-proxy the domain moves to the Anubis container by
itself. With your own proxy, forward the domain to `127.0.0.1:8923` instead of `:1337`; it has to set
`X-Real-IP` (as in [own-proxy.example.conf](config/nginx/own-proxy.example.conf)), which Anubis
passes on to f0ckm.

The rules are in [config/anubis/botPolicy.yaml](config/anubis/botPolicy.yaml) (apply with
`docker compose restart anubis`). Without a check pass:

- files (`/b/`, `/t/`, `/a/`, `/s/`, ...), so media linked or embedded elsewhere keeps loading;
- clients that don't claim to be a browser: API tools, feed readers, and most link-preview bots
  (Telegram, Slack, WhatsApp, Signal, Twitter/X, Facebook, Mastodon, Matrix, ...);
- verified search engines, `robots.txt`, `favicon`, `/.well-known/`.

Preview bots that call themselves browsers (Discord, iMessage, Reddit, Bluesky, Teams, LinkedIn) get
the check page, which carries the page's Open Graph tags, so their previews still show title and
picture. Known AI crawlers and `HeadlessChrome` are turned away. A swarm with one fixed user agent
can be blocked outright with a rule (example in the file).

A visitor whose IP changes (a phone switching between Wi-Fi and mobile data) is checked again; an
open tab's requests fail until it is reloaded. Logs: `docker compose logs -f anubis`.

**The look.** The check page can be black with white text
([public/s/css/anubis.css](public/s/css/anubis.css)) and show a jackal-headed keeper in a temple
gate instead of Anubis' mascot ([config/anubis/img](config/anubis/img): gold eyes while checking, an
ankh when passed, red eyes when turned away). Anubis builds its files into its binary, so the proxy
in front answers their URLs with these. The bundled nginx-proxy gets them from docker-compose.anubis.yml
([config/anubis/nginx-proxy-look.conf](config/anubis/nginx-proxy-look.conf)); for your own nginx, inside the site's `server {}` (install.sh
adds these itself, between `# >>> f0ckm anubis` markers, and checks that nginx really serves them;
when it can't read the checkout (a home folder, SELinux) it gets a copy in `/var/lib/f0ckm-anubis/<domain>/`):

```nginx
location = /.within.website/x/xess/xess.min.css { alias /path/to/f0ckm/public/s/css/anubis.css; }
location = /.within.website/x/cmd/anubis/static/img/pensive.webp { alias /path/to/f0ckm/config/anubis/img/checking.webp; }
location = /.within.website/x/cmd/anubis/static/img/happy.webp   { alias /path/to/f0ckm/config/anubis/img/passed.webp; }
location = /.within.website/x/cmd/anubis/static/img/reject.webp  { alias /path/to/f0ckm/config/anubis/img/denied.webp; }
```

Anubis lets browsers cache these for a year, so visitors who already saw the default page keep it
until their cache clears or Anubis is upgraded.

The drawing is generated by [scripts/anubis-keeper.py](scripts/anubis-keeper.py) (needs
`rsvg-convert` and ImageMagick): `scripts/anubis-keeper.py config/anubis/img`, with
`--eyes slit|blank|narrow|orb|horus|white` for the style of the eyes. Anubis' own images
are MIT-licensed, so replacing them on your server is fine; the "Protected by Anubis" footer stays.

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
