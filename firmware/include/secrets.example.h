// Скопируй этот файл в secrets.h (он не попадает в git) и заполни.
#pragma once

#define WIFI_SSID "MyWiFi"
#define WIFI_PASSWORD "password"

// Адрес сервера AI-Column. Пустая строка — найти сервер в сети автоматически (mDNS _aicolumn._tcp).
#define SERVER_HOST ""
#define SERVER_PORT 8080
#define SERVER_USE_TLS 0  // 1 — wss:// (например, сервер за обратным прокси в интернете)

// Тот же AUTH_TOKEN, что в server/.env
#define DEVICE_TOKEN ""
