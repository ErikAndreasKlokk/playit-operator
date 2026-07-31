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
# distroless ships ca-certificates and a nonroot user (uid 65532) with no shell
# or package manager — smaller and no apt/useradd step to flake on. The `cc`
# variant provides glibc for the dynamically linked binary.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/playit-operator /usr/local/bin/playit-operator
USER nonroot
ENTRYPOINT ["/usr/local/bin/playit-operator"]
