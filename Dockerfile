# ---- Builder Stage ----
FROM --platform=linux/amd64 rust:1.81 AS builder

# Install additional build dependencies required by aws-lc-sys
RUN apt-get update && \
    apt-get install -y \
      cmake \
      pkg-config \
      git \
      build-essential \
      ninja-build \
      python3 \
      libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

ARG SQLX_OFFLINE
ENV SQLX_OFFLINE=${SQLX_OFFLINE}

# Copy the entire source code into the container
COPY . .

# Build the application in release mode
RUN cargo build --release --bin wld-usernames

# ---- Runtime Stage ----
FROM --platform=linux/amd64 debian:bookworm-slim AS runtime

# Install runtime dependencies (CA certificates and libssl3 for OpenSSL 3)
RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

# Set the working directory in the runtime container
WORKDIR /app

# Copy the built binary from the builder stage
COPY --from=builder /app/target/release/wld-usernames /usr/local/bin/wld-usernames

# Expose the port your server listens on 
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/wld-usernames"]

HEALTHCHECK --interval=5m \
    CMD curl -f http://localhost:8000/ || exit 1
