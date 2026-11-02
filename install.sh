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
set -euo pipefail
cd "$(dirname "$0")"

say()  { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning: %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }
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

# ── .env ─────────────────────────────────────────────────────────────────────────
if [ -f .env ]; then
    say "Keeping existing .env"
    DOMAIN=${DOMAIN:-$(sed -n 's/^VIRTUAL_HOST=//p' .env)}
else
    ask DOMAIN "Domain (empty for a local install on localhost:1337)"
    DOMAIN=${DOMAIN#http*://}; DOMAIN=${DOMAIN%%/*}
    if [ -n "$DOMAIN" ]; then
        ask EMAIL "Email for Let's Encrypt"
        [ -n "${EMAIL:-}" ] || die "an email is needed for the Let's Encrypt certificate"
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
            echo "COMPOSE_PROFILES=f0ckm-nginx"
            echo "VIRTUAL_HOST=$DOMAIN"
            echo "LETSENCRYPT_HOST=$DOMAIN"
            echo "LETSENCRYPT_EMAIL=$EMAIL"
        fi
    } > .env
    chmod 600 .env
fi
[ "${DOMAIN:-}" = localhost ] && DOMAIN=

# ── config.yaml ──────────────────────────────────────────────────────────────────
if [ -f config.yaml ]; then
    say "Keeping existing config.yaml"
else
    say "Writing config.yaml from config_example.yaml"
    if [ -n "$DOMAIN" ]; then full="https://$DOMAIN"; host=$DOMAIN; else full="http://localhost:1337"; host=localhost; fi
    regex=$(printf '%s' "$host" | sed 's/\./\\\\./g')
    sed -e "s|^    full: https://example.com$|    full: $full|" \
        -e "s|^    domain: example.com$|    domain: $host|" \
        -e "s|^    regex: example\\\\.com$|    regex: $regex|" \
        -e "s|^  invite_secret: YOUR_SECRET_HERE$|  invite_secret: $(secret 40)|" \
        -e "s|^  development: true$|  development: false|" \
        -e "s|^  host: localhost$|  host: f0ckm-db|" \
        ${EMAIL:+-e "s|^  mail: admin@example.com$|  mail: $EMAIL|"} \
        config_example.yaml > config.yaml
    chmod 600 config.yaml
    grep -q 'YOUR_SECRET_HERE' config.yaml && warn "invite_secret was not replaced; set it in config.yaml"
fi

# ── Build and start ──────────────────────────────────────────────────────────────
say "Building and starting (the first build compiles the server and takes a few minutes)"
export GIT_HASH="f0ckm-$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
compose up -d --build

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
if [ -n "$DOMAIN" ]; then
    say "Done: https://$DOMAIN"
    echo "    The domain's DNS has to point at this server and ports 80/443 must be open;"
    echo "    the certificate is issued within a minute or two after that."
else
    say "Done: http://localhost:1337"
fi
echo "    Logs: $COMPOSE logs -f f0ckm    Settings: config.yaml (picked up live)"
