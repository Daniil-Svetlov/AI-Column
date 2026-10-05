// Напряжение аккумулятора 3S через делитель 100k/22k на GPIO1 (ADC1).
#pragma once
#include <Arduino.h>

namespace battery {

void begin();
void loop();             // раз в ~0,5 с обновляет показания
bool present();          // есть ли АКБ/делитель (иначе питание от USB)
float volts();
int percent();           // -1, если АКБ нет
bool low();              // пора заряжать
bool critical();         // держится ниже критического порога — надо выключаться

}  // namespace battery
