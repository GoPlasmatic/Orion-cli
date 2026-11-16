# Builds from this repository's root:
#   docker build -t orion-cli .
# The orion-* crates arrive as git dependencies on the Orion tag Cargo.toml
# names, fetched during `cargo chef cook`; nothing outside this repository is
# copied into the context.
#
# Digest-pinned to the same base images as the Orion CLI image always used so
# a rebuild months from now resolves the same layers; dependabot bumps the
# tag+digest.
FROM rust:1.100-slim@sha256:e9d95b74296f7126a83ae3d041aeaba1ff4c478f13abc5cc034fcf7c131c3d4a AS chef
# git: cargo fetches the orion-* git dependencies with the git CLI. cmake,
# g++ and pkg-config for the -sys crates (aws-lc-sys, the rustls crypto
# provider, among them) — the server image's builder line, plus git. Keep in
# step with .github/actions/apt-build-deps and [dist.dependencies.apt].
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates pkg-config cmake g++ \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN cargo install cargo-chef --locked
WORKDIR /app

# Planner stage: generate a recipe for dependency caching
FROM chef AS planner
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
RUN cargo chef prepare --recipe-path recipe.json

# Builder stage: cache dependencies, then build
FROM chef AS builder

# CI's no-push image build relaxes this to keep a PR build fast. The default
# is the shipped setting, and the release workflow passes nothing.
ARG CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
ENV CARGO_PROFILE_RELEASE_CODEGEN_UNITS=${CARGO_PROFILE_RELEASE_CODEGEN_UNITS}

# Cook dependencies (cached unless the manifest/lockfile change). --locked to
# match the build below.
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --bin orion-cli --recipe-path recipe.json

# Build application (only this layer rebuilds on source changes)
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/

# .dockerignore excludes .git/, so build.rs cannot ask git for the commit;
# CI passes it as a build-arg (--build-arg GIT_HASH=<sha>) and build.rs
# prefers the env var. Left unset, `orion-cli --version` reports unknown.
ARG GIT_HASH
ENV GIT_HASH=${GIT_HASH}

RUN cargo build --release --locked --bin orion-cli

# Runtime stage
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a

LABEL org.opencontainers.image.title="Orion CLI" \
      org.opencontainers.image.description="Command-line interface for the Orion declarative services runtime" \
      org.opencontainers.image.source="https://github.com/GoPlasmatic/Orion-cli" \
      org.opencontainers.image.vendor="Plasmatic" \
      org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

RUN groupadd --system orion && useradd --system --gid orion --no-create-home orion

WORKDIR /app
RUN mkdir -p /home/orion/.orion && chown -R orion:orion /home/orion /app

COPY --from=builder --chown=orion:orion /app/target/release/orion-cli /usr/local/bin/orion-cli
# The image redistributes the Apache-2.0 binary, so it carries the licence.
COPY LICENSE NOTICE /usr/share/doc/orion-cli/

USER orion

ENTRYPOINT ["orion-cli"]
