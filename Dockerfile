# syntax=docker/dockerfile:1
# PennyChest as one static binary with the frontend inside it, in an image with nothing else
# but CA certificates. Every stage that builds runs on the build machine's own platform and
# cross-compiles, so an arm64 image builds as quickly as an amd64 one, without emulation:
#
#   docker buildx build --platform linux/amd64,linux/arm64 -t pennychest .

# Stage 1: the React frontend, which is static files.
FROM --platform=$BUILDPLATFORM node:20-slim AS frontend
WORKDIR /frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --legacy-peer-deps
COPY frontend/ ./
RUN npm run build

# Stage 2: the server, linked statically against musl. cargo-zigbuild uses zig as the C
# compiler and linker for the target, for the C in SQLite and ring.
FROM --platform=$BUILDPLATFORM rust:1.97-bookworm AS server
ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-pip \
 && pip3 install --no-cache-dir --break-system-packages cargo-zigbuild==0.20.1 ziglang==0.13.0.post1 \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The target is added to the toolchain rust-toolchain.toml pins, which is the one used here.
COPY rust-toolchain.toml ./
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac > /rust-target \
 && rustup target add "$(cat /rust-target)"
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY tools tools
COPY --from=frontend /frontend/dist frontend/dist
# Both platforms build at once: they share the registry one at a time, and each has its own
# target directory.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=target-$TARGETARCH \
    cargo zigbuild --release --locked -p pennychest-server --bin pennychest \
      --target "$(cat /rust-target)" \
 && cp "target/$(cat /rust-target)/release/pennychest" /pennychest

# Stage 3: just the binary, and CA certificates for reaching AI providers.
FROM scratch
COPY --from=server /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=server /pennychest /pennychest
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
# The database, uploaded statements and plugins live here; mount a volume to keep them.
# The server creates what it needs on its first start, and brings the database up to date
# on every start, so there's no separate migration step.
WORKDIR /data
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/pennychest", "healthcheck"]
ENTRYPOINT ["/pennychest"]
