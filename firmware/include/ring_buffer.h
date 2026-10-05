// Кольцевой буфер «один писатель — один читатель» без блокировок.
// Писатель и читатель могут жить в разных задачах FreeRTOS / на разных ядрах.
#pragma once
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

class RingBuffer {
 public:
  RingBuffer() = default;
  RingBuffer(uint8_t* storage, size_t capacity) { attach(storage, capacity); }

  void attach(uint8_t* storage, size_t capacity) {
    buf_ = storage;
    cap_ = capacity;
    head_.store(0);
    tail_.store(0);
  }

  bool valid() const { return buf_ != nullptr && cap_ > 1; }
  size_t capacity() const { return cap_ ? cap_ - 1 : 0; }

  size_t available() const {
    size_t h = head_.load(std::memory_order_acquire);
    size_t t = tail_.load(std::memory_order_acquire);
    return h >= t ? h - t : cap_ - t + h;
  }

  size_t freeSpace() const { return capacity() - available(); }

  // Пишет сколько влезет, возвращает число записанных байт.
  size_t write(const uint8_t* data, size_t len) {
    if (!valid()) return 0;
    size_t n = len < freeSpace() ? len : freeSpace();
    size_t h = head_.load(std::memory_order_relaxed);
    size_t first = n < cap_ - h ? n : cap_ - h;
    memcpy(buf_ + h, data, first);
    memcpy(buf_, data + first, n - first);
    head_.store((h + n) % cap_, std::memory_order_release);
    return n;
  }

  // Читает до len байт, возвращает число прочитанных.
  size_t read(uint8_t* out, size_t len) {
    if (!valid()) return 0;
    size_t avail = available();
    size_t n = len < avail ? len : avail;
    size_t t = tail_.load(std::memory_order_relaxed);
    size_t first = n < cap_ - t ? n : cap_ - t;
    memcpy(out, buf_ + t, first);
    memcpy(out + first, buf_, n - first);
    tail_.store((t + n) % cap_, std::memory_order_release);
    return n;
  }

  // Сброс — вызывать, когда писатель гарантированно не пишет (или со стороны читателя для «выкинуть всё»).
  void clear() { tail_.store(head_.load(std::memory_order_acquire), std::memory_order_release); }

 private:
  uint8_t* buf_ = nullptr;
  size_t cap_ = 0;
  std::atomic<size_t> head_{0};
  std::atomic<size_t> tail_{0};
};
