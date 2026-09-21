FROM rust:1.98.1-alpine3.23 AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
ARG CARGO_BUILD_JOBS=2
RUN cargo build --locked --release -j "$CARGO_BUILD_JOBS"

FROM alpine:3.23
RUN apk add --no-cache ca-certificates curl \
    && addgroup -S -g 10001 guard \
    && adduser -S -D -H -u 10001 -G guard guard
WORKDIR /app
COPY --from=build /build/target/release/leigod-guard-server /app/leigod-guard-server
COPY static ./static
USER 10001:10001
ENV LISTEN_ADDRESS=0.0.0.0 PORT=3088 STATIC_DIR=/app/static
EXPOSE 3088
STOPSIGNAL SIGINT
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=5 \
    CMD curl --fail --silent "http://127.0.0.1:${PORT}/api/health" > /dev/null || exit 1
ENTRYPOINT ["/app/leigod-guard-server"]
