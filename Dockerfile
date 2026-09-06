# Deployment image for the live service — runs the pre-built static musl
# binary committed at bin/live-trader-x86_64 (built & tested in CI/sandbox;
# see rust/README.md for the build command). Zero-compile deploys: fast,
# deterministic, and safe on Render's no-cache build profile.
#
# For a self-contained build-from-source image use Dockerfile.build instead.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY bin/live-trader-x86_64 /usr/local/bin/live-trader
RUN chmod +x /usr/local/bin/live-trader
ENV HOST=0.0.0.0
EXPOSE 10000
USER 10001
CMD ["live-trader"]
