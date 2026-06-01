# ── builder ──────────────────────────────────────────────────────────────────
FROM rust:1.79-slim AS builder

RUN apt-get update && apt-get install -y pkg-config && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies before copying source
COPY Cargo.toml ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Build real source
COPY src ./src
# Touch main.rs so cargo rebuilds it even though Cargo.toml didn't change
RUN touch src/main.rs && cargo build --release

# ── runtime ───────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/cle-trader /usr/local/bin/cle-trader

ENV RUST_LOG=info

# Binance credentials — set via fly secrets / docker -e / env file
# BINANCE_API_KEY=
# BINANCE_SECRET=

CMD ["cle-trader"]
