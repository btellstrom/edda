# One image with all four services; compose.yaml picks which one each
# container runs. Works with `docker build` and `podman build`.

FROM docker.io/library/rust:1.98-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
        -p auth-service -p character-service -p combat-service -p monster-service \
    && mkdir /out \
    && cp target/release/auth-service target/release/character-service \
        target/release/combat-service target/release/monster-service /out/

FROM docker.io/library/debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home-dir /data campaign \
    && mkdir /data && chown campaign /data
COPY --from=build /out/ /usr/local/bin/
USER campaign
WORKDIR /data
