# syntax=docker/dockerfile:1

# ── 1. f0ckm binary ──────────────────────────────────────────────────────────────
# Rust on Alpine targets musl and links statically, so the binary runs on the plain Alpine runtime.
FROM rust:1.90-alpine3.22 AS build
RUN apk add --no-cache musl-dev gcc
WORKDIR /build/server
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/server/target \
    cargo build --release --locked \
    && cp target/release/f0ckm /usr/local/bin/f0ckm

# ── 2. Ruffle (Flash emulator, served at /s/ruffle/) ─────────────────────────────
# Pinned npm release tarball, SHA-256 checked before it is unpacked (see scripts/fetch-ruffle.sh).
FROM alpine:3.23 AS ruffle
RUN apk add --no-cache curl
ARG RUFFLE_VERSION=0.2.0-nightly.2026.4.21
ARG RUFFLE_SHA256=44a803178047324e35b7a85a7ddf37da8a66a93a840e9f57f83ce74f8e83c6df
COPY scripts/fetch-ruffle.sh /usr/local/bin/fetch-ruffle
RUN RUFFLE_VERSION="$RUFFLE_VERSION" RUFFLE_SHA256="$RUFFLE_SHA256" sh /usr/local/bin/fetch-ruffle /ruffle

# ── 3. Runtime ───────────────────────────────────────────────────────────────────
FROM alpine:3.23
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
COPY --from=ruffle /ruffle ./public/s/ruffle
RUN f0ckm build-css

# Project root (config.yaml, views/, public/, …). CLI commands run in the same place:
#   docker exec -it f0ckm f0ckm help
ENV F0CKM_BASE=/opt/f0ckm
EXPOSE 1337
ENTRYPOINT ["f0ckm"]
