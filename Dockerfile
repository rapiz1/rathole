FROM ekidd/rust-musl-builder:latest as builder
WORKDIR /home/rust/src
COPY . .
RUN cargo build --locked --release
RUN mkdir -p build-out/
RUN cp target/x86_64-unknown-linux-musl/release/rathole build-out/

FROM scratch
WORKDIR /app
COPY --from=builder /home/rust/src/build-out/rathole .
USER 1000:1000
ENTRYPOINT ["./rathole"]
