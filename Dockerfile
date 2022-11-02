FROM rust:alpine as builder
RUN apk add --no-cache musl-dev openssl openssl-dev pkgconfig
WORKDIR /home/rust/src
COPY . .
RUN cargo build --locked --release --features client,server,noise,hot-reload
RUN mkdir -p build-out/
RUN cp target/release/rathole build-out/

FROM scratch
WORKDIR /app
COPY --from=builder /home/rust/src/build-out/rathole .
USER 1000:1000
ENTRYPOINT ["./rathole"]
