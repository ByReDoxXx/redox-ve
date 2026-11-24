// Canlı konsol istemcisinin (assets/console.js) mantık testleri: node --test crates/redox-server/tests/console.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { XT, keysymOf, buttonMask, msg, toRgba, Reader, Session } = require("../assets/console.js");

const bytes = (...parts) => Uint8Array.from(parts.flatMap((p) => Array.from(p)));
const u16 = (v) => [v >> 8, v & 0xff];
const u32 = (v) => [(v >>> 24) & 0xff, (v >>> 16) & 0xff, (v >>> 8) & 0xff, v & 0xff];
const text = (s) => Array.from(Buffer.from(s, "latin1"));
const tick = () => new Promise((r) => setImmediate(r));

// Sunucunun açılış iletileri: sürüm, güvenlik türleri, sonuç, ekran boyutu ve ad.
function serverHello(w, h, name) {
  return bytes(text("RFB 003.008\n"), [1, 1], u32(0), u16(w), u16(h), new Array(16).fill(0), u32(name.length), text(name));
}

function harness() {
  const sent = [];
  const calls = [];
  const states = [];
  const display = {
    resize: (w, h) => calls.push(["resize", w, h]),
    draw: (x, y, w, h, rgba) => calls.push(["draw", x, y, w, h, Array.from(rgba)]),
    copy: (...a) => calls.push(["copy", ...a]),
  };
  const session = new Session((b) => sent.push(Array.from(b)), display, (s, d) => states.push([s, d]));
  const done = session.run();
  return { session, sent, calls, states, done };
}

test("okuyucu parçaları birleştirir ve sınırları korur", async () => {
  const r = new Reader();
  const pending = r.read(5);
  r.push(bytes([1, 2]));
  r.push(bytes([3, 4, 5, 6, 7]));
  assert.deepEqual(Array.from(await pending), [1, 2, 3, 4, 5]);
  assert.deepEqual(Array.from(await r.read(2)), [6, 7]);
  assert.equal(r.length, 0);
  assert.deepEqual(Array.from(await r.read(0)), []);
  const late = r.read(3);
  r.push(bytes([9]));
  r.close();
  await assert.rejects(late, /kapandı/);
  assert.deepEqual(Array.from(await r.read(1)), [9], "kapanmadan önce gelen bayt okunabilir");
  await assert.rejects(r.read(1), /kapandı/);
});

test("pikseller BGRX'ten RGBA'ya çevrilir", () => {
  assert.deepEqual(Array.from(toRgba(bytes([10, 20, 30, 0, 1, 2, 3, 99]))), [30, 20, 10, 255, 3, 2, 1, 255]);
  assert.deepEqual(Array.from(toRgba(bytes([1, 2, 3]))), [1, 2, 3], "yarım piksel olduğu gibi kalır");
});

test("tuş simgeleri ve tarama kodları", () => {
  assert.equal(keysymOf("a", "KeyA"), 0x61);
  assert.equal(keysymOf("A", "KeyA"), 0x41);
  assert.equal(keysymOf(" ", "Space"), 0x20);
  assert.equal(keysymOf("ç", "Period"), 0xe7, "Latin-1 doğrudan");
  assert.equal(keysymOf("ş", "Semicolon"), 0x01000000 + 0x15f, "Latin-1 dışı Unicode simgesi");
  assert.equal(keysymOf("Enter", "Enter"), 0xff0d);
  assert.equal(keysymOf("Shift", "ShiftLeft"), 0xffe1);
  assert.equal(keysymOf("Shift", "ShiftRight"), 0xffe2);
  assert.equal(keysymOf("Control", "ControlRight"), 0xffe4);
  assert.equal(keysymOf("Dead", "Quote"), 0, "ölü tuşun simgesi yok; konumla gönderilir");
  assert.equal(keysymOf("\u0007", ""), 0);
  assert.equal(keysymOf("toString", ""), 0, "nesne üyeleri tuş adı sayılmaz");
  assert.equal(XT.KeyQ, 0x10);
  assert.equal(XT.Enter, 0x1c);
  assert.equal(XT.NumpadEnter, 0x9c, "genişletilmiş tuş 0x80 ile işaretlenir");
  assert.equal(XT.ArrowUp, 0xc8);
  assert.equal(XT.Delete, 0xd3);
  assert.equal(new Set(Object.values(XT)).size, Object.values(XT).length, "tarama kodları yinelenmez");
  assert.deepEqual([buttonMask(0), buttonMask(1), buttonMask(2), buttonMask(4), buttonMask(7)], [0, 1, 4, 2, 7]);
});

test("ileti gövdeleri", () => {
  assert.deepEqual(Array.from(msg.key(true, 0xff0d)), [4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
  assert.deepEqual(Array.from(msg.extendedKey(false, 0x61, 0x1e)), [255, 0, 0, 0, 0, 0, 0, 0x61, 0, 0, 0, 0x1e]);
  assert.deepEqual(Array.from(msg.pointer(5, 300, 2)), [5, 5, 1, 44, 0, 2]);
  assert.deepEqual(Array.from(msg.updateRequest(true, 1024, 768)), [3, 1, 0, 0, 0, 0, 4, 0, 3, 0]);
  assert.deepEqual(Array.from(msg.pixelFormat()), [0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
  assert.deepEqual(Array.from(msg.encodings()), [2, 0, 0, 4, ...u32(1), ...u32(0), ...u32(-223 >>> 0), ...u32(-258 >>> 0)]);
});

test("el sıkışma, çizim ve giriş", async () => {
  const h = harness();
  // Karşılama iki parçada gelir: protokol sınırı ileti sınırına denk gelmez.
  const hello = serverHello(4, 2, "QEMU (deneme)");
  h.session.feed(hello.subarray(0, 7));
  h.session.feed(hello.subarray(7));
  await tick();
  assert.deepEqual(h.states.map((s) => s[0]), ["bağlanıyor", "bağlı"]);
  assert.equal(h.states[1][1], "QEMU (deneme)");
  assert.deepEqual(h.calls, [["resize", 4, 2]]);
  assert.deepEqual(h.sent[0], text("RFB 003.008\n"));
  assert.deepEqual(h.sent[1], [1], "güvenlik türü: yok");
  assert.deepEqual(h.sent[2], [1], "paylaşımlı oturum");
  assert.deepEqual(h.sent[3], Array.from(msg.pixelFormat()));
  assert.deepEqual(h.sent[4], Array.from(msg.encodings()));
  assert.deepEqual(h.sent[5], Array.from(msg.updateRequest(false, 4, 2)), "ilk istek tam ekran");

  // Bir güncelleme: 2x1 ham dikdörtgen, bir kopyalama ve tarama kodu desteği bildirimi.
  h.session.feed(bytes([0, 0], u16(3), u16(1), u16(0), u16(2), u16(1), u32(0), [10, 20, 30, 0, 40, 50, 60, 0], u16(0), u16(1), u16(2), u16(1), u32(1), u16(1), u16(0), u16(0), u16(0), u16(0), u16(0), u32(-258 >>> 0)));
  await tick();
  assert.deepEqual(h.calls[1], ["draw", 1, 0, 2, 1, [30, 20, 10, 255, 60, 50, 40, 255]]);
  assert.deepEqual(h.calls[2], ["copy", 1, 0, 2, 1, 0, 1]);
  assert.deepEqual(h.sent[6], Array.from(msg.updateRequest(true, 4, 2)), "her güncellemeden sonra artımlı istek");
  assert.equal(h.session.extendedKeys, true);

  // Konum bilinen tuş tarama koduyla, bilinmeyen simgeyle gider; ikisi de yoksa gönderilmez.
  assert.equal(h.session.key(true, "q", "KeyQ"), true);
  assert.deepEqual(h.sent[7], Array.from(msg.extendedKey(true, 0x71, 0x10)));
  assert.equal(h.session.key(true, "Pause", "Pause"), true);
  assert.deepEqual(h.sent[8], Array.from(msg.key(true, 0xff13)));
  assert.equal(h.session.key(true, "Unidentified", ""), false);
  assert.equal(h.sent.length, 9);
  h.session.pointer(1, 99.6, -3);
  assert.deepEqual(h.sent[9], Array.from(msg.pointer(1, 3, 0)), "imleç ekranın içine sıkıştırılır");
  h.session.ctrlAltDel();
  assert.deepEqual(h.sent.slice(10).map((m) => [m[3], m[11]]), [[1, 0x1d], [1, 0x38], [1, 0xd3], [0, 0xd3], [0, 0x38], [0, 0x1d]], "basış sırasıyla, bırakış tersinden");

  // Ekran boyutu değişimi, zil, pano ve renk tablosu iletileri akışı bozmaz.
  h.session.feed(bytes([2], [3, 0, 0, 0], u32(3), text("abc"), [1, 0], u16(0), u16(1), [0, 0, 0, 0, 0, 0], [0, 0], u16(1), u16(0), u16(0), u16(8), u16(6), u32(-223 >>> 0)));
  await tick();
  assert.deepEqual(h.calls.at(-1), ["resize", 8, 6]);
  assert.deepEqual(h.sent.at(-1), Array.from(msg.updateRequest(true, 8, 6)));

  h.session.end();
  await h.done;
  assert.equal(h.states.at(-1)[0], "kapandı");
  assert.equal(h.session.key(true, "a", "KeyA"), false, "kapalı oturum tuş göndermez");
});

test("tarama kodu desteklenmiyorsa simge gönderilir", async () => {
  const h = harness();
  h.session.feed(serverHello(2, 2, ""));
  await tick();
  assert.equal(h.session.key(true, "q", "KeyQ"), true);
  assert.deepEqual(h.sent.at(-1), Array.from(msg.key(true, 0x71)));
  assert.equal(h.session.key(true, "Dead", "Quote"), false, "ne simge ne destek: gönderilmez");
});

test("hatalı sunucu oturumu kapatır, sayfayı bozmaz", async () => {
  for (const [name, stream, expected] of [
    ["karşılama", bytes(text("HTTP/1.1 200\n")), /karşılama/],
    ["reddedilen bağlantı", bytes(text("RFB 003.008\n"), [0], u32(5), text("dolu!")), /dolu!/],
    ["parola isteyen sunucu", bytes(text("RFB 003.008\n"), [1, 2]), /parolasız/],
    ["başarısız güvenlik", bytes(text("RFB 003.008\n"), [1, 1], u32(1), u32(4), text("yok.")), /yok\./],
    ["dev ekran", bytes(text("RFB 003.008\n"), [1, 1], u32(0), u16(9000), u16(10), new Array(16).fill(0), u32(0)), /ekran boyutu/],
    ["ekran dışı çizim", bytes(serverHello(2, 2, ""), [0, 0], u16(1), u16(1), u16(1), u16(2), u16(2), u32(0)), /dışına/],
    ["bilinmeyen kodlama", bytes(serverHello(2, 2, ""), [0, 0], u16(1), u16(0), u16(0), u16(1), u16(1), u32(7)), /kodlama 7/],
    ["bilinmeyen ileti", bytes(serverHello(2, 2, ""), [9]), /ileti 9/],
  ]) {
    const h = harness();
    h.session.feed(stream);
    await h.done;
    const last = h.states.at(-1);
    assert.equal(last[0], "kapandı", name);
    assert.match(last[1], expected, name);
    assert.equal(h.calls.filter((c) => c[0] === "draw").length, 0, name);
  }
});
