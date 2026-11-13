# Сервер AI-Column (Rust)
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
# ffmpeg нужен, только если TTS отдаёт не WAV (mp3/ogg)
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/ai-column-server /usr/local/bin/ai-column-server
COPY config ./config
COPY public ./public
EXPOSE 8080
CMD ["ai-column-server"]
