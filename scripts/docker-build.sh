#!/usr/bin/env bash
# Builds the f0ckm image: scripts/docker-build.sh [docker build options] (e.g. -t f0ckm:latest)
#
# The Dockerfile uses BuildKit cache mounts so rebuilds reuse cargo downloads and build output.
# BuildKit needs the buildx plugin (Docker 23+) or an older Docker that still has it built in. Where
# neither works (e.g. Docker without docker-buildx, or the old docker-compose v1 builder) this falls back
# to the classic builder with the cache mounts removed: same image, but every build compiles from scratch.
set -euo pipefail
cd "$(dirname "$0")/.."

buildkit=
if docker buildx version >/dev/null 2>&1; then
    buildkit=1
else
    # Before Docker 23 BuildKit was part of the daemon and DOCKER_BUILDKIT=1 is enough
    major=$(docker version --format '{{.Server.Version}}' 2>/dev/null | cut -d. -f1)
    [[ $major =~ ^[0-9]+$ ]] && [ "$major" -ge 18 ] && [ "$major" -lt 23 ] && buildkit=1
fi

if [ -n "$buildkit" ]; then
    DOCKER_BUILDKIT=1 exec docker build -f Dockerfile "$@" .
fi

printf '\033[33mwarning: BuildKit is not available (install the docker-buildx plugin to get it);\n' >&2
printf 'building without the build cache, so every build compiles the server from scratch\033[0m\n' >&2
# "RUN --mount=... \" → "RUN \", and drop continuation lines that only hold a --mount
sed -e 's/^RUN --mount=[^ ]* \\$/RUN \\/' -e '/^[[:space:]]*--mount=[^ ]* \\$/d' Dockerfile \
    | DOCKER_BUILDKIT=0 docker build -f - "$@" .
