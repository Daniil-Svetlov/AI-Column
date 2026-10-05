import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SentenceSplitter } from '../src/text/sentences.js';
import { cleanForSpeech, ThinkFilter, normalize } from '../src/text/clean.js';

function feed(splitter, text, step = 3) {
  const out = [];
  for (let i = 0; i < text.length; i += step) out.push(...splitter.push(text.slice(i, i + step)));
  out.push(...splitter.flush());
  return out;
}

test('первое предложение уходит сразу, короткие следующие склеиваются', () => {
  const s = new SentenceSplitter({ minChars: 30 });
  const out = feed(s, 'Да. Конечно, сейчас расскажу. Ок. Это займёт пару минут, не больше.');
  assert.equal(out[0], 'Да.');
  assert.ok(out.length >= 2);
  assert.equal(out.join(' '), 'Да. Конечно, сейчас расскажу. Ок. Это займёт пару минут, не больше.');
});

test('числа и сокращения не режут предложение', () => {
  const s = new SentenceSplitter({ minChars: 1 });
  const out = feed(s, 'Число пи равно 3.14 примерно. Это т. е. константа и т. д. всем известна. Конец!');
  assert.deepEqual(out, ['Число пи равно 3.14 примерно.', 'Это т. е. константа и т. д. всем известна.', 'Конец!']);
});

test('вопросы, восклицания, многоточие и кавычки', () => {
  const s = new SentenceSplitter({ minChars: 1 });
  const out = feed(s, 'Ты спросил «почему?» Отвечаю… Потому что! А ещё?');
  assert.deepEqual(out, ['Ты спросил «почему?»', 'Отвечаю…', 'Потому что!', 'А ещё?']);
});

test('слишком длинный кусок без точек режется по запятой', () => {
  const s = new SentenceSplitter({ minChars: 1, maxChars: 60 });
  const long = 'раз два три четыре пять, шесть семь восемь девять десять, одиннадцать двенадцать тринадцать четырнадцать пятнадцать';
  const out = feed(s, long);
  assert.ok(out.length >= 2);
  for (const p of out) assert.ok(p.length <= 60, p);
  assert.equal(out.join(' '), long);
});

test('перевод строки — конец предложения', () => {
  const s = new SentenceSplitter({ minChars: 1 });
  assert.deepEqual(feed(s, 'Первое\nВторое'), ['Первое', 'Второе']);
});

test('cleanForSpeech убирает markdown, ссылки, эмодзи, код', () => {
  const t = cleanForSpeech('## Итог\n- **Жирный** и *курсив* 😀 см. [доку](https://x.y) или https://a.b/c\n```js\ncode()\n```\n`npm i`');
  assert.equal(t, 'Итог Жирный и курсив см. доку или ссылка npm i');
});

test('cleanForSpeech: символы, которых нет в шрифте колонки и которые TTS читает плохо', () => {
  assert.equal(cleanForSpeech('Стоит 500 ₽, „дёшево“'), 'Стоит 500 рублей, "дёшево"');
});

test('ThinkFilter вырезает <think> даже разрезанный между чанками', () => {
  const f = new ThinkFilter();
  const parts = ['<thi', 'nk>Надо подумать', ' ещё</th', 'ink>\n\nОтвет: ', '42<', 'b>'];
  const out = parts.map((p) => f.push(p)).join('') + f.flush();
  assert.equal(out, 'Ответ: 42<b>');
});

test('ThinkFilter пропускает текст без think-блоков', () => {
  const f = new ThinkFilter();
  assert.equal(f.push('Привет, мир') + f.flush(), 'Привет, мир');
});

test('normalize не меняет длину строки', () => {
  const s = 'Ёжик ЁЛКА';
  assert.equal(normalize(s), 'ежик елка');
  assert.equal(normalize(s).length, s.length);
});
