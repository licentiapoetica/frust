#!/usr/bin/env bash
# f0ckm installer: ./install.sh
#
# Sets up .env and config.yaml with generated secrets, builds and starts the containers and creates
# the admin account. Safe to run again: existing .env/config.yaml are kept, and an admin is only
# created when there is none.
#
# Asks for anything it needs; for unattended installs set these instead:
#   DOMAIN=f0ck.example  EMAIL=you@example.com  ADMIN_USER=admin  ADMIN_PASSWORD=...  ./install.sh
# Leave DOMAIN empty for a local install on http://localhost:1337 without nginx/Let's Encrypt.
# PROXY=nginx (default) runs the bundled nginx-proxy + Let's Encrypt; PROXY=own skips them, so your own
# reverse proxy forwards the domain to 127.0.0.1:1337 (example: config/nginx/own-proxy.example.conf).
#
# Moving an existing site: ./install.sh --import-db dump.sql (or IMPORT_DB=dump.sql) loads a pg_dump
# (plain .sql, optionally .gz/.zst/.xz/.bz2, or pg_dump -Fc) before f0ckm starts; f0ckm then upgrades it
# to the current schema on its first start. A fresh install also asks for one. Replacing a database
# that already has tables asks first (unattended: IMPORT_DB_REPLACE=1). The dump is loaded into a
# separate database and only swapped in once it loaded completely, so a failed import changes nothing.
# Uploaded files are not in the dump: copy them into f0ckm-data/ (see docker-compose.yml for the folders).
set -euo pipefail

say()  { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

IMPORT_DB=${IMPORT_DB:-}
while [ $# -gt 0 ]; do
    case $1 in
        --import-db) [ $# -ge 2 ] || die "--import-db needs a dump file"; IMPORT_DB=$2; shift 2 ;;
        --import-db=*) IMPORT_DB=${1#*=}; shift ;;
        -h|--help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        *) die "unknown option '$1' (see ./install.sh --help)" ;;
    esac
done
# Resolved before the cd below, so a path relative to where the installer was started works
resolve_dump() {
    IMPORT_DB=${IMPORT_DB/#\~/$HOME}
    [ -f "$IMPORT_DB" ] && [ -r "$IMPORT_DB" ] || die "can't read the database dump '$IMPORT_DB'"
    IMPORT_DB=$(cd "$(dirname "$IMPORT_DB")" && pwd)/$(basename "$IMPORT_DB")
}
[ -n "$IMPORT_DB" ] && resolve_dump
START_DIR=$PWD
cd "$(dirname "$0")"
secret() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c "${1:-40}" || true; }
ask() { # ask VAR "question" [default]: keeps VAR when already set in the environment
    local var=$1 q=$2 def=${3:-} ans
    [ -n "${!var:-}" ] && return
    [ -t 0 ] || { printf -v "$var" '%s' "$def"; return; }
    read -r -p "$q${def:+ [$def]}: " ans
    printf -v "$var" '%s' "${ans:-$def}"
}

# ── Docker ───────────────────────────────────────────────────────────────────────
if ! command -v docker >/dev/null 2>&1; then
    warn "Docker is not installed."
    ans=n; [ -t 0 ] && read -r -p "Install it now with the official script (get.docker.com)? [y/N] " ans
    [[ $ans =~ ^[Yy] ]] || die "install Docker first: https://docs.docker.com/engine/install/"
    curl -fsSL https://get.docker.com | sh
fi
SUDO=
if ! docker info >/dev/null 2>&1; then
    command -v sudo >/dev/null 2>&1 && sudo docker info >/dev/null 2>&1 && SUDO=sudo \
        || die "can't talk to Docker. Is it running, and is your user allowed to use it?"
fi
docker() { if [ -n "$SUDO" ]; then sudo docker "$@"; else command docker "$@"; fi; }
# Compose v2 is the `docker compose` plugin; older installs only have the standalone `docker-compose`
if docker compose version >/dev/null 2>&1; then
    COMPOSE="docker compose"
    compose() { docker compose "$@"; }
elif command -v docker-compose >/dev/null 2>&1 && \
        { if [ -n "$SUDO" ]; then sudo docker-compose version; else docker-compose version; fi; } >/dev/null 2>&1; then
    COMPOSE="docker-compose"
    compose() { if [ -n "$SUDO" ]; then sudo docker-compose "$@"; else command docker-compose "$@"; fi; }
else
    die "Docker Compose is missing (neither 'docker compose' nor 'docker-compose' works)"
fi

[ -d config.yaml ] && die "config.yaml is a directory (Docker creates one when the file is missing). Remove it: rmdir config.yaml"

# A fresh install can start from an existing site's database
if [ -z "$IMPORT_DB" ] && [ ! -f .env ] && [ -z "$(ls -A f0ckm-data/postgres 2>/dev/null)" ] && [ -t 0 ]; then
    read -r -p "Database dump to import from an existing site (empty to start with an empty database): " IMPORT_DB
    if [ -n "$IMPORT_DB" ]; then
        case $IMPORT_DB in /*|\~*) ;; *) IMPORT_DB=$START_DIR/$IMPORT_DB ;; esac
        resolve_dump
    fi
fi

# ── .env ─────────────────────────────────────────────────────────────────────────
if [ -f .env ]; then
    say "Keeping existing .env"
    DOMAIN=${DOMAIN:-$(sed -n 's/^VIRTUAL_HOST=//p' .env)}
    if grep -q '^COMPOSE_PROFILES=.*f0ckm-nginx' .env; then PROXY=nginx; else PROXY=own; fi
else
    ask DOMAIN "Domain (empty for a local install on localhost:1337)"
    DOMAIN=${DOMAIN#http*://}; DOMAIN=${DOMAIN%%/*}
    if [ -n "$DOMAIN" ]; then
        if [ -z "${PROXY:-}" ] && [ -t 0 ]; then
            echo "Reverse proxy for $DOMAIN:"
            echo "  1) install the bundled nginx with automatic Let's Encrypt certificates"
            echo "  2) bring my own (nginx, Caddy, Traefik, ...) and forward it to 127.0.0.1:1337"
            read -r -p "Choose [1]: " ans
            case ${ans:-1} in 1|n*|N*) PROXY=nginx ;; 2|o*|O*|b*|B*) PROXY=own ;; *) die "pick 1 or 2" ;; esac
        fi
        PROXY=${PROXY:-nginx}
        case $PROXY in nginx|own) ;; *) die "PROXY must be 'nginx' or 'own', not '$PROXY'" ;; esac
        if [ "$PROXY" = nginx ]; then
            ask EMAIL "Email for Let's Encrypt"
            [ -n "${EMAIL:-}" ] || die "an email is needed for the Let's Encrypt certificate"
        fi
    fi
    DB_PASS=$(secret 32)
    # An existing database keeps the password it was created with (the old default was f0ckm)
    if [ -n "$(ls -A f0ckm-data/postgres 2>/dev/null)" ]; then
        warn "f0ckm-data/postgres already holds a database; keeping its default password 'f0ckm'"
        DB_PASS=f0ckm
    fi
    say "Writing .env"
    {
        echo "POSTGRES_USER=f0ckm"
        echo "POSTGRES_DB=f0ckm"
        echo "POSTGRES_PASSWORD=$DB_PASS"
        echo "IP_HASH_SECRET=$(secret 64)"
        if [ -n "$DOMAIN" ]; then
            # VIRTUAL_HOST also tells a re-run which domain this install uses
            echo "VIRTUAL_HOST=$DOMAIN"
            if [ "$PROXY" = nginx ]; then
                echo "COMPOSE_PROFILES=f0ckm-nginx"
                echo "LETSENCRYPT_HOST=$DOMAIN"
                echo "LETSENCRYPT_EMAIL=$EMAIL"
            fi
        fi
    } > .env
    chmod 600 .env
fi
[ "${DOMAIN:-}" = localhost ] && DOMAIN=

# ── config.yaml ──────────────────────────────────────────────────────────────────
# config_example.yaml filled in for this install. A new config.yaml is written from it, and an existing
# one gets the settings it is missing from it after the build (so new keys never arrive with dev defaults)
render_config() {
    local full host regex
    if [ -n "$DOMAIN" ]; then full="https://$DOMAIN"; host=$DOMAIN; else full="http://localhost:1337"; host=localhost; fi
    regex=$(printf '%s' "$host" | sed 's/\./\\\\./g')
    sed -e "s|^    full: https://example.com$|    full: $full|" \
        -e "s|^    domain: example.com$|    domain: $host|" \
        -e "s|^    regex: example\\\\.com$|    regex: $regex|" \
        -e "s|^  invite_secret: YOUR_SECRET_HERE$|  invite_secret: $(secret 40)|" \
        -e "s|^  development: true$|  development: false|" \
        -e "s|^  host: localhost$|  host: f0ckm-db|" \
        ${EMAIL:+-e "s|^  mail: admin@example.com$|  mail: $EMAIL|"} \
        config_example.yaml
}
if [ -f config.yaml ]; then
    say "Keeping existing config.yaml (missing settings are added after the build)"
else
    say "Writing config.yaml from config_example.yaml"
    render_config > config.yaml
    chmod 600 config.yaml
    grep -q 'YOUR_SECRET_HERE' config.yaml && warn "invite_secret was not replaced; set it in config.yaml"
fi

# ── Build and start ──────────────────────────────────────────────────────────────
say "Building and starting (the first build compiles the server and takes a few minutes)"
export GIT_HASH="f0ckm-$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
# Built here instead of `compose up --build`: docker-compose v1 and Docker without buildx use the classic
# builder, which can't read the Dockerfile's cache mounts; scripts/docker-build.sh handles both
IMAGE="$(sed -n 's/^F0CKM_IMAGE=//p' .env | tail -n 1)"; IMAGE=${IMAGE:-f0ckm}
TAG="$(sed -n 's/^F0CKM_TAG=//p' .env | tail -n 1)"; TAG=${TAG:-latest}
$SUDO scripts/docker-build.sh --build-arg GIT_HASH="$GIT_HASH" -t "$IMAGE:$TAG"

# Add settings config.yaml is missing (new options since it was written), keeping its values and comments.
# Runs as the file's owner, so the result keeps owner and permissions.
say "Checking config.yaml for missing settings"
# Exit code 2 means config.yaml doesn't parse: the server wouldn't start, so stop before replacing the running one.
rc=0
render_config | docker run --rm -i --network none --user "$(stat -c %u:%g config.yaml)" \
    -v "$PWD/config.yaml:/opt/f0ckm/config.yaml:Z" "$IMAGE:$TAG" config-update --from - || rc=$?
case $rc in
    0) ;;
    2) die "config.yaml is not valid YAML (see above), f0ckm can't start with it. Fix it and run ./install.sh again" ;;
    *) warn "could not add the missing settings to config.yaml; compare it with config_example.yaml by hand" ;;
esac

# ── Database import ──────────────────────────────────────────────────────────────
import_db() {
    local user name tmp tables ans magic decomp
    user=$(sed -n 's/^POSTGRES_USER=//p' .env | tail -n 1); user=${user:-f0ckm}
    name=$(sed -n 's/^POSTGRES_DB=//p' .env | tail -n 1); name=${name:-f0ckm}
    tmp=${name}_import
    case $IMPORT_DB in
        *.gz) decomp="gzip -dc" ;; *.zst) decomp="zstd -dc" ;; *.xz) decomp="xz -dc" ;; *.bz2) decomp="bzip2 -dc" ;;
        *) decomp=cat ;;
    esac
    command -v "${decomp%% *}" >/dev/null 2>&1 || die "'${decomp%% *}' is needed to unpack $IMPORT_DB"
    pg() { docker exec -i -e PGOPTIONS='-c client_min_messages=warning' f0ckm-db "$@"; }
    sql() { pg psql -U "$user" -d "$1" -v ON_ERROR_STOP=1 -Atqc "$2"; }

    say "Importing $IMPORT_DB"
    compose up -d f0ckm-db
    for i in $(seq 1 60); do
        [ "$(docker inspect -f '{{.State.Health.Status}}' f0ckm-db 2>/dev/null)" = healthy ] && break
        [ "$i" = 60 ] && die "the database did not start; see: $COMPOSE logs f0ckm-db"
        sleep 2
    done

    tables=$(sql "$name" "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
    if [ "$tables" != 0 ] && [ "${IMPORT_DB_REPLACE:-}" != 1 ]; then
        warn "the database '$name' already holds $tables tables, the import replaces all of it"
        ans=; [ -t 0 ] && read -r -p "Type 'replace' to continue: " ans
        [ "$ans" = replace ] || die "import cancelled, the database was not changed (unattended: IMPORT_DB_REPLACE=1)"
    fi

    # Loaded into a separate database in one transaction; the current one is only replaced once it worked
    pg dropdb -U "$user" --if-exists "$tmp"
    pg createdb -U "$user" "$tmp"
    magic=$($decomp "$IMPORT_DB" 2>/dev/null | head -c 5 || true)
    if [ "$magic" = PGDMP ]; then
        $decomp "$IMPORT_DB" | pg pg_restore -U "$user" -d "$tmp" --no-owner --no-acl --single-transaction --exit-on-error
    else
        # The old site's roles don't exist here: drop OWNER TO lines (outside COPY data) so f0ckm owns everything
        $decomp "$IMPORT_DB" \
            | awk '/^COPY .* FROM stdin;$/ { c = 1 } c { print; if ($0 == "\\.") c = 0; next } /^ALTER .* OWNER TO [^ ]+;$/ { next } { print }' \
            | pg psql -U "$user" -d "$tmp" -q -v ON_ERROR_STOP=1 --single-transaction >/dev/null
    fi || { pg dropdb -U "$user" --if-exists "$tmp"; die "the import failed (see above); the database was not changed"; }
    sql "$tmp" "SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'items'" | grep -q 1 \
        || { pg dropdb -U "$user" --if-exists "$tmp"; die "$IMPORT_DB has no items table, is it an f0ckm database dump?"; }

    # f0ckm must not use the database while it is swapped (it is started again below)
    docker stop f0ckm >/dev/null 2>&1 || true
    sql postgres "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)"
    sql postgres "ALTER DATABASE \"$tmp\" RENAME TO \"$name\""
    say "Imported $(sql "$name" "SELECT count(*) FROM items") items and $(sql "$name" "SELECT count(*) FROM \"user\"") users; f0ckm updates the schema on its first start"
    echo "    Uploaded files are not part of the dump: copy them into f0ckm-data/ (folders: docker-compose.yml)"
}
[ -n "$IMPORT_DB" ] && import_db

compose up -d

say "Waiting for f0ckm to answer"
for i in $(seq 1 90); do
    code=$(docker exec f0ckm curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:1337/ 2>/dev/null || true)
    [ -n "$code" ] && [ "$code" != 000 ] && break
    [ "$i" = 90 ] && die "f0ckm did not come up; see: $COMPOSE logs f0ckm"
    sleep 2
done

# ── Admin ────────────────────────────────────────────────────────────────────────
if docker exec f0ckm f0ckm users list 2>/dev/null | awk 'NR > 1 && $4 == "true" { found = 1 } END { exit !found }'; then
    say "An admin account already exists"
else
    ask ADMIN_USER "Admin username" admin
    generated=
    if [ -z "${ADMIN_PASSWORD:-}" ] && [ -t 0 ]; then
        read -r -s -p "Admin password (at least 20 characters, empty to generate one): " ADMIN_PASSWORD; echo
    fi
    if [ -z "${ADMIN_PASSWORD:-}" ]; then ADMIN_PASSWORD=$(secret 24); generated=1; fi
    [ "${#ADMIN_PASSWORD}" -ge 20 ] || die "the admin password needs at least 20 characters"
    out=$(printf '%s\n' "$ADMIN_PASSWORD" | docker exec -i f0ckm f0ckm create-admin "$ADMIN_USER" 2>&1) \
        || { printf '%s\n' "$out" >&2; die "creating the admin failed"; }
    say "Created admin '$ADMIN_USER'"
    [ -n "$generated" ] && printf '    password: %s   (save it now, it is not stored anywhere else)\n' "$ADMIN_PASSWORD"
fi

# ── Done ─────────────────────────────────────────────────────────────────────────
echo
if [ -n "$DOMAIN" ] && [ "$PROXY" = nginx ]; then
    say "Done: https://$DOMAIN"
    echo "    The domain's DNS has to point at this server and ports 80/443 must be open;"
    echo "    the certificate is issued within a minute or two after that."
elif [ -n "$DOMAIN" ]; then
    say "Done: f0ckm runs on http://127.0.0.1:1337; point your proxy for https://$DOMAIN at it"
    echo "    nginx site for it (adjust the certificate paths; other proxies need the same headers,"
    echo "    WebSocket upgrade and upload limits). Also in config/nginx/own-proxy.example.conf:"
    echo
    # The example without its header comment, for this domain
    sed -e '1,/^$/d' -e "s/f0ck\.example/$DOMAIN/g" config/nginx/own-proxy.example.conf
    echo
else
    say "Done: http://localhost:1337"
fi
echo "    Logs: $COMPOSE logs -f f0ckm    Settings: config.yaml (picked up live)"
