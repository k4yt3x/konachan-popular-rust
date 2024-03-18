FROM rust:1.76.0-alpine3.19 as builder
COPY . /app
WORKDIR /app
RUN apk add --no-cache --virtual .build-deps \
        make \
        musl-dev \
        openssl-dev \
        perl \
        pkgconfig \
    && cargo build --release --target x86_64-unknown-linux-musl

FROM gcr.io/distroless/static:nonroot
LABEL maintainer="K4YT3X <i@k4yt3x.com>" \
      org.opencontainers.image.source="https://github.com/k4yt3x/konachan-popular-rust" \
      org.opencontainers.image.description="The backend of the Telegram channel @KonachanPopular"
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/konachan-popular \
                    /usr/local/bin/konachan-popular
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/konachan-popular"]
