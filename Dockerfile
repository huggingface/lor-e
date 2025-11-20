FROM lukemathwalker/cargo-chef:latest-rust-1.91-bookworm AS chef
WORKDIR /usr/src

ENV SCCACHE=0.12.0
ENV RUSTC_WRAPPER=/usr/local/bin/sccache
ENV SCCACHE_DIR=/sccache

# Donwload, configure sccache
RUN curl -fsSL https://github.com/mozilla/sccache/releases/download/v$SCCACHE/sccache-v$SCCACHE-x86_64-unknown-linux-musl.tar.gz | tar -xzv --strip-components=1 -C /usr/local/bin sccache-v$SCCACHE-x86_64-unknown-linux-musl/sccache && \
    chmod +x /usr/local/bin/sccache

FROM chef AS planner

COPY issue-bot ./

RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder

ARG GIT_SHA
ARG DOCKER_LABEL

ARG DATABASE_URL

COPY --from=planner /usr/src/recipe.json recipe.json

RUN --mount=type=cache,target=/sccache \
    cargo chef cook --release --recipe-path recipe.json && sccache -s

COPY issue-bot ./

RUN --mount=type=cache,target=/sccache \
    cargo build --release && sccache -s

FROM debian:bookworm-slim AS runtime

RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/target/release/issue-bot /usr/local/bin/issue-bot
COPY issue-bot/configuration/ ./configuration

ENTRYPOINT ["issue-bot"]
