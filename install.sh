#!/usr/bin/env bash
# f0ckm installer: ./install.sh
#
# Sets up .env and config.yaml with generated secrets, builds and starts the containers and creates
# the admin account. Safe to run again: existing .env/config.yaml are kept, and an admin is only
# created when there is none.
#
# Asks for anything it needs; for unattended installs set these instead:
#   DOMAIN=f0ck.example  EMAIL=you@example.com  ADMIN_USER=admin  ADMIN_PASSWORD=...  ./install.sh
# The database user and name are f0ckm (POSTGRES_USER, POSTGRES_DB and POSTGRES_PASSWORD set others);
# with an imported dump the user defaults to the one the dump was made with.
# Leave DOMAIN empty for a local install on http://localhost:1337 without nginx/Let's Encrypt.
# PROXY=nginx (default) runs the bundled nginx-proxy + Let's Encrypt; PROXY=own skips them, so your own
# reverse proxy forwards the domain to 127.0.0.1:1337 (example: config/nginx/own-proxy.example.conf).
# ANUBIS=1 puts the Anubis bot protection in front (browsers solve a proof of work once; needs JavaScript,
# the .onion works without; an own proxy then forwards to 127.0.0.1:8923); ANUBIS=0 takes it out again.
# An install that hasn't decided yet (no ANUBIS= in .env) asks once; the answer is kept in .env.
# An own nginx on this server is switched along: its site for the domain gets proxy_pass to :8923 (or
# back to :1337), the check page's look and X-Real-IP; the changes are shown and asked for first
# (unattended: NGINX_EDIT=1 applies them, NGINX_EDIT=0 never touches nginx).
#
# Moving an existing site: ./install.sh --import-db dump.sql (or IMPORT_DB=dump.sql) loads a pg_dump
# (plain .sql, optionally .gz/.zst/.xz/.bz2, or pg_dump -Fc) before f0ckm starts; f0ckm then upgrades it
# to the current schema on its first start. A fresh install also asks for one. Replacing a database
# that already has tables asks first (unattended: IMPORT_DB_REPLACE=1). The dump is loaded into a
# separate database and only swapped in once it loaded completely, so a failed import changes nothing.
# Uploaded files are not in the dump: copy them into f0ckm-data/ (see docker-compose.yml for the folders).
#
# Postgres: a new install starts on the current version (PG_TARGET below, kept as PG_MAJOR in .env). An
# older database is upgraded on the next run, by itself: f0ckm is stopped, the database dumped (into
# f0ckm-data/pg-upgrade/), restored into a new Postgres, checked table by table, and f0ckm started again.
# The old folder stays as f0ckm-data/postgres-<old version>; when anything fails, it is put back.
# ./install.sh --rollback-db goes back to it afterwards (losing what was written since). PG_UPGRADE=0
# in .env stays on the old version; ./install.sh --upgrade-db upgrades anyway.
set -euo pipefail

say()  { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

IMPORT_DB=${IMPORT_DB:-}
UPGRADE_DB=
ROLLBACK_DB=
while [ $# -gt 0 ]; do
    case $1 in
        --import-db) [ $# -ge 2 ] || die "--import-db needs a dump file"; IMPORT_DB=$2; shift 2 ;;
        --import-db=*) IMPORT_DB=${1#*=}; shift ;;
        --upgrade-db) UPGRADE_DB=1; shift ;;
        --rollback-db) ROLLBACK_DB=1; shift ;;
        -h|--help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        *) die "unknown option '$1' (see ./install.sh --help)" ;;
    esac
done
# Resolved before the cd below, so a path relative to where the installer was started works
resolve_dump() {
    IMPORT_DB=${IMPORT_DB/#\~/$HOME}
    [ -f "$IMPORT_DB" ] && [ -r "$IMPORT_DB" ] || die "can't read the database dump '$IMPORT_DB'"
    IMPORT_DB=$(cd "$(dirname "$IMPORT_DB")" && pwd)/$(basename "$IMPORT_DB")
    command -v "$(dump_unpacker)" >/dev/null 2>&1 || die "'$(dump_unpacker)' is needed to unpack $IMPORT_DB"
}
# The dump, unpacked
dump_unpacker() {
    case $IMPORT_DB in *.gz) echo gzip ;; *.zst) echo zstd ;; *.xz) echo xz ;; *.bz2) echo bzip2 ;; *) echo cat ;; esac
}
dump_cat() { local d; d=$(dump_unpacker); if [ "$d" = cat ]; then cat "$IMPORT_DB"; else "$d" -dc "$IMPORT_DB"; fi; }
# The database user that owns the objects in a plain SQL dump (the old site's user), if it names one
dump_owner() {
    { dump_cat 2>/dev/null | head -c 4000000 | grep -m 1 -oE 'OWNER TO "?[A-Za-z_][A-Za-z0-9_]*' \
        | sed -E 's/OWNER TO "?//'; } || true
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
as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }
SUDO=
DOCKER_GROUP_NOTE=
if ! command docker info >/dev/null 2>&1; then
    [ "$(id -u)" = 0 ] || command -v sudo >/dev/null 2>&1 \
        || die "can't talk to Docker. Is it running, and is your user in the docker group?"
    # Not running: some distros (Arch, Fedora, ...) don't start the service when Docker is installed
    if ! as_root docker info >/dev/null 2>&1 && command -v systemctl >/dev/null 2>&1; then
        say "Starting the Docker service"
        as_root systemctl enable --now docker.service || true
        for i in $(seq 1 15); do as_root docker info >/dev/null 2>&1 && break; sleep 1; done
    fi
    as_root docker info >/dev/null 2>&1 \
        || die "can't talk to Docker, is it running? (sudo systemctl enable --now docker)"
    if ! command docker info >/dev/null 2>&1; then
        # Docker works, but only as root: let this user use it too (from the next login on); this
        # run goes on through sudo
        SUDO=sudo
        me=$(id -un)
        if [[ " $(id -nG "$me") " == *" docker "* ]]; then
            DOCKER_GROUP_NOTE="You are in the docker group, but this login doesn't have it yet: log out and back in (or run: newgrp docker) to use docker without sudo."
        else
            warn "your user '$me' can't use Docker without sudo (it is not in the docker group)"
            ans=n; [ -t 0 ] && read -r -p "Add '$me' to the docker group? (docker access equals root access) [Y/n] " ans && ans=${ans:-y}
            if [[ $ans =~ ^[Yy] ]]; then
                getent group docker >/dev/null 2>&1 || sudo groupadd docker
                sudo usermod -aG docker "$me"
                DOCKER_GROUP_NOTE="Added '$me' to the docker group: log out and back in (or run: newgrp docker) to use docker without sudo; until then use sudo."
            else
                DOCKER_GROUP_NOTE="Your user can't use docker without sudo; to change that: sudo usermod -aG docker $me, then log in again."
            fi
        fi
    fi
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
    # Database user and name: f0ckm, or for an imported site the user its dump was made with
    if [ -n "$IMPORT_DB" ]; then
        owner=$(dump_owner)
        ask POSTGRES_USER "Database user" "${owner:-f0ckm}"
    fi
    POSTGRES_USER=${POSTGRES_USER:-f0ckm}
    POSTGRES_DB=${POSTGRES_DB:-$POSTGRES_USER}
    for v in "$POSTGRES_USER" "$POSTGRES_DB"; do
        [[ $v =~ ^[a-z_][a-z0-9_]*$ ]] || die "database user/name '$v' may only use a-z, 0-9 and _"
    done
    DB_PASS=${POSTGRES_PASSWORD:-$(secret 32)}
    # An existing database keeps the password it was created with (the old default was f0ckm)
    if [ -z "${POSTGRES_PASSWORD:-}" ] && [ -n "$(ls -A f0ckm-data/postgres 2>/dev/null)" ]; then
        warn "f0ckm-data/postgres already holds a database; keeping its default password 'f0ckm'"
        DB_PASS=f0ckm
    fi
    say "Writing .env"
    {
        echo "POSTGRES_USER=$POSTGRES_USER"
        echo "POSTGRES_DB=$POSTGRES_DB"
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

# The app container runs as UID:GID from .env and must be able to read config.yaml (mode 600) and
# write f0ckm-data. Without them it runs as 1000:1000, which is a stranger to an installing user with
# another uid. The checkout's owner is used (also under sudo); a root-owned checkout gets 1000.
OWNER=$(stat -c %u:%g .)
if ! grep -q '^UID=' .env; then
    if [ "${OWNER%%:*}" = 0 ]; then ids=1000:1000; else ids=$OWNER; fi
    printf 'UID=%s\nGID=%s\n' "${ids%%:*}" "${ids#*:}" | { if [ -w .env ]; then cat >> .env; else as_root tee -a .env >/dev/null; fi; }
fi

# main.url.domain from config.yaml (empty when unreadable or still the example)
config_domain() {
    [ -r config.yaml ] || return 0
    awk '/^main:/ { m = 1; next } /^[^ #]/ { m = 0 } m && /^  url:/ { u = 1; next } m && u && /^  [^ ]/ { u = 0 }
         m && u && /^    domain:/ { sub(/^    domain:[ \t]*/, ""); gsub(/["'"'"' \t\r]/, ""); print; exit }' config.yaml \
        | grep -v '^\(example\.com\|localhost\)$' || true
}
# The public domain; older .env files (own proxy) have no VIRTUAL_HOST, then config.yaml's
SITE_DOMAIN=${DOMAIN:-$(config_domain)}

# Bot protection: Anubis runs from docker-compose.anubis.yml, named in COMPOSE_FILE next to the main file
anubis_on() { grep -q '^COMPOSE_FILE=.*docker-compose\.anubis\.yml' .env; }
env_sed() { if [ -w .env ]; then sed -i "$1" .env; else as_root sed -i "$1" .env; fi; }
env_add() { printf '%s\n' "$1" | { if [ -w .env ]; then cat >> .env; else as_root tee -a .env >/dev/null; fi; }; }
env_get() { sed -n "s/^$1=//p" .env | tail -n 1; }
env_set() { env_sed "/^$1=/d"; env_add "$1=$2"; }

# ── Postgres version ─────────────────────────────────────────────────────────────
# f0ckm-data/postgres only works with the Postgres major version that made it; docker-compose.yml runs
# postgres:$PG_MAJOR from .env. New databases start on PG_TARGET, older ones are upgraded to it.
PG_TARGET=19
PGDATA_DIR=f0ckm-data/postgres
PGU_DIR=f0ckm-data/pg-upgrade
PGU_FROM=       # set while an upgrade has moved the old folder away (the EXIT trap puts it back)
PGU_DONE=       # the version upgraded from, for the closing notes
# f0ckm-data belongs to the app user (or root); Postgres' folder in it to the container's postgres user
fs() { if [ -w f0ckm-data ]; then "$@"; else as_root "$@"; fi; }
db_running() { [ "$(docker inspect -f '{{.State.Running}}' f0ckm-db 2>/dev/null)" = true ]; }
db_wait() {
    local i
    for i in $(seq 1 60); do
        [ "$(docker inspect -f '{{.State.Health.Status}}' f0ckm-db 2>/dev/null)" = healthy ] && return 0
        sleep 2
    done
    return 1
}
# The major version of the database in f0ckm-data/postgres, empty when there is none yet. The folder is
# the container user's (mode 700): read through the running container, otherwise as root.
pg_data_version() {
    [ -e "$PGDATA_DIR" ] || return 0
    if db_running && docker exec f0ckm-db test -f /data/postgres/PG_VERSION 2>/dev/null; then
        docker exec f0ckm-db cat /data/postgres/PG_VERSION
    elif as_root test -f "$PGDATA_DIR/PG_VERSION"; then
        as_root cat "$PGDATA_DIR/PG_VERSION"
    fi
}
# Every table's exact row count ("table|rows" lines), to compare a database before and after
pg_counts() {
    local q
    q=$(docker exec f0ckm-db psql -U "$1" -d "$2" -v ON_ERROR_STOP=1 -Atqc "SELECT string_agg(format('SELECT %L, count(*) FROM %I.%I', tablename, schemaname, tablename), ' UNION ALL ' ORDER BY tablename) FROM pg_tables WHERE schemaname = 'public'")
    docker exec f0ckm-db psql -U "$1" -d "$2" -v ON_ERROR_STOP=1 -Atqc "$q ORDER BY 1"
}
# Put the kept folder of Postgres $1 back in place of the one Postgres $2 made (kept as well) and start
# everything on $1 again
db_restore_old() {
    local from=$1 to=$2 why=$3
    docker stop f0ckm >/dev/null 2>&1 || true
    compose stop f0ckm-db >/dev/null 2>&1 || true
    if [ -e "$PGDATA_DIR" ]; then fs mv "$PGDATA_DIR" "$PGDATA_DIR-$to-$why-$(date +%Y%m%d-%H%M%S)"; fi
    fs mv "$PGDATA_DIR-$from" "$PGDATA_DIR"
    env_set PG_MAJOR "$from"
    compose up -d
}
# EXIT trap while an upgrade runs: anything that ends the run (an error, die, Ctrl-C) after the old
# folder was moved away puts it back, so the site runs on the old Postgres as before
pg_upgrade_failed() {
    local rc=$?
    trap - EXIT
    [ -n "$PGU_FROM" ] || exit "$rc"
    set +e
    warn "the upgrade to Postgres $PG_TARGET did not finish; going back to Postgres $PGU_FROM"
    if db_restore_old "$PGU_FROM" "$PG_TARGET" failed; then
        warn "back on Postgres $PGU_FROM with the database as it was; the upgrade's dump is in $PGU_DIR/"
        warn "nothing is upgraded until you run ./install.sh again (PG_UPGRADE=0 in .env: never)"
    else
        warn "going back failed too. By hand: $COMPOSE down; mv $PGDATA_DIR $PGDATA_DIR-$PG_TARGET-failed;"
        warn "mv $PGDATA_DIR-$PGU_FROM $PGDATA_DIR; set PG_MAJOR=$PGU_FROM in .env; $COMPOSE up -d"
    fi
    exit "$rc"
}

# --rollback-db: back to the folder an upgrade kept (everything written since the upgrade is lost)
if [ -n "$ROLLBACK_DB" ]; then
    pg_now=$(pg_data_version)
    [ -n "$pg_now" ] || die "there is no database in $PGDATA_DIR"
    pg_old=
    for d in "$PGDATA_DIR"-*; do
        # only postgres-<version> (not postgres-19-failed-<date> and the like)
        [[ ${d##*/} =~ ^postgres-([0-9]+)$ ]] || continue
        v=${BASH_REMATCH[1]}
        [ "$v" -lt "$pg_now" ] && { [ -z "$pg_old" ] || [ "$v" -gt "$pg_old" ]; } && pg_old=$v
    done
    [ -n "$pg_old" ] || die "no kept database of an older Postgres ($PGDATA_DIR-<version>) to go back to"
    warn "going back from Postgres $pg_now to the Postgres $pg_old database kept by the upgrade:"
    warn "everything written since the upgrade (uploads, comments, accounts, ...) is lost"
    ans=; [ -t 0 ] && read -r -p "Type 'rollback' to continue: " ans
    [ "$ans" = rollback ] || [ "${ROLLBACK_DB_CONFIRM:-}" = 1 ] || die "rollback cancelled, nothing was changed (unattended: ROLLBACK_DB_CONFIRM=1)"
    db_restore_old "$pg_old" "$pg_now" rolledback
    # Or the next ./install.sh would upgrade it straight away again
    env_set PG_UPGRADE 0
    say "Back on Postgres $pg_old; the Postgres $pg_now folder is kept as $(ls -d "$PGDATA_DIR-$pg_now"-rolledback-* | tail -n 1)"
    echo "    PG_UPGRADE=0 is set in .env, so ./install.sh stays on Postgres $pg_old; ./install.sh --upgrade-db upgrades again"
    exit 0
fi
# Asked once per install (fresh or one from before Anubis); ANUBIS= in .env keeps the answer
if [ -z "${ANUBIS:-}" ] && [ -n "$SITE_DOMAIN" ] && ! anubis_on && ! grep -q '^ANUBIS=' .env && [ -t 0 ]; then
    echo "Bot protection (Anubis) for $SITE_DOMAIN: browsers solve a short proof of work once before they get in."
    echo "  Stops scrapers, but visitors need JavaScript (the .onion address works without)."
    [ "$PROXY" = own ] && echo "  Your proxy then forwards to 127.0.0.1:8923 instead of :1337 (an nginx on this server is set up for that)."
    read -r -p "Turn it on? [y/N]: " ans
    case ${ans:-n} in y*|Y*|j*|J*) ANUBIS=1 ;; *) ANUBIS=0 ;; esac
fi
if [ -n "${ANUBIS:-}" ]; then
    case $ANUBIS in 0|1) ;; *) die "ANUBIS must be 1 or 0, not '$ANUBIS'" ;; esac
    grep -qx "ANUBIS=$ANUBIS" .env || { env_sed '/^ANUBIS=/d'; env_add "ANUBIS=$ANUBIS"; }
fi
ANUBIS_REMOVE=
if [ "${ANUBIS:-}" = 1 ] && ! anubis_on; then
    say "Turning on the Anubis bot protection"
    env_sed '/^COMPOSE_FILE=/d'
    env_add "COMPOSE_FILE=docker-compose.yml:docker-compose.anubis.yml"
    # Signs Anubis' pass cookies; a fixed key keeps them valid when the container restarts
    grep -q '^ANUBIS_KEY=.' .env || env_add "ANUBIS_KEY=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
elif [ "${ANUBIS:-}" = 0 ] && anubis_on; then
    say "Turning off the Anubis bot protection"
    env_sed '/^COMPOSE_FILE=/d'
    # Removed once the proxy goes to f0ckm again (after the start below)
    ANUBIS_REMOVE=1
fi

# ── Own nginx ────────────────────────────────────────────────────────────────────
# An nginx on this server (PROXY=own) gets its site for the domain pointed at Anubis (:8923) or f0ckm
# (:1337), see scripts/own-nginx.awk. Planned and shown here, written after the start (nginx_apply).
NGINX=$(command -v nginx 2>/dev/null || true)
[ -z "$NGINX" ] && [ -x /usr/sbin/nginx ] && NGINX=/usr/sbin/nginx
NGX_FILES=()      # config files to change, with their new and old text in $NGX_DIR/<n>.new / .old
NGX_DIR=
NGX_STATE=        # ok: the site for the domain goes to the right port (after nginx_apply)
NGX_NAMES=        # the server names of that site
NGX_LOOK=         # copy of the check page's look nginx can read, when it can't read this checkout
LOOK_COPY=        # 1: use that copy (nginx was found unable to serve the files from the checkout)
nginx_plan() {
    local port=1337 dump f i css img u info names subf
    [ "${PROXY:-}" = own ] && [ -n "$SITE_DOMAIN" ] && [ -n "$NGINX" ] && [ "${NGINX_EDIT:-}" != 0 ] || return 0
    [ "$(id -u)" = 0 ] || command -v sudo >/dev/null 2>&1 || return 0
    anubis_on && port=8923
    dump=$(as_root "$NGINX" -T 2>/dev/null) || { warn "nginx -T failed, so your nginx is left alone; point it at 127.0.0.1:$port by hand"; return 0; }
    [ -z "$NGX_DIR" ] || rm -rf "$NGX_DIR"
    NGX_DIR=$(mktemp -d); info=$NGX_DIR/info; : > "$info"; NGX_FILES=()
    # Anubis' check page in f0ckm's look, read by the nginx workers straight from this checkout
    css=$PWD/public/s/css/anubis.css; img=$PWD/config/anubis/img
    u=$(ps -o user= -C nginx 2>/dev/null | grep -vx root | head -n 1 || true)
    [ -n "$u" ] || u=$(printf '%s\n' "$dump" | awk '$1 == "user" { sub(/;.*/, "", $2); print $2; exit }')
    # Once the copy is in use it stays in use (the check below found nginx can't read the checkout)
    printf '%s\n' "$dump" | grep -qF "/var/lib/f0ckm-anubis/$SITE_DOMAIN/" && LOOK_COPY=1
    if [ "$port" = 8923 ] && { [ -n "$LOOK_COPY" ] || { [ -n "$u" ] && id "$u" >/dev/null 2>&1 \
            && ! as_root su -s /bin/sh -c 'test -r "$0" && test -r "$1/checking.webp"' "$u" "$css" "$img" 2>/dev/null; }; }; then
        # Not readable for the workers (a home folder, SELinux): they get a copy, refreshed on every run
        NGX_LOOK=/var/lib/f0ckm-anubis/$SITE_DOMAIN
        css=$NGX_LOOK/anubis.css; img=$NGX_LOOK
    fi
    # The sub module rewrites the check page's file URLs (scripts/own-nginx.awk); without it nginx -t would fail
    subf=; as_root "$NGINX" -V 2>&1 | grep -q -- --with-http_sub_module && subf=1
    i=0
    while IFS= read -r f; do
        # As root: the folder may be closed to this user
        as_root test -f "$f" || continue
        i=$((i + 1))
        as_root cat "$f" > "$NGX_DIR/$i.old" || continue
        awk -v dom="$SITE_DOMAIN" -v port="$port" -v css="$css" -v img="$img" -v info="$info" -v subf="$subf" \
            -f scripts/own-nginx.awk "$NGX_DIR/$i.old" > "$NGX_DIR/$i.new" || continue
        cmp -s "$NGX_DIR/$i.old" "$NGX_DIR/$i.new" || NGX_FILES+=("$i:$f")
    done < <(printf '%s\n' "$dump" | sed -n 's/^# configuration file \(.*\):$/\1/p' | sort -u)

    names=$(sed -n 's/^names //p' "$info" | tr ' ' '\n' | grep -E '^[A-Za-z0-9.-]+\.[A-Za-z0-9-]+$' | sort -u | tr '\n' ' ' || true)
    NGX_NAMES=${names% }
    # Hints only when the proxy has to change (Anubis on, or just turned off)
    if [ -z "$NGX_NAMES" ]; then
        [ "$port" = 1337 ] && [ -z "$ANUBIS_REMOVE" ] && return 0
        warn "your nginx has no server_name $SITE_DOMAIN, so it is left alone; point your proxy at 127.0.0.1:$port by hand"
        NGX_FILES=()
        return 0
    fi
    if ! grep -qx backend "$info"; then
        [ "$port" = 1337 ] && [ -z "$ANUBIS_REMOVE" ] && return 0
        warn "nginx serves $SITE_DOMAIN, but not with proxy_pass http://127.0.0.1:1337 (an upstream {} or another address?); point it at 127.0.0.1:$port by hand"
        NGX_FILES=()
        return 0
    fi
    grep -qx noxrealip "$info" && warn "nginx: a location for $SITE_DOMAIN that proxies to f0ckm sets no X-Real-IP; Anubis needs it: add proxy_set_header X-Real-IP \$remote_addr; there"
    NGX_STATE=ok
    [ ${#NGX_FILES[@]} -gt 0 ] || return 0

    if [ "$port" = 8923 ]; then say "Your nginx serves $SITE_DOMAIN; it has to go to Anubis (127.0.0.1:8923) now:"
    else say "Your nginx serves $SITE_DOMAIN; it has to go straight to f0ckm (127.0.0.1:1337) again:"; fi
    for f in "${NGX_FILES[@]}"; do
        diff -u --label "${f#*:}" --label "${f#*:}" "$NGX_DIR/${f%%:*}.old" "$NGX_DIR/${f%%:*}.new" || true
    done
    [ -z "$NGX_LOOK" ] || echo "    (nginx ($u) can't read this checkout, so the check page's look is copied to $NGX_LOOK)"
    ans=${NGINX_EDIT:-}
    if [ -z "$ans" ] && [ -t 0 ]; then read -r -p "Change your nginx like this (checked with nginx -t, then reloaded)? [Y/n] " ans; ans=${ans:-y}; fi
    if [[ $ans =~ ^[Yy1] ]]; then
        NGINX_EDIT=1
    else
        NGX_FILES=(); NGX_STATE=
        [ -t 0 ] || warn "nginx left alone (NGINX_EDIT=1 ./install.sh applies the changes above)"
    fi
}
# Writes the planned changes once the port they point at answers; puts the old files back when nginx -t fails
nginx_apply() {
    local f out
    [ ${#NGX_FILES[@]} -gt 0 ] || return 0
    if anubis_on; then
        for i in $(seq 1 30); do
            [ "$(docker inspect -f '{{.State.Health.Status}}' f0ckm-anubis 2>/dev/null)" = healthy ] && break
            [ "$i" = 30 ] && { warn "Anubis did not come up, so nginx still goes to f0ckm; see: $COMPOSE logs anubis"; NGX_STATE=; return 0; }
            sleep 2
        done
    fi
    look_copy
    for f in "${NGX_FILES[@]}"; do as_root tee "${f#*:}" < "$NGX_DIR/${f%%:*}.new" >/dev/null; done
    if out=$(as_root "$NGINX" -t 2>&1); then
        if command -v systemctl >/dev/null 2>&1 && as_root systemctl is-active --quiet nginx; then
            as_root systemctl reload nginx
        else
            as_root "$NGINX" -s reload
        fi
        if anubis_on; then say "nginx: $SITE_DOMAIN now goes through Anubis"; else say "nginx: $SITE_DOMAIN goes straight to f0ckm again"; fi
    else
        for f in "${NGX_FILES[@]}"; do as_root tee "${f#*:}" < "$NGX_DIR/${f%%:*}.old" >/dev/null; done
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        warn "nginx -t failed with the change, so your nginx config was put back as it was; change it by hand (example below)"
        NGX_STATE=
    fi
}
# The copy of the look nginx reads when it can't read this checkout, labelled for nginx under SELinux
look_copy() {
    [ -n "$NGX_LOOK" ] || return 0
    as_root install -d -m 755 "$NGX_LOOK"
    as_root install -m 644 public/s/css/anubis.css config/anubis/img/checking.webp config/anubis/img/passed.webp \
        config/anubis/img/denied.webp "$NGX_LOOK/"
    if command -v selinuxenabled >/dev/null 2>&1 && selinuxenabled; then
        as_root chcon -R -t httpd_sys_content_t "$(dirname "$NGX_LOOK")" 2>/dev/null || true
    fi
}
# Whether nginx answers the check page's style with f0ckm's (0), with something else: Anubis' own, an
# error page (1), or can't be asked (2). Asked on this server first, then through DNS.
look_served() {
    local url="https://$SITE_DOMAIN/.within.website/x/xess/xess.min.css?install-check" body
    command -v curl >/dev/null 2>&1 || return 2
    # nginx -s reload returns before the new workers run: the old ones may answer for a moment
    for i in 1 2 3 4 5 6; do
        body=$(curl -sk --max-time 10 --resolve "$SITE_DOMAIN:443:127.0.0.1" "$url" 2>/dev/null) \
            || body=$(curl -s --max-time 10 "$url" 2>/dev/null) || return 2
        [ "$body" = "$(cat public/s/css/anubis.css)" ] && return 0
        sleep 1
    done
    return 1
}
nginx_plan
# A copy of the look is refreshed on every run, also when nginx needs no change
[ ${#NGX_FILES[@]} -gt 0 ] || [ "$NGX_STATE" != ok ] || look_copy

# After the check Anubis only sends visitors back to ANUBIS_DOMAINS, else VIRTUAL_HOST; with neither,
# every visitor gets "redirect domain not allowed". Older .env files (own proxy) may lack both, and an
# own nginx may serve more names (www.) than VIRTUAL_HOST.
if anubis_on && ! grep -q '^ANUBIS_DOMAINS=.' .env; then
    doms=$(printf '%s\n' $NGX_NAMES | paste -sd, -)
    if [ -n "$doms" ] && [ "$doms" != "$SITE_DOMAIN" ]; then
        say "Anubis: sending visitors back to $doms after the check (ANUBIS_DOMAINS in .env)"
        env_add "ANUBIS_DOMAINS=$doms"
    elif ! grep -q '^VIRTUAL_HOST=.' .env; then
        if [ -n "$SITE_DOMAIN" ]; then
            say "Anubis: sending visitors back to $SITE_DOMAIN after the check (ANUBIS_DOMAINS in .env)"
            env_add "ANUBIS_DOMAINS=$SITE_DOMAIN"
        else
            warn "set ANUBIS_DOMAINS=<your domain> in .env, or visitors get 'redirect domain not allowed' after the Anubis check"
        fi
    fi
fi
APP_UID=$(sed -n 's/^UID=//p' .env | tail -n 1)
APP_GID=$(sed -n 's/^GID=//p' .env | tail -n 1); APP_GID=${APP_GID:-$APP_UID}
# A .env written under sudo belongs to root; `docker compose` run by the user later has to read it
[ "$(stat -c %u:%g .env)" = "$OWNER" ] || as_root chown "$OWNER" .env

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
# f0ckm reads it as the app user
if [ "$(stat -c %u:%g config.yaml)" != "$APP_UID:$APP_GID" ]; then
    say "Giving config.yaml to the app user ($APP_UID:$APP_GID, UID/GID in .env)"
    if [ "$(stat -c %u config.yaml)" = "$(id -u)" ] && [ "$APP_UID" = "$(id -u)" ]; then
        chgrp "$APP_GID" config.yaml
    else
        as_root chown "$APP_UID:$APP_GID" config.yaml
    fi
fi

# ── Build and start ──────────────────────────────────────────────────────────────
say "Building and starting (the first build compiles the server and takes a few minutes)"
export GIT_HASH="$(git rev-parse --short HEAD 2>/dev/null)"
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
    local user name tmp tables ans magic owner
    pg() { docker exec -i -e PGOPTIONS='-c client_min_messages=warning' f0ckm-db "$@"; }
    sql() { pg psql -U "$user" -d "$1" -v ON_ERROR_STOP=1 -Atqc "$2"; }

    say "Importing $IMPORT_DB"
    compose up -d f0ckm-db
    for i in $(seq 1 60); do
        [ "$(docker inspect -f '{{.State.Health.Status}}' f0ckm-db 2>/dev/null)" = healthy ] && break
        [ "$i" = 60 ] && die "the database did not start; see: $COMPOSE logs f0ckm-db"
        sleep 2
    done
    # The user and database the container runs with (from .env), which are also what f0ckm connects with
    user=$(docker exec f0ckm-db printenv POSTGRES_USER); name=$(docker exec f0ckm-db printenv POSTGRES_DB)
    tmp=${name}_import
    # Postgres only creates POSTGRES_USER when it sets up an empty f0ckm-data/postgres; a folder made
    # by another setup keeps its own users, and f0ckm couldn't log in to it either
    if ! sql postgres "SELECT 1" >/dev/null 2>&1; then
        owner=$(dump_owner)
        if [ -n "$owner" ] && [ "$owner" != "$user" ] && pg psql -U "$owner" -d postgres -Atqc "SELECT 1" >/dev/null 2>&1; then
            owner="It has the user '$owner' instead: set POSTGRES_USER=$owner (and its POSTGRES_PASSWORD) in .env,"
        else
            owner="Set POSTGRES_USER/POSTGRES_PASSWORD in .env to the user it was created with,"
        fi
        die "the database in f0ckm-data/postgres has no user '$user' (POSTGRES_USER in .env): another setup created it.
       $owner
       or start with a new database (the dump brings the data): $COMPOSE down,
       mv f0ckm-data/postgres f0ckm-data/postgres.old, and run the installer again"
    fi

    tables=0
    if sql postgres "SELECT 1 FROM pg_database WHERE datname = '$name'" | grep -q 1; then
        tables=$(sql "$name" "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
    fi
    if [ "$tables" != 0 ] && [ "${IMPORT_DB_REPLACE:-}" != 1 ]; then
        warn "the database '$name' already holds $tables tables, the import replaces all of it"
        ans=; [ -t 0 ] && read -r -p "Type 'replace' to continue: " ans
        [ "$ans" = replace ] || die "import cancelled, the database was not changed (unattended: IMPORT_DB_REPLACE=1)"
    fi

    # Loaded into a separate database in one transaction; the current one is only replaced once it worked
    pg dropdb -U "$user" --if-exists "$tmp"
    pg createdb -U "$user" "$tmp"
    magic=$(dump_cat 2>/dev/null | head -c 5 || true)
    if [ "$magic" = PGDMP ]; then
        dump_cat | pg pg_restore -U "$user" -d "$tmp" --no-owner --no-acl --single-transaction --exit-on-error
    else
        # Drop the OWNER TO lines (outside COPY data), so this install's database user owns everything
        dump_cat \
            | awk '/^COPY .* FROM stdin;$/ { c = 1 } c { print; if ($0 == "\\.") c = 0; next } /^ALTER .* OWNER TO [^ ]+;$/ { next } { print }' \
            | pg psql -U "$user" -d "$tmp" -q -v ON_ERROR_STOP=1 --single-transaction >/dev/null
    fi || { pg dropdb -U "$user" --if-exists "$tmp"; die "the import failed (see above); the database was not changed"; }
    sql "$tmp" "SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'items'" | grep -q 1 \
        || { pg dropdb -U "$user" --if-exists "$tmp"; die "$IMPORT_DB has no items table, is it an f0ckm database dump?"; }

    # f0ckm must not use the database while it is swapped (it is started again below)
    docker stop f0ckm >/dev/null 2>&1 || true
    sql postgres "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)"
    sql postgres "ALTER DATABASE \"$tmp\" RENAME TO \"$name\""
    say "Imported $(sql "$name" "SELECT count(*) FROM items") items and $(sql "$name" "SELECT count(*) FROM \"user\"") users${PGU_FROM:+ into Postgres $PG_TARGET}"
    [ -n "$PGU_FROM" ] && return
    echo "    f0ckm updates the schema on its first start."
    echo "    Uploaded files are not part of the dump: copy them into f0ckm-data/ (folders: docker-compose.yml)"
}

# Upgrade the database from Postgres $1 to $2: dump with the new pg_dump, move the old folder aside,
# restore into a new cluster (import_db), compare every table's row count. Up to the move nothing is
# changed; after it, any failure puts the old folder back (pg_upgrade_failed).
pg_upgrade_db() {
    local from=$1 to=$2 user name size free need dump
    say "Upgrading the database from Postgres $from to $to"
    echo "    (dump and restore; the old database stays in $PGDATA_DIR-$from for ./install.sh --rollback-db)"
    [ ! -e "$PGDATA_DIR-$from" ] || die "$PGDATA_DIR-$from already exists (kept by an earlier upgrade?); move it elsewhere, then run ./install.sh again. Nothing was changed"
    docker pull -q "postgres:$to" >/dev/null || die "can't download postgres:$to; nothing was changed"
    { compose up -d f0ckm-db && db_wait; } || die "the Postgres $from database did not start (see: $COMPOSE logs f0ckm-db); nothing was changed"
    user=$(docker exec f0ckm-db printenv POSTGRES_USER); name=$(docker exec f0ckm-db printenv POSTGRES_DB)

    # Room for the dump and the new database next to the kept old one
    size=$(docker exec f0ckm-db psql -U "$user" -d "$name" -Atqc "SELECT pg_database_size(current_database())")
    free=$(( $(df -Pk f0ckm-data | awk 'NR == 2 { print $4 }') * 1024 ))
    need=$(( size * 2 + 512 * 1024 * 1024 ))
    [ "$free" -ge "$need" ] || die "the upgrade needs $(( need / 1048576 )) MB free next to f0ckm-data, there are $(( free / 1048576 )) MB; nothing was changed"

    # The dump holds the whole database (password hashes too): readable by this user only
    fs mkdir -p "$PGU_DIR"
    [ -O "$PGU_DIR" ] || as_root chown "$(id -u):$(id -g)" "$PGU_DIR"
    chmod 700 "$PGU_DIR"
    dump=$PGU_DIR/$name-pg$from-$(date +%Y%m%d-%H%M%S).dump

    say "Stopping f0ckm and dumping the database (with the pg_dump of Postgres $to)"
    docker stop f0ckm >/dev/null 2>&1 || true
    pg_counts "$user" "$name" > "$PGU_DIR/counts-pg$from.txt" \
        || { docker start f0ckm >/dev/null 2>&1; die "counting the tables failed; nothing was changed"; }
    # The newer pg_dump reads the old server (Postgres recommends it), in the database container's network
    docker run --rm --network container:f0ckm-db -e PGPASSWORD="$(docker exec f0ckm-db printenv POSTGRES_PASSWORD)" \
        "postgres:$to" pg_dump -h 127.0.0.1 -U "$user" -d "$name" -Fc > "$dump" \
        && docker run --rm -i --network none "postgres:$to" pg_restore -l < "$dump" >/dev/null \
        || { docker start f0ckm >/dev/null 2>&1; die "the dump failed (see above); nothing was changed, f0ckm runs on Postgres $from again"; }
    chmod 600 "$dump"
    say "Dumped $(( $(stat -c %s "$dump") / 1048576 )) MB to $dump"

    say "Moving the Postgres $from database to $PGDATA_DIR-$from and starting Postgres $to"
    compose stop f0ckm-db >/dev/null
    PGU_FROM=$from
    trap pg_upgrade_failed EXIT
    fs mv "$PGDATA_DIR" "$PGDATA_DIR-$from"
    env_set PG_MAJOR "$to"
    IMPORT_DB=$dump IMPORT_DB_REPLACE=1 import_db

    say "Updating the planner statistics and comparing every table"
    docker exec f0ckm-db vacuumdb -U "$user" -d "$name" --analyze-in-stages -q
    pg_counts "$user" "$name" > "$PGU_DIR/counts-pg$to.txt"
    if ! diff "$PGU_DIR/counts-pg$from.txt" "$PGU_DIR/counts-pg$to.txt" >&2; then
        die "the restored database differs from the old one (above: < Postgres $from, > Postgres $to)"
    fi
    trap - EXIT
    PGU_FROM=
    PGU_DONE=$from
    say "Database upgraded to Postgres $to ($(wc -l < "$PGU_DIR/counts-pg$to.txt") tables, every row count the same)"
}

# Which Postgres runs the database: pinned in .env, upgraded when older than PG_TARGET
pg_have=$(pg_data_version)
if [ -z "$pg_have" ]; then
    # No database yet: Postgres creates it, in the current version
    [ "$(env_get PG_MAJOR)" = "$PG_TARGET" ] || env_set PG_MAJOR "$PG_TARGET"
else
    [[ $pg_have =~ ^[0-9]+$ ]] || die "can't tell which Postgres made $PGDATA_DIR (PG_VERSION says '$pg_have')"
    if [ "$(env_get PG_MAJOR)" != "$pg_have" ]; then
        [ -z "$(env_get PG_MAJOR)" ] || warn "PG_MAJOR=$(env_get PG_MAJOR) in .env, but Postgres $pg_have made the database; using $pg_have"
        env_set PG_MAJOR "$pg_have"
    fi
    if [ "$pg_have" -lt "$PG_TARGET" ]; then
        if [ "${PG_UPGRADE:-$(env_get PG_UPGRADE)}" = 0 ] && [ -z "$UPGRADE_DB" ]; then
            say "Staying on Postgres $pg_have (PG_UPGRADE=0 in .env); ./install.sh --upgrade-db moves it to $PG_TARGET"
        else
            pg_upgrade_db "$pg_have" "$PG_TARGET"
            env_sed '/^PG_UPGRADE=/d'
        fi
    fi
fi

[ -n "$IMPORT_DB" ] && import_db

compose up -d

# A container that exits gets restarted (restart: unless-stopped), so a crash shows as a restart
restarts() { docker inspect -f '{{.RestartCount}}' f0ckm 2>/dev/null || echo 0; }
crashed() {
    [ "$(restarts)" != "$RESTARTS" ] || [ "$(docker inspect -f '{{.State.Status}}' f0ckm 2>/dev/null)" = exited ] || return 1
    echo >&2; docker logs --tail 20 f0ckm 2>&1 | sed 's/^/    /' >&2
    die "f0ckm crashed on start (its log above); fix that and run ./install.sh again"
}
RESTARTS=$(restarts)
say "Waiting for f0ckm to answer"
for i in $(seq 1 90); do
    crashed || true
    code=$(docker exec f0ckm curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:1337/ 2>/dev/null || true)
    [[ $code =~ ^[1-5][0-9][0-9]$ ]] && break
    [ "$i" = 90 ] && die "f0ckm did not come up; see: ${SUDO:+sudo }$COMPOSE logs f0ckm"
    sleep 2
done
sleep 2; crashed || true

nginx_apply
# The check page's look has to actually come out of nginx. Without read access to this checkout (a home
# folder, SELinux) nginx answers 403 and the page keeps Anubis' own look: then it gets a copy to read.
if anubis_on && [ "$NGX_STATE" = ok ]; then
    rc=0; look_served || rc=$?
    if [ "$rc" = 1 ] && [ -z "$LOOK_COPY" ]; then
        say "nginx doesn't serve the check page's look from this checkout; giving it a copy in /var/lib/f0ckm-anubis"
        LOOK_COPY=1
        nginx_plan
        nginx_apply
        rc=0; look_served || rc=$?
    fi
    case $rc in
        0) say "Anubis' check page has f0ckm's look" ;;
        1) warn "nginx doesn't answer /.within.website/x/xess/xess.min.css with public/s/css/anubis.css, so Anubis' check page keeps its own look (README: Bot protection)" ;;
    esac
fi
if [ -n "$ANUBIS_REMOVE" ]; then
    [ "$NGX_STATE" = ok ] || [ "${PROXY:-}" != own ] || [ -z "$SITE_DOMAIN" ] \
        || warn "Anubis is gone now: point your proxy back at 127.0.0.1:1337"
    docker rm -f f0ckm-anubis >/dev/null 2>&1 || true
fi
[ -z "$NGX_DIR" ] || rm -rf "$NGX_DIR"

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
elif [ -n "$SITE_DOMAIN" ] && [ "$NGX_STATE" = ok ]; then
    say "Done: https://$SITE_DOMAIN"
    if anubis_on; then echo "    Your nginx sends it to the Anubis bot protection (127.0.0.1:8923), which passes visitors on to f0ckm."
    else echo "    Your nginx sends it to f0ckm (127.0.0.1:1337)."; fi
elif [ -n "$SITE_DOMAIN" ]; then
    if anubis_on; then port=8923; else port=1337; fi
    say "Done: f0ckm runs on http://127.0.0.1:$port; point your proxy for https://$SITE_DOMAIN at it"
    [ "$port" = 8923 ] && echo "    (that is the Anubis bot protection, which passes visitors on to f0ckm on :1337)"
    echo "    nginx site for it (adjust the certificate paths; other proxies need the same headers,"
    echo "    WebSocket upgrade and upload limits). Also in config/nginx/own-proxy.example.conf:"
    echo
    # The example without its header comment, for this domain (and port); with Anubis also the gate
    # keeper images from this checkout, otherwise without them
    if [ "$port" = 8923 ]; then
        keeper=(-e "s|/path/to/f0ckm|$PWD|g" -e 's|^    # location = /\.within\.website|    location = /.within.website|')
    else
        keeper=(-e '/With Anubis: its check page/,/^$/d')
    fi
    sed -e '1,/^$/d' -e "s/f0ck\.example/$SITE_DOMAIN/g" -e '/# f0ckm; with Anubis/d' "${keeper[@]}" \
        -e "s|proxy_pass http://127.0.0.1:1337;|proxy_pass http://127.0.0.1:$port;|" config/nginx/own-proxy.example.conf
    echo
else
    say "Done: http://localhost:1337"
fi
echo "    Logs: ${SUDO:+sudo }$COMPOSE logs -f f0ckm    Settings: config.yaml (picked up live)"
if [ -n "$PGU_DONE" ]; then
    echo
    say "The database now runs on Postgres $PG_TARGET"
    echo "    Kept for going back: $PGDATA_DIR-$PGU_DONE (./install.sh --rollback-db; loses what was written since)"
    echo "    and the dump in $PGU_DIR/. Once the site works, free the space: sudo rm -rf $PGDATA_DIR-$PGU_DONE $PGU_DIR"
fi
[ -z "$DOCKER_GROUP_NOTE" ] || warn "$DOCKER_GROUP_NOTE"
