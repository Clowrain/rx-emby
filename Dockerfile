# syntax=docker/dockerfile:1

# ==========================================
# Stage 1: Build Vue 3 + Vite Frontend
# ==========================================
FROM node:22-alpine AS frontend-builder
WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
RUN npm run build

# ==========================================
# Stage 2: Build Rust Backend (with embedded web_ui)
# ==========================================
FROM rust:1-bookworm AS rust-builder
WORKDIR /app

# Pre-cache Cargo dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy actual source, migrations, and built frontend assets
COPY src ./src
COPY migrations ./migrations
COPY --from=frontend-builder /app/web_ui ./web_ui

RUN touch src/main.rs && cargo build --release

# ==========================================
# Stage 3: Minimal Runtime Image
# ==========================================
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates tzdata && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /app/target/release/rx-emby /app/rx-emby
COPY deploy/config.example.toml /app/config.example.toml

RUN mkdir -p /app/cache/images

ENV RUST_LOG=rx_emby=info \
    TZ=Asia/Shanghai

EXPOSE 8096

ENTRYPOINT ["/app/rx-emby"]
