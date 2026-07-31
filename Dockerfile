# --- build stage -------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /app

# Cache dependency compilation: build against a stub main first, then the real
# sources. Any change to src/ then only recompiles this crate, not the deps.
COPY Cargo.toml ./
RUN mkdir -p src src/bin \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && echo 'fn main() {}' > src/bin/crdgen.rs \
    && cargo build --release --bin playit-operator || true
COPY . .
RUN cargo build --release --bin playit-operator

# --- runtime stage -----------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 10001 -r -s /usr/sbin/nologin operator
COPY --from=builder /app/target/release/playit-operator /usr/local/bin/playit-operator
USER 10001
ENTRYPOINT ["/usr/local/bin/playit-operator"]
