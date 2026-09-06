# Multi-stage build: static musl binary -> tiny runtime image (~15MB total).
# The same binary is also committed at bin/live-trader-x86_64 so the live
# Render service (created pre-Docker, python runtime) can run it via
# startCommand without changing its runtime — see README "Deployment".
FROM rust:1.88-slim AS build
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY dashboard ./dashboard
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/x86_64-unknown-linux-musl/release/live-trader /usr/local/bin/live-trader
ENV HOST=0.0.0.0
EXPOSE 10000
CMD ["live-trader"]
