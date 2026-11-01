# syntax=docker/dockerfile:1

# ── 1. f0ckm binary ──────────────────────────────────────────────────────────────
# Rust on Alpine targets musl and links statically, so the binary runs on the plain Alpine runtime.
# OpenSSL (needed by webauthn-rs for passkeys) is linked statically too.
FROM rust:1.98-alpine3.22 AS build
RUN apk add --no-cache musl-dev gcc openssl-dev openssl-libs-static pkgconf
ENV OPENSSL_STATIC=1
WORKDIR /build/server
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/server/target \
    cargo build --release --locked \
    && cp target/release/f0ckm /usr/local/bin/f0ckm

# ── 2. Browser libraries (jszip, marked, Cap captcha, Ruffle) ───────────────────
# Versions and checksums come from package-lock.json; scripts/vendor.mjs copies the served files.
FROM node:24-alpine AS vendor
WORKDIR /vendor
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts --no-audit --no-fund
COPY scripts/vendor.mjs ./scripts/
RUN node scripts/vendor.mjs /out

# ── 3. Runtime ───────────────────────────────────────────────────────────────────
FROM alpine:3.24
ARG GIT_HASH=unknown
ENV GIT_HASH=$GIT_HASH

# deno: JavaScript runtime yt-dlp needs for YouTube (main.ytdlp_js_runtime, default deno).
# tzdata: local time for the `TZ` setting (Node carried its own time zone data).
RUN apk add --no-cache \
    ffmpeg \
    yt-dlp \
    deno \
    ffmpegthumbnailer \
    imagemagick \
    ghostscript \
    mailcap \
    file \
    curl \
    torsocks \
    exiftool \
    tzdata \
    ca-certificates

# curl-impersonate: reproduces real browser TLS/HTTP2 handshakes for outgoing 4chan requests
# (picked up automatically via PATH). Pinned release + SHA-256 per arch; unsupported
# architectures skip it and the app falls back to plain curl with a random User-Agent.
ARG CURL_IMPERSONATE_VERSION=v2.2.3
ARG CURL_IMPERSONATE_SHA256_X86_64=288332a313e9edd884a1575c2627844fd9f3388fcefc62982b6b4eae12828357
ARG CURL_IMPERSONATE_SHA256_AARCH64=18000c51542fe63f0acc5cd3d89fb2217e35574f4c2911aec33e6774973a0809
ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH:-$(uname -m)}" in \
      amd64|x86_64)  arch=x86_64;  sha="$CURL_IMPERSONATE_SHA256_X86_64" ;; \
      arm64|aarch64) arch=aarch64; sha="$CURL_IMPERSONATE_SHA256_AARCH64" ;; \
      *) echo "curl-impersonate: no build for ${TARGETARCH:-$(uname -m)}, skipping"; exit 0 ;; \
    esac; \
    curl -fsSL -o /tmp/curl-impersonate.tgz \
      "https://github.com/lexiforest/curl-impersonate/releases/download/${CURL_IMPERSONATE_VERSION}/curl-impersonate-${CURL_IMPERSONATE_VERSION}.${arch}-linux-musl.tar.gz"; \
    echo "${sha}  /tmp/curl-impersonate.tgz" | sha256sum -c -; \
    mkdir -p /opt/curl-impersonate; \
    tar xzf /tmp/curl-impersonate.tgz -C /opt/curl-impersonate; \
    rm /tmp/curl-impersonate.tgz; \
    /opt/curl-impersonate/curl-impersonate --version | head -n 1
ENV PATH="/opt/curl-impersonate:${PATH}"

WORKDIR /opt/f0ckm
COPY --from=build /usr/local/bin/f0ckm /usr/local/bin/f0ckm
COPY config_example.yaml ./
COPY migrations ./migrations
COPY locales ./locales
COPY views ./views
COPY public ./public
COPY --from=vendor /out/ ./public/
RUN f0ckm build-css

# Project root (config.yaml, views/, public/, …). CLI commands run in the same place:
#   docker exec -it f0ckm f0ckm help
ENV F0CKM_BASE=/opt/f0ckm
EXPOSE 1337
ENTRYPOINT ["f0ckm"]
