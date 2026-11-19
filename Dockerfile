# syntax=docker/dockerfile:1

# ==========================================
# Stage 1: Build Vue 3 + Vite Frontend (native on build host)
# ==========================================
FROM --platform=$BUILDPLATFORM node:22-alpine AS frontend-builder
ARG APP_VERSION=""
ARG GIT_SHA=""
ARG GIT_BRANCH=""
ENV APP_VERSION=${APP_VERSION} \
    GIT_SHA=${GIT_SHA} \
    GIT_BRANCH=${GIT_BRANCH}
WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
RUN npm run build

# ==========================================
# Stage 2: Cross-Compile Rust Backend (native on build host)
# ==========================================
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS rust-builder
ARG TARGETARCH
WORKDIR /app

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar

RUN if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get update && \
      apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross && \
      rm -rf /var/lib/apt/lists/* && \
      rustup target add aarch64-unknown-linux-gnu; \
    fi

# Pre-cache Cargo dependencies
COPY Cargo.toml Cargo.lock ./
RUN case "$TARGETARCH" in \
      amd64) RUST_TARGET="x86_64-unknown-linux-gnu" ;; \
      arm64) RUST_TARGET="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported TARGETARCH: $TARGETARCH" && exit 1 ;; \
    esac && \
    mkdir -p src && echo "fn main() {}" > src/main.rs && \
    cargo build --release --target "$RUST_TARGET" && \
    rm -rf src

# Copy actual source, migrations, and built frontend assets
COPY src ./src
COPY migrations ./migrations
COPY --from=frontend-builder /app/web_ui ./web_ui

RUN case "$TARGETARCH" in \
      amd64) RUST_TARGET="x86_64-unknown-linux-gnu" ;; \
      arm64) RUST_TARGET="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported TARGETARCH: $TARGETARCH" && exit 1 ;; \
    esac && \
    touch src/main.rs && \
    cargo build --release --target "$RUST_TARGET" && \
    cp "target/${RUST_TARGET}/release/rx-emby" /app/rx-emby

# ==========================================
# Stage 3: Minimal Runtime Image
# ==========================================
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates tzdata && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /app/rx-emby /app/rx-emby
COPY deploy/config.example.toml /app/config.example.toml

RUN mkdir -p /app/cache/images

ENV RUST_LOG=rx_emby=info \
    TZ=Asia/Shanghai \
    MALLOC_ARENA_MAX=2 \
    MALLOC_MMAP_THRESHOLD_=131072

EXPOSE 8096

ENTRYPOINT ["/app/rx-emby"]
