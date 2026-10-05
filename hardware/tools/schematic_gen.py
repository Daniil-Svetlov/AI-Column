#!/usr/bin/env python3
"""Генератор SVG-схемы соединений AI-Column (hardware/schematic.svg).

Запуск: python3 hardware/tools/schematic_gen.py
Скрипт сам проверяет схему: наложения проводов разных цепей, Т-стыки, провода сквозь модули.
"""
import html

W, H = 2400, 1890
FONT = "'DejaVu Sans','Segoe UI',Roboto,Arial,sans-serif"

C = dict(
    VBAT='#d62828', V5='#f08c00', V33='#e64980', GND='#212529',
    MIC='#2f9e44', DAC='#7048e8', TFT='#1c7ed6', AUD='#0c8599',
    SPK='#8d5524', LED='#c99700', BTN='#5c677d', ADC='#5c940d',
    BAL='#868e96', BATN='#495057')

NETCOL = {
    'VBAT': C['VBAT'], 'VBAT_RAW': C['VBAT'], 'VBAT_SW': C['VBAT'],
    '5V': C['V5'], '5V_ESP': C['V5'], '3V3': C['V33'], 'GND': C['GND'],
    'BATN': C['BATN'], 'B1': C['BAL'], 'B2': C['BAL'],
    'MIC_SCK': C['MIC'], 'MIC_WS': C['MIC'], 'MIC_SD': C['MIC'],
    'DAC_BCK': C['DAC'], 'DAC_LCK': C['DAC'], 'DAC_DIN': C['DAC'],
    'AUD_L': C['AUD'], 'AUD_G': C['GND'],
    'SPK_P': C['SPK'], 'SPK_N': C['SPK'],
    'T_IRQ': C['TFT'], 'MISO': C['TFT'], 'MOSI': C['TFT'], 'T_CS': C['TFT'],
    'SCK': C['TFT'], 'TFT_LED': C['TFT'], 'TFT_DC': C['TFT'],
    'TFT_RST': C['TFT'], 'TFT_CS': C['TFT'],
    'BAT_ADC': C['ADC'], 'LED_DATA': C['LED'], 'LED_Y': C['LED'],
    'LED_DIN': C['LED'], 'BTN1': C['BTN'], 'BTN2': C['BTN'],
}
POWER = {'VBAT', 'VBAT_RAW', 'VBAT_SW', '5V', '5V_ESP', 'GND', 'BATN', '3V3'}

wires, boxes = [], []
fg, top = [], []          # слои: компоненты / подписи поверх всего


def esc(s):
    return html.escape(s, quote=True)


def T(x, y, s, size=12, anchor='start', weight='normal', fill='#1f2328',
      rot=None, layer=None, italic=False):
    tr = f' transform="rotate({rot} {x} {y})"' if rot is not None else ''
    it = ' font-style="italic"' if italic else ''
    (fg if layer is None else layer).append(
        f'<text x="{x}" y="{y}" font-size="{size}" text-anchor="{anchor}" '
        f'font-weight="{weight}" fill="{fill}"{it}{tr}>{esc(s)}</text>')


def R(x, y, w, h, fill, stroke='#343a40', sw=1.5, rx=6, layer=None, extra=''):
    (fg if layer is None else layer).append(
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" '
        f'stroke="{stroke}" stroke-width="{sw}"{extra}/>')


def Ln(x1, y1, x2, y2, stroke='#343a40', sw=1.5, layer=None, extra=''):
    (fg if layer is None else layer).append(
        f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{stroke}" '
        f'stroke-width="{sw}" stroke-linecap="round"{extra}/>')


def module(x0, y0, x1, y1, fill, stroke, name, rx=8, sw=1.8):
    R(x0, y0, x1 - x0, y1 - y0, fill, stroke, sw, rx)
    boxes.append((x0, y0, x1, y1, name))


def pin(x, y, side, label, net=None, size=11, nc=False, lab='#1f2328', bold=False):
    c = NETCOL.get(net, '#adb5bd') if net else '#adb5bd'
    fg.append(f'<rect x="{x-4}" y="{y-4}" width="8" height="8" rx="1.5" fill="{c}" '
              f'stroke="#ffffff" stroke-width="1"/>')
    w = 'bold' if bold else 'normal'
    if label:
        if side == 'L':
            T(x + 9, y + 4, label, size, 'start', w, lab)
        elif side == 'R':
            T(x - 9, y + 4, label, size, 'end', w, lab)
        elif side == 'T':
            T(x, y + 17, label, size, 'middle', w, lab)
        elif side == 'B':
            T(x, y - 9, label, size, 'middle', w, lab)
    if nc:
        dx, dy = {'L': (-14, 0), 'R': (14, 0), 'T': (0, -14), 'B': (0, 14)}[side]
        ex, ey = x + dx, y + dy
        Ln(x, y, ex, ey, '#adb5bd', 1.5)
        Ln(ex - 4, ey - 4, ex + 4, ey + 4, '#e03131', 2)
        Ln(ex - 4, ey + 4, ex + 4, ey - 4, '#e03131', 2)


def wire(net, pts, w=None):
    wires.append(dict(net=net, pts=pts,
                      w=w if w else (3.4 if net in POWER else 2.4)))


def tag(x, y, s, color, size=10.5, anchor='start'):
    """Подпись к проводу на белой подложке."""
    wch = len(s) * size * 0.6 + 6
    x0 = {'start': x - 3, 'middle': x - wch / 2, 'end': x - wch + 3}[anchor]
    top.append(f'<rect x="{x0:.1f}" y="{y - size + 1:.1f}" width="{wch:.1f}" '
               f'height="{size + 3:.1f}" rx="3" fill="#ffffff" fill-opacity="0.92"/>')
    T(x, y, s, size, anchor, 'bold', color, layer=top)


# ---------------------------------------------------------------- резисторы и пр.
def resistor_v(x, y0, y1, name, val, lab_side='R'):
    """Вертикальный резистор (ГОСТ-прямоугольник) между y0 и y1."""
    Ln(x, y0, x, y0 + 8, '#343a40', 2)
    Ln(x, y1 - 8, x, y1, '#343a40', 2)
    R(x - 8, y0 + 8, 16, y1 - y0 - 16, '#ffffff', '#343a40', 2, 1)
    if lab_side == 'R':
        T(x + 14, (y0 + y1) / 2 - 2, name, 11.5, 'start', 'bold')
        T(x + 14, (y0 + y1) / 2 + 13, val, 11, 'start')


def resistor_h(x0, x1, y, name, val):
    Ln(x0, y, x0 + 8, y, '#343a40', 2)
    Ln(x1 - 8, y, x1, y, '#343a40', 2)
    R(x0 + 8, y - 7, x1 - x0 - 16, 14, '#ffffff', '#343a40', 2, 1)
    T((x0 + x1) / 2, y - 12, f'{name} {val}', 11, 'middle', 'bold')


def cap_h(xa, xb, y, polar=False):
    """Конденсатор: пластины в xa и xb (горизонтальный провод)."""
    Ln(xa, y - 11, xa, y + 11, '#343a40', 2.6)
    if polar:
        fg.append(f'<path d="M {xb+4} {y-11} Q {xb-2} {y} {xb+4} {y+11}" fill="none" '
                  f'stroke="#343a40" stroke-width="2.6"/>')
    else:
        Ln(xb, y - 11, xb, y + 11, '#343a40', 2.6)


# =========================================================== ЗАГОЛОВОК
T(40, 50, 'Умная колонка на ESP32-S3 — полная схема соединений', 27, 'start', 'bold')
T(40, 80, 'ESP32-S3-DevKitC-1 N16R8 · TFT ILI9341 + тач XPT2046 · 2× INMP441 · '
          'PCM5102A · TPA3116D2 · WS2812B 8×8 · 74AHCT125 · MP1584EN · АКБ 3S Li-ion',
  14, 'start', 'normal', '#495057')

# =========================================================== ESP32-S3
EX0, EX1, EY0, EY1 = 1010, 1230, 500, 1262


def py(n):
    return 540 + (n - 1) * 32


module(EX0, EY0, EX1, EY1, '#1d2b3a', '#0b1520', 'esp', rx=10, sw=2)
T((EX0 + EX1) / 2, 490, 'ESP32-S3-DevKitC-1 (N16R8)', 13.5, 'middle', 'bold')
T(EX0 + 8, EY0 + 20, 'J1', 11, 'start', 'bold', '#8fa3b8')
T(EX1 - 8, EY0 + 20, 'J3', 11, 'end', 'bold', '#8fa3b8')
R(1084, 548, 72, 150, '#c9ced6', '#8a939e', 1.5, 3)
T(1114, 623, 'ESP32-S3-WROOM-1', 11, 'middle', 'bold', '#343a40', rot=-90)
T(1132, 623, '16 МБ Flash · 8 МБ PSRAM', 9.5, 'middle', 'normal', '#343a40', rot=-90)
# USB-разъёмы
for ux, ul in ((1045, 'UART'), (1150, 'USB')):
    R(ux, EY1 - 10, 46, 24, '#adb5bd', '#495057', 1.5, 5)
    T(ux + 23, EY1 - 16, ul, 9.5, 'middle', 'bold', '#dee2e6')
T(1120, EY1 + 32, 'прошивка — по USB-C', 11, 'middle', 'normal', '#495057')

J1 = [('3V3', '3V3'), ('3V3', None), ('RST (EN)', None),
      ('GPIO4', 'MIC_SCK'), ('GPIO5', 'MIC_WS'), ('GPIO6', 'MIC_SD'),
      ('GPIO7', 'DAC_BCK'), ('GPIO15', 'DAC_LCK'), ('GPIO16', 'DAC_DIN'),
      ('GPIO17', 'T_IRQ'), ('GPIO18', 'MISO'), ('GPIO8', 'MOSI'),
      ('GPIO3 · strap', None), ('GPIO46 · strap', None),
      ('GPIO9', 'T_CS'), ('GPIO10', 'SCK'), ('GPIO11', 'TFT_LED'),
      ('GPIO12', 'TFT_DC'), ('GPIO13', 'TFT_RST'), ('GPIO14', 'TFT_CS'),
      ('5V', '5V_ESP'), ('GND', 'GND')]
J3 = [('GND', None), ('TX · 43', None), ('RX · 44', None),
      ('GPIO1', 'BAT_ADC'), ('GPIO2', 'LED_DATA'), ('GPIO42', 'BTN1'),
      ('GPIO41', 'BTN2'), ('GPIO40', None), ('GPIO39', None),
      ('GPIO38', None), ('PSRAM ✕ GPIO37', 'X'), ('PSRAM ✕ GPIO36', 'X'),
      ('PSRAM ✕ GPIO35', 'X'), ('BOOT · GPIO0', None), ('strap · GPIO45', None),
      ('RGB · GPIO48', None), ('GPIO47', None), ('GPIO21', None),
      ('USB D+ · GPIO20', 'X'), ('USB D− · GPIO19', 'X'), ('GND', 'GND'), ('GND', 'GND')]

for i, (lab, net) in enumerate(J1):
    y = py(i + 1)
    used = net is not None
    col = '#ffffff' if used else '#7d8fa3'
    pin(EX0, y, 'L', lab, net if used else None, 11, lab=col, bold=used)
for i, (lab, net) in enumerate(J3):
    y = py(i + 1)
    if net == 'X':
        pin(EX1, y, 'R', lab, None, 10.5, lab='#ff8787')
    else:
        used = net is not None
        col = '#ffffff' if used else '#7d8fa3'
        pin(EX1, y, 'R', lab, net if used else None, 11, lab=col, bold=used)

# =========================================================== МИКРОФОНЫ INMP441


def mic(x0, name, sub, lr_net):
    y0, y1 = 330, 420
    module(x0, y0, x0 + 160, y1, '#eaf7ee', C['MIC'], name)
    T(x0 + 80, y0 + 45, name, 12.5, 'middle', 'bold')
    T(x0 + 80, y0 + 61, sub, 10, 'middle', 'normal', '#495057')
    pin(x0 + 30, y0, 'T', 'L/R', lr_net)
    pin(x0 + 80, y0, 'T', 'VDD', '3V3')
    pin(x0 + 130, y0, 'T', 'GND', 'GND')
    pin(x0 + 30, y1, 'B', 'SCK', 'MIC_SCK')
    pin(x0 + 80, y1, 'B', 'WS', 'MIC_WS')
    pin(x0 + 130, y1, 'B', 'SD', 'MIC_SD')


mic(560, 'INMP441 · Mic B', 'L/R → 3V3 = правый', '3V3')
mic(780, 'INMP441 · Mic A', 'L/R → GND = левый', 'GND')

# 3V3 c пина 1 J1
wire('3V3', [(1010, 540), (990, 540), (990, 260), (590, 260), (590, 330)])
wire('3V3', [(640, 330), (640, 260)])
wire('3V3', [(860, 330), (860, 260)])
tag(750, 254, '+3,3 В с ESP32', C['V33'], anchor='middle')
# GND микрофонов
wire('GND', [(910, 330), (910, 194)])
wire('GND', [(810, 330), (810, 194)])          # Mic A L/R -> GND
# шина I2S0
wire('MIC_SCK', [(1010, 636), (970, 636), (970, 460), (590, 460), (590, 420)])
wire('MIC_SCK', [(810, 420), (810, 460)])
wire('MIC_WS', [(1010, 668), (950, 668), (950, 480), (640, 480), (640, 420)])
wire('MIC_WS', [(860, 420), (860, 480)])
wire('MIC_SD', [(1010, 700), (930, 700), (930, 500), (690, 500), (690, 420)])
wire('MIC_SD', [(910, 420), (910, 500)])
T(560, 540, 'I2S0 (оба микрофона на одной шине):', 11, 'start', 'bold', C['MIC'])
T(560, 556, 'SCK → GPIO4 · WS → GPIO5 · SD → GPIO6', 11, 'start', 'normal', C['MIC'])

# =========================================================== ЦАП PCM5102A
module(600, 680, 790, 935, '#f1edff', C['DAC'], 'dac')
T(695, 699, 'PCM5102A · ЦАП', 12.5, 'middle', 'bold')
T(695, 714, 'I2S1, выход — линейный', 10, 'middle', 'normal', '#495057')
pin(790, 732, 'R', 'BCK ← GPIO7', 'DAC_BCK')
pin(790, 764, 'R', 'LCK ← GPIO15', 'DAC_LCK')
pin(790, 796, 'R', 'DIN ← GPIO16', 'DAC_DIN')
pin(600, 748, 'L', 'LOUT', 'AUD_L')
pin(600, 780, 'L', 'AGND', 'AUD_G')
pin(600, 812, 'L', 'ROUT', None, nc=True)
pin(600, 844, 'L', 'SCK', 'GND')
pin(600, 876, 'L', 'GND', 'GND')
pin(600, 908, 'L', 'VIN (5 В)', '5V')
T(782, 846, 'на обороте:', 10, 'end', 'normal', '#495057')
T(782, 862, '1-L 2-L 3-H 4-L', 10.5, 'end', 'bold', C['DAC'])
T(782, 878, '(FLT DEMP XSMT FMT)', 9, 'end', 'normal', '#495057')
T(782, 898, 'SCK обязательно', 10, 'end', 'normal', '#e03131')
T(782, 912, 'на GND', 10, 'end', 'normal', '#e03131')

wire('DAC_BCK', [(1010, 732), (790, 732)])
wire('DAC_LCK', [(1010, 764), (790, 764)])
wire('DAC_DIN', [(1010, 796), (790, 796)])
wire('GND', [(600, 844), (580, 844), (580, 876)])            # SCK -> GND
wire('GND', [(600, 876), (440, 876), (440, 1470)])
wire('5V', [(600, 908), (460, 908), (460, 1492), (2340, 1492), (2340, 172),
            (1560, 172), (1560, 630)])

# =========================================================== УСИЛИТЕЛЬ TPA3116D2
module(170, 640, 410, 900, '#e6f6f7', C['AUD'], 'amp')
T(290, 662, 'TPA3116D2', 13, 'middle', 'bold')
T(290, 678, 'класс D, питание 9–12,6 В', 10, 'middle', 'normal', '#495057')
pin(410, 748, 'R', 'IN L', 'AUD_L')
pin(410, 780, 'R', 'IN GND', 'AUD_G')
pin(410, 812, 'R', 'IN R', None, nc=True)
pin(170, 700, 'L', 'L+', 'SPK_P')
pin(170, 740, 'L', 'L−', 'SPK_N')
pin(170, 800, 'L', 'R+', None, nc=True)
pin(170, 840, 'L', 'R−', None, nc=True)
pin(230, 900, 'B', 'VCC+', 'VBAT')
pin(300, 900, 'B', 'GND', 'GND')
T(290, 866, 'моно: в прошивке L = (L+R)/2', 9.5, 'middle', 'italic', '#495057')

wire('AUD_L', [(600, 748), (410, 748)])
wire('AUD_G', [(600, 780), (410, 780)], w=2.4)
tag(505, 742, 'L', C['AUD'], anchor='middle')
tag(505, 774, 'GND сигн.', C['GND'], anchor='middle')

# =========================================================== ДИНАМИК
module(30, 650, 130, 790, '#fbf4ee', C['SPK'], 'spk')
R(46, 690, 26, 60, '#8d5524', '#5c3a1a', 1.5, 2)
fg.append('<path d="M 72 690 L 108 666 L 108 774 L 72 750 Z" fill="#d9b99b" '
          'stroke="#5c3a1a" stroke-width="1.5"/>')
pin(130, 700, 'R', '', 'SPK_P')
pin(130, 740, 'R', '', 'SPK_N')
T(122, 684, '+', 13, 'end', 'bold', C['SPK'])
T(122, 766, '−', 13, 'end', 'bold', C['SPK'])
wire('SPK_P', [(170, 700), (130, 700)])
wire('SPK_N', [(170, 740), (130, 740)])
T(30, 812, 'Динамик 45 мм', 11.5, 'start', 'bold')
T(30, 828, '4 Ом · 10 Вт', 11, 'start')

T(30, 960, 'Выход усилителя мостовой:', 11, 'start', 'bold', '#e03131')
T(30, 976, 'L− — это НЕ земля,', 11, 'start', 'normal', '#e03131')
T(30, 992, 'не соединяй его с GND.', 11, 'start', 'normal', '#e03131')
T(30, 1020, 'ЦАП даёт до 2,1 В, усилитель', 11, 'start')
T(30, 1036, '×20 (26 дБ): на полной', 11, 'start')
T(30, 1052, 'громкости динамик 10 Вт', 11, 'start')
T(30, 1068, 'перегреется. Потолок', 11, 'start')
T(30, 1084, 'громкости в прошивке −16 дБ', 11, 'start')
T(30, 1100, '(VOLUME_MAX_GAIN_DB в config.h).', 11, 'start')

# =========================================================== ДИСПЛЕЙ
module(500, 968, 790, 1432, '#e7f1ff', C['TFT'], 'tft')
T(512, 988, 'ILI9341 + XPT2046', 12.5, 'start', 'bold')
R(512, 1000, 128, 416, '#243b55', '#10202f', 1.5, 4)
T(576, 1208, 'TFT 2,4″ / 2,8″ 320×240', 13, 'middle', 'bold', '#dbe7f5', rot=-90)
T(598, 1208, 'SPI, питание 5 В (перемычку J1 не замыкать)', 9.5, 'middle', 'normal',
  '#9fb6cf', rot=-90)
TFT_PINS = [('14 T_IRQ ← GPIO17', 'T_IRQ'), ('13 T_DO ← GPIO18', 'MISO'),
            ('12 T_DIN ← GPIO8', 'MOSI'), ('11 T_CS ← GPIO9', 'T_CS'),
            ('10 T_CLK ← GPIO10', 'SCK'), ('9 SDO — не подкл.', None),
            ('8 LED ← GPIO11 (PWM)', 'TFT_LED'), ('7 SCK ← GPIO10', 'SCK'),
            ('6 SDI ← GPIO8', 'MOSI'), ('5 DC ← GPIO12', 'TFT_DC'),
            ('4 RESET ← GPIO13', 'TFT_RST'), ('3 CS ← GPIO14', 'TFT_CS'),
            ('2 GND', 'GND'), ('1 VCC (5 В)', '5V')]
for k, (lab, net) in enumerate(TFT_PINS):
    pin(790, 990 + 32 * k, 'R', lab, net, 10.5, nc=(net is None))

TFT_W = [('T_IRQ', 10, 990), ('MISO', 11, 1022), ('MOSI', 12, 1054),
         ('T_CS', 15, 1086), ('SCK', 16, 1118), ('TFT_LED', 17, 1182),
         ('TFT_DC', 18, 1278), ('TFT_RST', 19, 1310), ('TFT_CS', 20, 1342)]
for k, (net, pn, yd) in enumerate(TFT_W):
    x = 800 + 18 * k
    wire(net, [(1010, py(pn)), (x, py(pn)), (x, yd), (790, yd)])
wire('MOSI', [(836, 1054), (836, 1246), (790, 1246)])
wire('SCK', [(872, 1118), (872, 1214), (790, 1214)])
wire('GND', [(790, 1374), (825, 1374), (825, 1470)])
wire('5V', [(790, 1406), (805, 1406), (805, 1492)])

# питание самой ESP32 (через D1) и её GND
wire('5V_ESP', [(1010, 1180), (968, 1180), (968, 1396)])
wire('5V', [(968, 1424), (968, 1492)])
wire('GND', [(1010, 1212), (994, 1212), (994, 1470)])
# диод D1 (анод снизу, катод сверху)
fg.append('<path d="M 968 1400 L 958 1420 L 978 1420 Z" fill="#343a40"/>')
Ln(956, 1399, 980, 1399, '#343a40', 3)
Ln(968, 1396, 968, 1400, '#343a40', 2.4)
Ln(968, 1420, 968, 1424, '#343a40', 2.4)
T(950, 1407, 'D1 SS34', 11, 'end', 'bold')
T(950, 1421, '(1N5819)', 10, 'end', 'normal', '#495057')

# =========================================================== ДЕЛИТЕЛЬ НАПРЯЖЕНИЯ АКБ
wire('VBAT', [(1300, 240), (1300, 150), (2360, 150), (2360, 1514), (230, 1514),
              (230, 900)])
resistor_v(1300, 240, 320, 'R1', '100 кОм')
wire('BAT_ADC', [(1300, 320), (1300, 420)])
wire('BAT_ADC', [(1230, 636), (1300, 636), (1300, 420)])
wire('BAT_ADC', [(1300, 420), (1340, 420)])
resistor_h(1340, 1400, 420, 'R2', '22 кОм')
wire('GND', [(1400, 420), (1440, 420)])
wire('BAT_ADC', [(1300, 480), (1366, 480)])
cap_h(1366, 1374, 480)
T(1370, 506, 'C1 100 нФ', 11, 'middle', 'bold')
wire('GND', [(1374, 480), (1440, 480), (1440, 194)])
tag(1308, 604, 'АЦП → GPIO1', C['ADC'])
T(1318, 208, 'делитель АКБ', 10.5, 'start', 'italic', '#495057')

# =========================================================== 74AHCT125
module(1500, 630, 1720, 800, '#2b2f36', '#111418', 'ls')
pin(1500, 668, 'L', '2 · 1A', 'LED_DATA', lab='#ffffff')
pin(1720, 668, 'R', '1Y · 3', 'LED_Y', lab='#ffffff')
pin(1560, 630, 'T', '14 VCC', '5V', lab='#ffffff')
T(1610, 706, '74AHCT125', 13, 'middle', 'bold', '#ffffff')
T(1610, 722, 'уровень 3,3 → 5 В', 10, 'middle', 'normal', '#ced4da')
T(1610, 742, '1,4,5,7,9,10,12,13 → GND', 10, 'middle', 'normal', '#ffd8a8')
T(1610, 757, '6, 8, 11 — не подключать', 10, 'middle', 'normal', '#ced4da')
LS_G = [1, 4, 5, 7, 9, 10, 12, 13]
for k, n in enumerate(LS_G):
    x = 1515 + 27 * k
    pin(x, 800, 'B', str(n), 'GND', 9.5, lab='#ffffff')
    wire('GND', [(x, 800), (x, 830)])
wire('GND', [(1704, 830), (1515, 830), (1515, 1470)])
T(1526, 850, 'DIP-14: ключ (выемка) — между выводами 1 и 14', 9.5, 'start', 'italic', '#495057')

wire('LED_DATA', [(1230, 668), (1500, 668)])
wire('LED_Y', [(1720, 668), (1835, 668)])
resistor_h(1835, 1895, 668, 'R3', '330 Ом')
wire('LED_DIN', [(1895, 668), (1920, 668)])
# C3 100 нФ между VCC и GND у микросхемы
wire('5V', [(1560, 560), (1676, 560)])
cap_h(1676, 1684, 560)
wire('GND', [(1684, 560), (1800, 560)])
T(1680, 540, 'C3 100 нФ', 11, 'middle', 'bold')

# =========================================================== МАТРИЦА WS2812B
module(1920, 560, 2250, 890, '#16181c', '#000000', 'mx')
pin(1920, 600, 'L', '+5V', '5V', lab='#ffffff', bold=True)
pin(1920, 630, 'L', 'GND', 'GND', lab='#ffffff', bold=True)
pin(1920, 668, 'L', 'DIN', 'LED_DIN', lab='#ffffff', bold=True)
import colorsys
for r in range(8):
    for c in range(8):
        h = ((r + c) / 14.0) % 1.0
        rr, gg, bb = colorsys.hsv_to_rgb(h, 0.75, 1.0)
        col = '#%02x%02x%02x' % (int(rr * 255), int(gg * 255), int(bb * 255))
        cx, cy = 2012 + c * 30, 598 + r * 36
        fg.append(f'<rect x="{cx-9}" y="{cy-9}" width="18" height="18" rx="3" '
                  f'fill="#f1f3f5" opacity="0.9"/><circle cx="{cx}" cy="{cy}" r="5.5" fill="{col}"/>')
T(2085, 910, 'WS2812B 8×8 (64 LED, 5 В)', 12.5, 'middle', 'bold')
wire('GND', [(1920, 630), (1800, 630), (1800, 194)])
wire('5V', [(1920, 600), (1870, 600), (1870, 172)])
wire('GND', [(1800, 520), (1831, 520)])
cap_h(1831, 1839, 520)
wire('5V', [(1839, 520), (1870, 520)])
T(1848, 510, '+', 12, 'middle', 'bold', C['V5'])
T(1880, 516, 'C2 1000 мкФ', 11, 'start', 'bold')
T(1880, 531, '(электролит, ≥10 В)', 10, 'start', 'normal', '#495057')

# =========================================================== КНОПКИ


def button(x, name, gp):
    y0, y1 = 930, 990
    Ln(x, y0, x, 945, '#343a40', 2)
    Ln(x, 975, x, y1, '#343a40', 2)
    R(x - 15, 945, 30, 30, '#e9ecef', '#495057', 1.8, 4)
    fg.append(f'<circle cx="{x}" cy="960" r="9" fill="#343a40"/>')
    for dx in (-15, 15):
        for dy in (-15, 15):
            fg.append(f'<circle cx="{x+dx}" cy="{960+dy}" r="2" fill="#868e96"/>')
    T(x + 22, 956, name, 11.5, 'start', 'bold')
    T(x + 22, 971, gp, 10.5, 'start', 'normal', '#495057')
    boxes.append((x - 15, 945, x + 15, 975, name))


button(1280, 'SW2', 'GPIO41')
button(1420, 'SW1', 'GPIO42')
wire('BTN1', [(1230, 700), (1420, 700), (1420, 930)])
wire('GND', [(1420, 990), (1420, 1212), (1230, 1212)])
wire('BTN2', [(1230, 732), (1280, 732), (1280, 930)])
wire('GND', [(1280, 990), (1280, 1180), (1230, 1180)])
T(1350, 1090, 'кнопки 12 мм', 10.5, 'middle', 'italic', '#495057')
T(1350, 1105, 'на GND, в коде', 10.5, 'middle', 'italic', '#495057')
T(1350, 1120, 'INPUT_PULLUP', 10.5, 'middle', 'italic', '#495057')
T(1350, 1140, '(ножки по диагонали)', 9.5, 'middle', 'italic', '#495057')

# =========================================================== ШИНЫ ПИТАНИЯ
# GND: усилитель -> нижняя шина -> правый край -> верх -> Mic B
wire('GND', [(300, 900), (300, 1470), (2320, 1470), (2320, 194), (690, 194), (690, 330)])
tag(1740, 1465, 'GND', C['GND'])
tag(1740, 1487, '+5 В (MP1584)', C['V5'])
tag(1740, 1509, 'VBAT +9…12,6 В (после S1 и F1)', C['VBAT'])
tag(2140, 145, 'VBAT', C['VBAT'])
tag(2140, 167, '+5 В', C['V5'])
tag(2140, 189, 'GND', C['GND'])

# =========================================================== ЗАРЯДКА / АКБ / BMS
module(30, 1580, 170, 1700, '#f8f9fa', '#495057', 'chg')
T(100, 1604, 'Зарядное', 12, 'middle', 'bold')
T(100, 1620, 'устройство', 12, 'middle', 'bold')
T(100, 1640, '12,6 В · 1–2 А', 11.5, 'middle', 'bold', C['VBAT'])
T(100, 1656, 'CC/CV для 3S', 10.5, 'middle')
T(100, 1672, 'Li-ion', 10.5, 'middle')
T(100, 1690, 'центр «+»', 10, 'middle', 'normal', '#495057')
Ln(170, 1640, 205, 1640, '#495057', 5)
module(205, 1575, 290, 1705, '#e9ecef', '#343a40', 'jack')
T(247, 1640, 'DC 5,5×2,1', 10, 'middle', 'bold', rot=-90)
pin(290, 1600, 'R', '−', 'GND', 12, bold=True)
pin(290, 1680, 'R', '+', 'VBAT_RAW', 12, bold=True)
wire('GND', [(290, 1600), (315, 1600), (315, 1545), (780, 1545)])
wire('VBAT_RAW', [(290, 1680), (300, 1680), (300, 1800), (860, 1800), (860, 1735)])

# ячейки
CELLS = [(340, 450), (470, 580), (600, 710)]
for i, (x0, x1) in enumerate(CELLS):
    R(x0, 1575, x1 - x0, 40, '#3b5bdb', '#1c2f8a', 1.5, 7)
    R(x1, 1587, 6, 16, '#adb5bd', '#495057', 1, 1)
    T((x0 + x1) / 2, 1592, f'18650 #{i+1}', 10.5, 'middle', 'bold', '#ffffff')
    T((x0 + x1) / 2, 1607, '3,6 В', 10, 'middle', 'normal', '#dbe4ff')
    T(x0 + 7, 1570, '−', 12, 'middle', 'bold', '#495057')
    T(x1 + 3, 1570, '+', 12, 'middle', 'bold', '#495057')
    boxes.append((x0, 1575, x1 + 6, 1615, f'cell{i}'))
module(320, 1660, 740, 1760, '#edf7ed', '#2b8a3e', 'bms')
T(530, 1712, 'BMS 3S (10–20 А, с балансировкой)', 12, 'middle', 'bold')
T(530, 1728, 'общий порт: заряд и нагрузка через P+/P−', 10, 'middle', 'normal', '#495057')
pin(330, 1660, 'T', 'B−', 'BATN', 10.5)
pin(463, 1660, 'T', 'B1', 'B1', 10.5)
pin(593, 1660, 'T', 'B2', 'B2', 10.5)
pin(726, 1660, 'T', 'B+', 'VBAT_RAW', 10.5)
pin(740, 1690, 'R', 'P−', 'GND', 10.5, bold=True)
pin(740, 1735, 'R', 'P+ (=B+)', 'VBAT_RAW', 10.5, bold=True)
fg.append('<path d="M 726 1680 L 726 1735 L 732 1735" fill="none" stroke="#c92a2a" '
          'stroke-width="1.2" stroke-dasharray="4 3"/>')
wire('BATN', [(340, 1595), (330, 1595), (330, 1660)])
wire('B1', [(456, 1595), (470, 1595)], w=2.4)
wire('B1', [(463, 1595), (463, 1660)], w=2)
wire('B2', [(586, 1595), (600, 1595)], w=2.4)
wire('B2', [(593, 1595), (593, 1660)], w=2)
wire('VBAT_RAW', [(716, 1595), (726, 1595), (726, 1660)])
T(530, 1782, 'АКБ 3S1P: 11,1 В номинал · 12,6 В полный · 3000–3500 мА·ч',
  11.5, 'middle', 'bold', C['VBAT'])

wire('GND', [(740, 1690), (780, 1690), (780, 1470)])
wire('VBAT_RAW', [(740, 1735), (905, 1735)])

# выключатель S1
fg.append('<circle cx="905" cy="1735" r="4" fill="#ffffff" stroke="#343a40" stroke-width="2"/>')
fg.append('<circle cx="975" cy="1735" r="4" fill="#ffffff" stroke="#343a40" stroke-width="2"/>')
Ln(908, 1733, 968, 1712, '#343a40', 2.6)
T(940, 1695, 'S1 вкл/выкл', 11.5, 'middle', 'bold')
T(940, 1765, '(≥ 3 А)', 10.5, 'middle', 'normal', '#495057')
wire('VBAT_SW', [(975, 1735), (1010, 1735)])
# предохранитель F1
R(1018, 1727, 64, 16, '#ffffff', '#343a40', 2, 2)
Ln(1010, 1735, 1090, 1735, '#343a40', 1.6)
T(1050, 1716, 'F1 3 А', 11.5, 'middle', 'bold')
T(1050, 1765, '(самовосст.', 10, 'middle', 'normal', '#495057')
T(1050, 1778, 'или стекл.)', 10, 'middle', 'normal', '#495057')
wire('VBAT', [(1090, 1735), (1150, 1735), (1150, 1514)])

# =========================================================== MP1584EN
module(1260, 1600, 1560, 1720, '#fff4e6', C['V5'], 'mp')
pin(1290, 1600, 'T', 'IN+', 'VBAT', 10.5, bold=True)
pin(1340, 1600, 'T', 'IN−', 'GND', 10.5, bold=True)
pin(1480, 1600, 'T', 'OUT−', 'GND', 10.5, bold=True)
pin(1530, 1600, 'T', 'OUT+', '5V', 10.5, bold=True)
T(1410, 1650, 'MP1584EN (DC-DC ↓)', 12.5, 'middle', 'bold')
T(1410, 1668, 'вход 9–12,6 В → выход 5,2 В', 10.5, 'middle')
T(1410, 1690, 'выстави 5,2 В ДО подключения!', 10.5, 'middle', 'bold', '#e03131')
T(1410, 1706, '(крути подстроечник, мерь мультиметром)', 9.5, 'middle', 'normal', '#495057')
wire('VBAT', [(1290, 1600), (1290, 1514)])
wire('GND', [(1340, 1600), (1340, 1470)])
wire('GND', [(1480, 1600), (1480, 1470)])
wire('5V', [(1530, 1600), (1530, 1492)])

# =========================================================== ЛЕГЕНДА
R(30, 110, 500, 516, '#fbfbfc', '#ced4da', 1.2, 10)
T(50, 138, 'Обозначения', 15, 'start', 'bold')
LEG = [(C['VBAT'], 'VBAT +9…12,6 В (АКБ / зарядка)', 3.4),
       (C['V5'], '+5 В от MP1584 (+ D1 на ESP32)', 3.4),
       (C['V33'], '+3,3 В с пина 3V3 ESP32 (только микрофоны)', 3.4),
       (C['GND'], 'GND (общий минус)', 3.4),
       (C['MIC'], 'I2S0 — микрофоны INMP441', 2.4),
       (C['DAC'], 'I2S1 — ЦАП PCM5102A', 2.4),
       (C['TFT'], 'SPI — дисплей и тачскрин (общая шина)', 2.4),
       (C['AUD'], 'аналоговый звук ЦАП → усилитель', 2.4),
       (C['SPK'], 'выход усилителя → динамик', 2.4),
       (C['LED'], 'данные WS2812B (через 74AHCT125)', 2.4),
       (C['BTN'], 'кнопки', 2.4),
       (C['ADC'], 'измерение напряжения АКБ (АЦП)', 2.4),
       (C['BAL'], 'балансировочные провода BMS', 2.4)]
for i, (col, txt, sw) in enumerate(LEG):
    y = 166 + i * 23
    Ln(50, y, 100, y, col, sw)
    T(114, y + 4, txt, 12)
y = 166 + len(LEG) * 23 + 6
Ln(50, y, 100, y, '#495057', 2.4)
Ln(75, y - 16, 75, y + 16, '#495057', 2.4)
fg.append(f'<circle cx="75" cy="{y}" r="4.5" fill="#495057"/>')
T(114, y + 4, 'точка — провода соединены (пайка / общий узел)', 12)
y += 36
Ln(50, y, 100, y, '#495057', 2.4)
fg.append(f'<path d="M 75 {y-18} L 75 {y-6} A 6 6 0 0 1 75 {y+6} L 75 {y+18}" '
          f'fill="none" stroke="#495057" stroke-width="2.4"/>')
T(114, y + 4, 'дуга — провода пересекаются, НО НЕ соединены', 12)
y += 32
Ln(62, y, 80, y, '#adb5bd', 1.5)
Ln(84, y - 4, 92, y + 4, '#e03131', 2)
Ln(84, y + 4, 92, y - 4, '#e03131', 2)
T(114, y + 4, 'вывод не подключать', 12)
T(50, y + 34, 'Шины питания здесь логические. Физически GND веди «звездой»', 11, 'start',
  'normal', '#495057')
T(50, y + 50, 'от P− BMS; усилитель и MP1584 — проводом ≥ 0,5 мм².', 11, 'start',
  'normal', '#495057')

# =========================================================== ТАБЛИЦА GPIO + ВАЖНО
R(1560, 940, 730, 500, '#fbfbfc', '#ced4da', 1.2, 10)
T(1580, 966, 'Распиновка ESP32-S3', 15, 'start', 'bold')
GP = [('GPIO4', 'MIC SCK (I2S0 BCLK)', C['MIC']),
      ('GPIO5', 'MIC WS (I2S0 LRCLK)', C['MIC']),
      ('GPIO6', 'MIC SD (I2S0 DATA IN)', C['MIC']),
      ('GPIO7', 'DAC BCK (I2S1 BCLK)', C['DAC']),
      ('GPIO15', 'DAC LCK (I2S1 WS)', C['DAC']),
      ('GPIO16', 'DAC DIN (I2S1 DATA OUT)', C['DAC']),
      ('GPIO10', 'SPI SCK → SCK + T_CLK', C['TFT']),
      ('GPIO8', 'SPI MOSI → SDI + T_DIN', C['TFT']),
      ('GPIO18', 'SPI MISO ← T_DO', C['TFT']),
      ('GPIO14', 'TFT CS', C['TFT']),
      ('GPIO12', 'TFT DC', C['TFT']),
      ('GPIO13', 'TFT RESET', C['TFT']),
      ('GPIO11', 'TFT LED (подсветка, PWM)', C['TFT']),
      ('GPIO9', 'TOUCH CS (XPT2046)', C['TFT']),
      ('GPIO17', 'TOUCH IRQ', C['TFT']),
      ('GPIO2', 'WS2812B DATA (RMT)', C['LED']),
      ('GPIO42', 'кнопка SW1', C['BTN']),
      ('GPIO41', 'кнопка SW2', C['BTN']),
      ('GPIO1', 'АКБ: АЦП (ADC1_CH0)', C['ADC'])]
for i, (g, f, col) in enumerate(GP):
    y = 994 + i * 22.5
    R(1580, y - 10, 10, 10, col, col, 1, 2)
    T(1598, y, g, 12, 'start', 'bold')
    T(1660, y, f, 12)
T(1580, 1430, 'Свободны: GPIO21, 47, 40, 39 (38 — RGB-LED на v1.1)', 10.5, 'start',
  'italic', '#495057')

T(1920, 966, 'Важно', 15, 'start', 'bold', '#e03131')
NOTES = [
    '• GPIO35–37 заняты PSRAM (N16R8),',
    '  GPIO19/20 — USB, 0/3/45/46 — strap.',
    '• SDO (MISO) дисплея не подключаем:',
    '  он мешает тачу на общей шине.',
    '• PCM5102A: SCK на GND, иначе нет',
    '  звука; XSMT = H (перемычка 3-H).',
    '• Mic A — левый канал, Mic B —',
    '  правый; SD общий. Микрофоны',
    '  подальше от динамика.',
    '• WS2812B: ограничь яркость ≈ 1 А',
    '  (FastLED setMaxPowerInMilliWatts).',
    '• D1: можно шить по USB при',
    '  включённом S1 — питание не',
    '  «встретится». Перемычку IN-OUT',
    '  на плате ESP32 не запаивать.',
    '• 3,3 В с ESP32 — только на',
    '  микрофоны (LDO платы слабый).',
    '• Сначала MP1584 → 5,2 В,',
    '  потом всё остальное.',
]
for i, s in enumerate(NOTES):
    T(1920, 992 + i * 22, s, 11.5)

# =========================================================== ПИТАНИЕ — ЗАМЕТКИ
R(1600, 1560, 780, 300, '#fff8f8', '#ffc9c9', 1.2, 10)
T(1620, 1588, 'Аккумулятор и питание', 15, 'start', 'bold', C['VBAT'])
PN = [
    ('АКБ:', '3S1P Li-ion 18650 — 11,1 В (12,6 В полный, ~9 В пустой)'),
    ('Ёмкость:', '3000–3500 мА·ч на ячейку (≈33–39 Вт·ч) → ~7–10 ч работы'),
    ('Ячейки:', 'с током ≥ 8–10 А: Samsung 35E / 30Q, LG MJ1, Molicel P28A'),
    ('BMS:', '3S 10–20 А с балансировкой, общий порт (P+ = B+)'),
    ('Зарядка:', 'ЗУ 12,6 В 1–2 А CC/CV в то же гнездо 5,5×2,1'),
    ('', 'блок 12 В 2 А для зарядки 3S не годится (нет CC/CV, мало напряжения)'),
    ('S1:', 'отключает только нагрузку — заряд идёт и в выключенной колонке'),
    ('АЦП:', 'Vакб = Vgpio1 × 5,55 (100k/22k); предупреждать при < 9,9 В,'),
    ('', 'выключаться при < 9,6 В; BMS отрежет сама на ~7,5–8,4 В'),
    ('Ток:', 'пик ≈ 2,5 А от АКБ (усилитель + 5 В ветка), средний ≈ 0,3–0,5 А'),
]
for i, (k, v) in enumerate(PN):
    y = 1616 + i * 23
    T(1620, y, k, 12.5, 'start', 'bold')
    T(1712, y, v, 12.5)


# =========================================================== РЕНДЕР ПРОВОДОВ
def segs(wr):
    p = wr['pts']
    return [(p[i], p[i + 1]) for i in range(len(p) - 1)]


def is_h(s):
    return s[0][1] == s[1][1]


def is_v(s):
    return s[0][0] == s[1][0]


problems = []
allsegs = []
for wi, wr in enumerate(wires):
    for s in segs(wr):
        if not (is_h(s) or is_v(s)):
            problems.append(f'diagonal {wr["net"]} {s}')
        allsegs.append((wi, wr['net'], s))

HOP_R = 6
hops = {}   # (wi, seg_index) -> list of y
for wi, wr in enumerate(wires):
    for si, s in enumerate(segs(wr)):
        if not is_v(s) or s[0][1] == s[1][1]:
            continue
        x = s[0][0]
        ya, yb = sorted((s[0][1], s[1][1]))
        for wj, net2, s2 in allsegs:
            if wj == wi or not is_h(s2) or s2[0][0] == s2[1][0]:
                continue
            y = s2[0][1]
            xa, xb = sorted((s2[0][0], s2[1][0]))
            if xa < x < xb and ya < y < yb:
                if net2 == wr['net']:
                    problems.append(f'same-net crossing {net2} at {(x, y)}')
                else:
                    hops.setdefault((wi, si), []).append(y)

# проверки: коллинеарные наложения и Т-стыки разных цепей
for i, (wi, n1, s1) in enumerate(allsegs):
    for wj, n2, s2 in allsegs[i + 1:]:
        if n1 == n2:
            continue
        if is_h(s1) and is_h(s2) and s1[0][1] == s2[0][1]:
            a0, a1 = sorted((s1[0][0], s1[1][0]))
            b0, b1 = sorted((s2[0][0], s2[1][0]))
            if max(a0, b0) < min(a1, b1):
                problems.append(f'H-overlap {n1}/{n2} y={s1[0][1]}')
        if is_v(s1) and is_v(s2) and s1[0][0] == s2[0][0]:
            a0, a1 = sorted((s1[0][1], s1[1][1]))
            b0, b1 = sorted((s2[0][1], s2[1][1]))
            if max(a0, b0) < min(a1, b1):
                problems.append(f'V-overlap {n1}/{n2} x={s1[0][0]}')


def on_seg(p, s, strict=True):
    (x1, y1), (x2, y2) = s
    if is_h(s) and p[1] == y1:
        lo, hi = sorted((x1, x2))
        return lo < p[0] < hi if strict else lo <= p[0] <= hi
    if is_v(s) and p[0] == x1:
        lo, hi = sorted((y1, y2))
        return lo < p[1] < hi if strict else lo <= p[1] <= hi
    return False


for wi, n1, s1 in allsegs:
    for p in s1:
        for wj, n2, s2 in allsegs:
            if n1 != n2 and on_seg(p, s2):
                problems.append(f'T-junction of different nets {n1}->{n2} at {p}')

# пересечение проводов с корпусами модулей
for wi, n1, s in allsegs:
    (x1, y1), (x2, y2) = s
    lo_x, hi_x = sorted((x1, x2))
    lo_y, hi_y = sorted((y1, y2))
    for bx0, by0, bx1, by1, name in boxes:
        if is_h(s):
            if by0 < y1 < by1 and max(lo_x, bx0) < min(hi_x, bx1):
                problems.append(f'{n1} H-seg through box {name} at y={y1}')
        else:
            if bx0 < x1 < bx1 and max(lo_y, by0) < min(hi_y, by1):
                problems.append(f'{n1} V-seg through box {name} at x={x1}')

# проверка близости дуг
for (wi, si), ys in hops.items():
    s = segs(wires[wi])[si]
    ends = (s[0][1], s[1][1])
    ys_s = sorted(ys)
    for a, b in zip(ys_s, ys_s[1:]):
        if b - a < 2 * HOP_R + 4:
            problems.append(f'hops too close {wires[wi]["net"]} {a},{b}')
    for y in ys:
        if min(abs(y - e) for e in ends) < HOP_R + 3:
            problems.append(f'hop near end {wires[wi]["net"]} y={y}')

# точки соединений
dots = []
for net in set(w['net'] for w in wires):
    ss = [s for _, n, s in allsegs if n == net]
    pts = set(p for s in ss for p in s)
    for p in pts:
        deg = sum(1 for s in ss if p in s) + 2 * sum(1 for s in ss if on_seg(p, s))
        if deg >= 3:
            dots.append((p, NETCOL[net]))


def path_d(wi, wr):
    p = wr['pts']
    d = [f'M {p[0][0]} {p[0][1]}']
    for si, s in enumerate(segs(wr)):
        (x1, y1), (x2, y2) = s
        if (wi, si) in hops:
            dr = 1 if y2 > y1 else -1
            for y in sorted(hops[(wi, si)], key=lambda v: v * dr):
                sweep = 1 if dr > 0 else 0
                d.append(f'L {x1} {y - dr * HOP_R} A {HOP_R} {HOP_R} 0 0 {sweep} {x1} {y + dr * HOP_R}')
        d.append(f'L {x2} {y2}')
    return ' '.join(d)


wire_svg = []
# сначала силовые, потом сигнальные
order = sorted(range(len(wires)), key=lambda i: 0 if wires[i]['net'] in POWER else 1)
for wi in order:
    wr = wires[wi]
    wire_svg.append(f'<path d="{path_d(wi, wr)}" fill="none" stroke="{NETCOL[wr["net"]]}" '
                    f'stroke-width="{wr["w"]}" stroke-linejoin="round" stroke-linecap="round"/>')
# дуги повторно поверх — с белой «подложкой», чтобы перемычка читалась
hop_svg = []
for (wi, si), ys in hops.items():
    wr = wires[wi]
    s = segs(wr)[si]
    x1, y1 = s[0]
    y2 = s[1][1]
    dr = 1 if y2 > y1 else -1
    sweep = 1 if dr > 0 else 0
    for y in ys:
        d = f'M {x1} {y - dr * HOP_R} A {HOP_R} {HOP_R} 0 0 {sweep} {x1} {y + dr * HOP_R}'
        hop_svg.append(f'<path d="{d}" fill="none" stroke="#ffffff" stroke-width="{wr["w"] + 3}"/>')
        hop_svg.append(f'<path d="{d}" fill="none" stroke="{NETCOL[wr["net"]]}" '
                       f'stroke-width="{wr["w"]}" stroke-linecap="round"/>')
dot_svg = [f'<circle cx="{p[0]}" cy="{p[1]}" r="4.6" fill="{c}" stroke="#ffffff" '
           f'stroke-width="1.2"/>' for p, c in dots]

svg = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
       f'viewBox="0 0 {W} {H}" font-family="{FONT}">',
       '<title>Умная колонка на ESP32-S3 — схема соединений</title>',
       f'<rect width="{W}" height="{H}" fill="#ffffff"/>']
svg += wire_svg + hop_svg + fg + dot_svg + top
svg.append(f'<text x="{W-20}" y="{H-12}" font-size="10" text-anchor="end" '
           f'fill="#adb5bd">v1 · схема соединений</text>')
svg.append('</svg>')

import os
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'schematic.svg')
with open(OUT, 'w', encoding='utf-8') as f:
    f.write('\n'.join(svg))

print('wires:', len(wires), 'hops:', sum(len(v) for v in hops.values()), 'dots:', len(dots))
print('problems:', len(problems))
for p in problems:
    print('  ', p)
