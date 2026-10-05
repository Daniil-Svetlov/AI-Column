# Сервер AI-Column
FROM node:22-bookworm-slim
# ffmpeg нужен, только если TTS отдаёт не WAV (mp3/ogg)
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev
COPY . .
ENV NODE_ENV=production
EXPOSE 8080
CMD ["node", "server.js"]
