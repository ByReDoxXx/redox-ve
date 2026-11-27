// Konteyner terminali öykünücüsünün (assets/terminal.js) testleri: node --test crates/redox-server/tests/terminal.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { Screen, keySequence, fit } = require("../assets/terminal.js");

test("ekran kutuya sığar: iç boşluklar düşülür, sınırlar korunur", () => {
  const cell = { w: 7.8, h: 17 };
  const size = fit(1000, 590, cell);
  assert.deepEqual(size, { cols: 126, rows: 33 });
  assert.ok(size.rows * cell.h + 16 <= 590 && size.cols * cell.w + 16 <= 1000);
  assert.deepEqual(fit(100, 50, cell), { cols: 40, rows: 10 });
  assert.deepEqual(fit(9000, 9000, cell), { cols: 240, rows: 80 });
});

function screen(cols = 20, rows = 5) {
  const replies = [];
  const s = new Screen(cols, rows, (r) => replies.push(r));
  return { s, replies, all: () => Array.from({ length: rows }, (_, y) => s.text(y)) };
}

test("metin, satır sonu, geri silme, sekme ve satır kaydırma", () => {
  const { s, all } = screen(10, 4);
  s.write("merhaba\r\nab\bX\r\n1\t2\r\n0123456789ABC");
  assert.deepEqual(all(), ["aX", "1       2", "0123456789", "ABC"]);
  assert.equal(s.history.length, 1, "tepeden çıkan satır geçmişe gider");
  assert.equal(s.history[0].map((c) => c.ch).join("").trim(), "merhaba");
});

test("son sütuna yazmak satırı hemen atlatmaz", () => {
  const { s, all } = screen(5, 2);
  s.write("abcde");
  assert.deepEqual([s.x, s.y], [4, 0]);
  s.write("\r\nf");
  assert.deepEqual(all(), ["abcde", "f"]);
});

test("imleç hareketi, silme, karakter ekleme ve çıkarma", () => {
  const { s, all } = screen(10, 3);
  s.write("abcdefghij\x1b[2;1Hxyz\x1b[1;3H\x1b[K");
  assert.deepEqual(all(), ["ab", "xyz", ""]);
  s.write("\x1b[2;2H\x1b[@Q");
  assert.equal(s.text(1), "xQyz");
  s.write("\x1b[2;2H\x1b[2P");
  assert.equal(s.text(1), "xz");
  s.write("\x1b[2J");
  assert.deepEqual(all(), ["", "", ""]);
  s.write("\x1b[99;99H*");
  assert.equal(s.text(2), "         *", "sınır dışı konum ekrana sığdırılır");
});

test("kaydırma bölgesi, satır ekleme ve ters satır besleme", () => {
  const { s, all } = screen(5, 4);
  s.write("1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\n");
  assert.deepEqual(all(), ["1", "3", "", "4"]);
  assert.equal(s.history.length, 0, "bölge içi kaydırma geçmişe gitmez");
  s.write("\x1b[r\x1b[1;1H\x1bM");
  assert.deepEqual(all(), ["", "1", "3", ""]);
  s.write("\x1b[2;1H\x1b[L");
  assert.deepEqual(all(), ["", "", "1", "3"]);
  s.write("\x1b[M");
  assert.deepEqual(all(), ["", "1", "3", ""]);
});

test("seçenek ekranı (vi, top) kapanınca eski ekran döner", () => {
  const { s, all } = screen(10, 3);
  s.write("kabuk$ ");
  s.write("\x1b[?1049h\x1b[HVI EKRANI\r\n\n\n\n\n");
  assert.equal(s.history.length, 0, "seçenek ekranı geçmişi kirletmez");
  s.write("\x1b[?1049l");
  assert.deepEqual(all(), ["kabuk$", "", ""]);
  assert.deepEqual([s.x, s.y], [7, 0]);
});

test("renkler ve biçimler parçalara ayrılır", () => {
  const { s } = screen(20, 2);
  s.write("\x1b[31ma\x1b[1mb\x1b[0mc\x1b[7md\x1b[0;42me\x1b[38;5;3mf\x1b[38;2;9;9;9mg");
  const runs = s.runs(s.lines[0], -1);
  assert.deepEqual(runs.map((r) => [r.text, r.classes]), [["a", "t-fg1"], ["b", "t-fg9 t-b"], ["c", ""], ["d", "t-inv"], ["e", "t-bg2"], ["f", "t-fg3 t-bg2"], ["g", "t-bg2"]]);
  assert.deepEqual(s.runs(s.lines[1], 0).map((r) => r.classes), ["t-inv"], "boş satırda imleç görünür");
});

test("parça parça gelen diziler, yok sayılan iletiler ve sorgulara yanıt", () => {
  const { s, replies, all } = screen(10, 2);
  for (const part of ["\x1b", "[", "3", "1m", "x\x1b]0;baslik", "\x07y\x1b(Bz\x1b]2;t\x1b\\w"]) s.write(part);
  assert.deepEqual(all(), ["xyzw", ""]);
  s.write("\x1b[6n\x1b[c\x1b[>c\x1b[5n");
  assert.deepEqual(replies, ["\x1b[1;5R", "\x1b[?1;2c", "\x1b[>0;0;0c", "\x1b[0n"]);
});

test("konuğun gönderdiği HTML yalnızca metindir ve aşırı parametreler zararsızdır", () => {
  const { s, all } = screen(40, 2);
  s.write("<img src=x onerror=alert(1)>");
  assert.equal(all()[0], "<img src=x onerror=alert(1)>");
  s.write("\x1b[999999999999;5H\x1b[99999999L\x1b[99999999@\x1b[99999999S" + "\x1b[" + "1;".repeat(5000) + "m");
  assert.equal(s.lines.length, 2);
  assert.ok(s.lines.every((line) => line.length === 40));
});

test("özel tuşlar ve Ctrl birleşimleri; AltGr yazıya bırakılır", () => {
  const key = (k, extra = {}) => keySequence({ key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...extra }, false);
  assert.equal(key("Enter"), "\r");
  assert.equal(key("Backspace"), "\x7f");
  assert.equal(key("ArrowUp"), "\x1b[A");
  assert.equal(keySequence({ key: "ArrowUp" }, true), "\x1bOA");
  assert.equal(key("Tab", { shiftKey: true }), "\x1b[Z");
  assert.equal(key("c", { ctrlKey: true }), "\x03");
  assert.equal(key("d", { ctrlKey: true }), "\x04");
  assert.equal(key("C", { ctrlKey: true, shiftKey: true }), null, "tarayıcının kopyalaması");
  assert.equal(key("b", { altKey: true }), "\x1bb");
  assert.equal(key("@", { ctrlKey: true, altKey: true }), null, "Türkçe klavyede AltGr+Q");
  assert.equal(key("a"), null, "yazdırılabilir karakter metin girişinden gelir");
  assert.equal(key("r", { metaKey: true }), null);
});
