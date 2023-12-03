FROM rust:slim-bookworm as chef
RUN cargo install cargo-chef

WORKDIR /validation-data

FROM chef as planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef as builder
COPY --from=planner /validation-data/recipe.json recipe.json
RUN apt-get update && \
    apt-get install -y \
    python3-dev \
    pkgconf \
    libssl-dev \
    && \
    apt-get clean && \
    apt-get autoremove --purge -y && \
    rm -rf /var/lib/apt/lists/*

RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim as runtime
RUN apt-get update && \
    apt-get install -y \
    ca-certificates \
    python3 \
    python3-requests \
    python3-unicorn \
    python3-plist \
    python3-distutils \
    && \
    update-ca-certificates && \
    apt-get clean && \
    apt-get autoremove --purge -y && \
    rm -rf /var/lib/apt/lists/* 

COPY --from=builder /validation-data/target/release/validation-data /usr/local/bin/validation-data
CMD ["/usr/local/bin/validation-data"]
