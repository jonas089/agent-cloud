# The provider as it runs inside the TEE: the agentcloud-provider binary plus the Docker CLI it
# drives the CVM's Docker with. Built from this repository by deploy/phala.sh.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p agentcloud-provider

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates e2fsprogs \
 && rm -rf /var/lib/apt/lists/*
COPY --from=docker:27-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=build /src/target/release/agentcloud-provider /usr/local/bin/agentcloud-provider
ENTRYPOINT ["/usr/local/bin/agentcloud-provider"]
