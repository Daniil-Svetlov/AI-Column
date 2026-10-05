# Piper TTS с HTTP API (POST /synthesize) и русским голосом
FROM python:3.11-slim
RUN pip install --no-cache-dir "piper-tts[http]"
ARG VOICE=ru_RU-irina-medium
ENV VOICE=${VOICE}
RUN mkdir -p /voices && python3 -m piper.download_voices --download-dir /voices ${VOICE}
EXPOSE 5000
CMD ["sh", "-c", "python3 -m piper.http_server -m \"$VOICE\" --data-dir /voices --host 0.0.0.0 --port 5000"]
