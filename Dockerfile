# The evalhub image: one static-ish binary with the web UI inside it.
#
# Three stages. `web` builds the SvelteKit SPA into the server crate, which
# is where `build.rs` insists on finding it for a release build; `build`
# compiles the release binary; the final stage is a slim Debian with CA
# certificates (the hub talks TLS to Postgres and S3) and nothing else.
#
#   docker build -t evalhub .
#   docker run --rm -p 8080:8080 -e EVALHUB_DATABASE__URL=postgres://... evalhub
#
# `rust-toolchain.toml` is deliberately not copied in: the image's own
# stable toolchain is used, so the build does not download another.

FROM node:22-bookworm-slim AS web
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml ./
RUN corepack enable && corepack pnpm install --frozen-lockfile
COPY web/ ./
# svelte.config.js writes to ../crates/evalhub-server/web-dist.
RUN mkdir -p /src/crates/evalhub-server && corepack pnpm build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=web /src/crates/evalhub-server/web-dist ./crates/evalhub-server/web-dist
# No BuildKit cache mounts: the file then builds with the classic builder
# too, and CI caches layers instead.
RUN cargo build --release --locked -p evalhub-server \
    && cp target/release/evalhub /evalhub

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin evalhub
COPY --from=build /evalhub /usr/local/bin/evalhub
USER evalhub
# Inside a container the loopback default would be unreachable.
ENV EVALHUB_BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["evalhub"]
CMD ["serve"]
