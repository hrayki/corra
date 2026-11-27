FROM rust:1-bookworm AS launcher
RUN rustup target add x86_64-unknown-linux-musl \
 && apt-get update \
 && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY crates crates
RUN cargo build --release --locked --target x86_64-unknown-linux-musl -p corra-launcher

FROM debian:bookworm-slim AS litestream
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && curl -fsSL -o /tmp/ls.tar.gz https://github.com/benbjohnson/litestream/releases/download/v0.5.18/litestream-0.5.18-linux-x86_64.tar.gz \
 && echo "d3687b9fd0f2e9426bbd3da828429ff3f1dfc5c9b74d54fdf028e4ee510e7dea  /tmp/ls.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/ls.tar.gz -C /tmp litestream

FROM gcr.io/distroless/static-debian12
COPY --from=launcher /src/target/x86_64-unknown-linux-musl/release/corra-launcher /corra/corra-launcher
COPY --from=litestream /tmp/litestream /corra/litestream
ENV CORRA_DATA=/data PORT=8080
EXPOSE 8080
ENTRYPOINT ["/corra/corra-launcher"]
