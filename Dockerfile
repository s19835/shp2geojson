FROM rust:latest AS builder
RUN apt-get update && apt-get install -y libproj-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY assets/ assets/
COPY .cargo/ .cargo/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends libproj25 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/shp2geojson /usr/local/bin/
ENTRYPOINT ["shp2geojson"]
