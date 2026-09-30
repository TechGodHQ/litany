FROM rust:1-slim-bookworm AS builder

WORKDIR /src
COPY . .
RUN cargo build --release --locked -p litany-server --bin litany-http

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/litany litany \
    && mkdir --parents /data \
    && chown --recursive litany:litany /data /home/litany

COPY --from=builder /src/target/release/litany-http /usr/local/bin/litany-http

ENV LITANY_DB=/data/litany.db
ENV LITANY_HTTP_BIND=0.0.0.0:8941

EXPOSE 8941
USER litany
ENTRYPOINT ["/usr/local/bin/litany-http"]
