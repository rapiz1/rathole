FROM rust:bookworm as builder
RUN apt update && apt install -y libssl-dev
WORKDIR /home/rust/src
COPY . .
ARG FEATURES
RUN cargo build --locked --release --features ${FEATURES:-default}
RUN mkdir -p build-out/
RUN cp target/release/rathole build-out/



FROM gcr.io/distroless/cc-debian12
WORKDIR /app
COPY --from=builder /home/rust/src/build-out/rathole .
USER 1000:1000
ENTRYPOINT ["./rathole"]
