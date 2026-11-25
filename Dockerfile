FROM node:26-bookworm-slim AS web
RUN npm install -g pnpm@12
WORKDIR /src/web
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm exec vite build

FROM rust:1-bookworm AS rust
RUN rustup target add x86_64-unknown-linux-musl \
 && curl -fsSL https://ziglang.org/download/0.17.1/zig-x86_64-linux-0.17.1.tar.xz | tar -xJ -C /opt \
 && ln -s /opt/zig-x86_64-linux-0.17.1/zig /usr/local/bin/zig \
 && cargo install cargo-zigbuild --locked
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY crates crates
COPY machines machines
COPY --from=web /src/web/dist web/dist
RUN cargo zigbuild --release --target x86_64-unknown-linux-musl -p corra-hub -p corra-runner

FROM golang:1-bookworm AS go
WORKDIR /src
COPY helpers/modal ./
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /corra-modal .

FROM golang:1-bookworm AS litestream
RUN curl -fsSL -o /tmp/ls.tar.gz https://github.com/benbjohnson/litestream/releases/download/v0.5.18/litestream-0.5.18-linux-x86_64.tar.gz \
 && echo "d3687b9fd0f2e9426bbd3da828429ff3f1dfc5c9b74d54fdf028e4ee510e7dea  /tmp/ls.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/ls.tar.gz -C /tmp litestream

FROM gcr.io/distroless/static-debian12
COPY --from=rust /src/target/x86_64-unknown-linux-musl/release/corra-hub /corra/corra-hub
COPY --from=rust /src/target/x86_64-unknown-linux-musl/release/corra-runner /corra/corra-runner
COPY --from=go /corra-modal /corra/corra-modal
COPY --from=litestream /tmp/litestream /corra/litestream
ENV CORRA_DATA=/data PORT=8080
EXPOSE 8080
ENTRYPOINT ["/corra/corra-hub", "serve"]
