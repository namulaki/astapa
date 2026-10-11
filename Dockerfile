FROM rust:1.82-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy src to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src target/release/astapa

# Copy actual source
COPY src ./src

# Build for real
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -u 1000 -s /bin/false astapa

COPY --from=builder /app/target/release/astapa /usr/local/bin/

USER astapa

ENV RUST_LOG=astapa=info
ENV PROXY_ADDR=0.0.0.0:5433
ENV API_ADDR=0.0.0.0:8080

EXPOSE 5433 8080

CMD ["astapa"]
