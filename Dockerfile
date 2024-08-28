FROM --platform=linux/amd64 clux/muslrust AS builder
WORKDIR /wld-usernames
COPY . .
RUN cargo build --release --bin wld-usernames

FROM --platform=linux/amd64 alpine AS runtime
WORKDIR /wld-usernames
COPY --from=builder /wld-usernames/target/x86_64-unknown-linux-musl/release/wld-usernames /usr/local/bin

EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/wld-usernames"]

HEALTHCHECK --interval=5m \
    CMD curl -f http://localhost:8000/ || exit 1
