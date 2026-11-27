// RedoX VE canlı konsolu: konuğun ekranını ve girişini VNC (RFB 3.8) ile taşıyan küçük istemci.
// Uygulamadaki tek betiktir ve yalnızca konsol sayfasında yüklenir. Sunucu, tarayıcının WebSocket
// bağlantısını konuğun yerel VNC soketine köprüler; kimlik ve yetki denetimi sunucudadır.
(function (root) {
  "use strict";

  // ---------------------------------------------------------------- tuşlar
  // Tuşun fiziksel konumu (event.code) → XT tarama kodu. Genişletilmiş (e0 önekli) tuşlar 0x80 ile işaretlenir.
  // Konum gönderildiği için karakteri konuğun kendi klavye düzeni belirler (Türkçe Q, F ve ötekiler doğru çalışır).
  const XT = {
    Escape: 0x01, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05, Digit5: 0x06, Digit6: 0x07, Digit7: 0x08,
    Digit8: 0x09, Digit9: 0x0a, Digit0: 0x0b, Minus: 0x0c, Equal: 0x0d, Backspace: 0x0e, Tab: 0x0f,
    KeyQ: 0x10, KeyW: 0x11, KeyE: 0x12, KeyR: 0x13, KeyT: 0x14, KeyY: 0x15, KeyU: 0x16, KeyI: 0x17, KeyO: 0x18,
    KeyP: 0x19, BracketLeft: 0x1a, BracketRight: 0x1b, Enter: 0x1c, ControlLeft: 0x1d,
    KeyA: 0x1e, KeyS: 0x1f, KeyD: 0x20, KeyF: 0x21, KeyG: 0x22, KeyH: 0x23, KeyJ: 0x24, KeyK: 0x25, KeyL: 0x26,
    Semicolon: 0x27, Quote: 0x28, Backquote: 0x29, ShiftLeft: 0x2a, Backslash: 0x2b,
    KeyZ: 0x2c, KeyX: 0x2d, KeyC: 0x2e, KeyV: 0x2f, KeyB: 0x30, KeyN: 0x31, KeyM: 0x32,
    Comma: 0x33, Period: 0x34, Slash: 0x35, ShiftRight: 0x36, NumpadMultiply: 0x37, AltLeft: 0x38, Space: 0x39,
    CapsLock: 0x3a, F1: 0x3b, F2: 0x3c, F3: 0x3d, F4: 0x3e, F5: 0x3f, F6: 0x40, F7: 0x41, F8: 0x42, F9: 0x43,
    F10: 0x44, NumLock: 0x45, ScrollLock: 0x46, Numpad7: 0x47, Numpad8: 0x48, Numpad9: 0x49, NumpadSubtract: 0x4a,
    Numpad4: 0x4b, Numpad5: 0x4c, Numpad6: 0x4d, NumpadAdd: 0x4e, Numpad1: 0x4f, Numpad2: 0x50, Numpad3: 0x51,
    Numpad0: 0x52, NumpadDecimal: 0x53, IntlBackslash: 0x56, F11: 0x57, F12: 0x58,
    NumpadEnter: 0x9c, ControlRight: 0x9d, NumpadDivide: 0xb5, PrintScreen: 0xb7, AltRight: 0xb8, Home: 0xc7,
    ArrowUp: 0xc8, PageUp: 0xc9, ArrowLeft: 0xcb, ArrowRight: 0xcd, End: 0xcf, ArrowDown: 0xd0, PageDown: 0xd1,
    Insert: 0xd2, Delete: 0xd3, MetaLeft: 0xdb, MetaRight: 0xdc, ContextMenu: 0xdd,
  };

  // Adlandırılmış tuşlar (event.key) → X11 tuş simgesi.
  const KEYSYM = {
    Backspace: 0xff08, Tab: 0xff09, Enter: 0xff0d, Escape: 0xff1b, Delete: 0xffff, Insert: 0xff63, Home: 0xff50,
    End: 0xff57, PageUp: 0xff55, PageDown: 0xff56, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53,
    ArrowDown: 0xff54, F1: 0xffbe, F2: 0xffbf, F3: 0xffc0, F4: 0xffc1, F5: 0xffc2, F6: 0xffc3, F7: 0xffc4,
    F8: 0xffc5, F9: 0xffc6, F10: 0xffc7, F11: 0xffc8, F12: 0xffc9, Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9,
    AltGraph: 0xfe03, Meta: 0xffeb, CapsLock: 0xffe5, NumLock: 0xff7f, ScrollLock: 0xff14, Pause: 0xff13,
    PrintScreen: 0xff61, ContextMenu: 0xff67,
  };

  // Metin yapıştırma: karakter → [event.key, event.code, Shift gerekli mi]. ABD klavye düzenine göre; konuk başka
  // bir düzen kullanıyorsa simgeler farklı çıkabilir. Bu düzende olmayan karakter için null.
  const PAIRS = { Minus: "-_", Equal: "=+", BracketLeft: "[{", BracketRight: "]}", Backslash: "\\|", Semicolon: ";:", Quote: "'\"", Backquote: "`~", Comma: ",<", Period: ".>", Slash: "/?" };
  const TYPED = new Map([[" ", [" ", "Space", false]], ["\n", ["Enter", "Enter", false]], ["\t", ["Tab", "Tab", false]]]);
  for (let i = 0; i < 26; i++) {
    const lower = String.fromCharCode(97 + i);
    TYPED.set(lower, [lower, "Key" + lower.toUpperCase(), false]);
    TYPED.set(lower.toUpperCase(), [lower.toUpperCase(), "Key" + lower.toUpperCase(), true]);
  }
  Array.from(")!@#$%^&*(").forEach((symbol, digit) => {
    TYPED.set(String(digit), [String(digit), "Digit" + digit, false]);
    TYPED.set(symbol, [symbol, "Digit" + digit, true]);
  });
  Object.entries(PAIRS).forEach(([code, pair]) => {
    TYPED.set(pair[0], [pair[0], code, false]);
    TYPED.set(pair[1], [pair[1], code, true]);
  });
  function typedKey(ch) {
    return TYPED.get(ch) || null;
  }

  // Tuş olayının X11 simgesi; bilinmiyorsa 0.
  function keysymOf(key, code) {
    if (Object.prototype.hasOwnProperty.call(KEYSYM, key)) {
      const sym = KEYSYM[key];
      // Sağdaki Shift, Control, Alt ve Meta bir sonraki simgedir.
      return /^(Shift|Control|Alt|Meta)Right$/.test(code || "") ? sym + 1 : sym;
    }
    const points = Array.from(key || "");
    if (points.length !== 1) return 0;
    const cp = points[0].codePointAt(0);
    if (cp < 0x20) return 0;
    return cp < 0x100 ? cp : 0x01000000 + cp;
  }

  // Fare düğmeleri: tarayıcının (sol 1, sağ 2, orta 4) maskesi → RFB (sol 1, orta 2, sağ 4).
  function buttonMask(buttons) {
    return (buttons & 1) | ((buttons & 4) >> 1) | ((buttons & 2) << 1);
  }

  // ---------------------------------------------------------------- iletiler
  function u16(v) { return [(v >> 8) & 0xff, v & 0xff]; }
  function u32(v) { return [(v >>> 24) & 0xff, (v >>> 16) & 0xff, (v >>> 8) & 0xff, v & 0xff]; }

  const msg = {
    // 32 bit, küçük uçlu, gerçek renk: bellekte B, G, R, boş.
    pixelFormat() {
      return Uint8Array.from([0, 0, 0, 0, 32, 24, 0, 1, ...u16(255), ...u16(255), ...u16(255), 16, 8, 0, 0, 0, 0]);
    },
    // CopyRect, Raw, masaüstü boyutu, QEMU'nun tarama kodlu tuş olayı ve (ses kartı varsa) QEMU'nun ses uzantısı.
    encodings(sound) {
      const list = sound ? [1, 0, -223, -258, -259] : [1, 0, -223, -258];
      return Uint8Array.from([2, 0, ...u16(list.length), ...list.flatMap((e) => u32(e))]);
    },
    updateRequest(incremental, w, h) {
      return Uint8Array.from([3, incremental ? 1 : 0, 0, 0, 0, 0, ...u16(w), ...u16(h)]);
    },
    key(down, keysym) {
      return Uint8Array.from([4, down ? 1 : 0, 0, 0, ...u32(keysym)]);
    },
    extendedKey(down, keysym, scancode) {
      return Uint8Array.from([255, 0, ...u16(down ? 1 : 0), ...u32(keysym), ...u32(scancode)]);
    },
    pointer(mask, x, y) {
      return Uint8Array.from([5, mask & 0xff, ...u16(x), ...u16(y)]);
    },
    // QEMU ses uzantısı: biçim (16 bit işaretli, iki kanal) ve akışı açma / kapama.
    audioFormat() {
      return Uint8Array.from([255, 1, ...u16(2), 3, CHANNELS, ...u32(RATE)]);
    },
    audio(on) {
      return Uint8Array.from([255, 1, ...u16(on ? 0 : 1)]);
    },
  };

  const RATE = 44100;
  const CHANNELS = 2;
  const MAX_AUDIO = 1 << 20;

  // İç içe geçmiş 16 bit küçük uçlu örnekler → kanal başına [-1, 1) aralığında diziler. Yarım kare atılır.
  function toChannels(bytes) {
    const frames = Math.floor(bytes.length / (2 * CHANNELS));
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return Array.from({ length: CHANNELS }, (_, channel) => Float32Array.from({ length: frames }, (_, i) => view.getInt16((i * CHANNELS + channel) * 2, true) / 32768));
  }

  // Ham piksel (B, G, R, boş) → tuval pikseli (R, G, B, 255), yerinde.
  function toRgba(pixels) {
    for (let i = 0; i + 3 < pixels.length; i += 4) {
      const b = pixels[i];
      pixels[i] = pixels[i + 2];
      pixels[i + 2] = b;
      pixels[i + 3] = 255;
    }
    return pixels;
  }

  // ---------------------------------------------------------------- bayt kuyruğu
  // WebSocket iletileri protokol sınırlarına denk gelmez: gelen parçalar burada birikir, istenen kadar bayt okunur.
  class Reader {
    constructor() { this.chunks = []; this.length = 0; this.waiting = null; this.closed = false; }
    push(chunk) {
      if (!chunk.length) return;
      this.chunks.push(chunk);
      this.length += chunk.length;
      this.wake();
    }
    close() { this.closed = true; this.wake(); }
    wake() {
      const w = this.waiting;
      if (w && (this.length >= w.n || this.closed)) {
        this.waiting = null;
        if (this.length >= w.n) w.resolve(this.take(w.n)); else w.reject(new Error("bağlantı kapandı"));
      }
    }
    take(n) {
      const out = new Uint8Array(n);
      let at = 0;
      while (at < n) {
        const head = this.chunks[0];
        const part = Math.min(head.length, n - at);
        out.set(head.subarray(0, part), at);
        at += part;
        if (part === head.length) this.chunks.shift(); else this.chunks[0] = head.subarray(part);
      }
      this.length -= n;
      return out;
    }
    read(n) {
      if (n === 0) return Promise.resolve(new Uint8Array(0));
      if (this.length >= n) return Promise.resolve(this.take(n));
      if (this.closed) return Promise.reject(new Error("bağlantı kapandı"));
      return new Promise((resolve, reject) => { this.waiting = { n, resolve, reject }; });
    }
  }

  const be16 = (b, i) => (b[i] << 8) | b[i + 1];
  const be32 = (b, i) => ((b[i] << 24) | (b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]) >>> 0;
  const MAX_SIDE = 8192;

  // ---------------------------------------------------------------- oturum
  // `send(Uint8Array)`: sunucuya yazar. `display`: { resize(w, h), draw(x, y, w, h, rgba), copy(sx, sy, w, h, dx, dy) }.
  // `onState(durum, ayrıntı)`: "bağlanıyor", "bağlı", "kapandı".
  class Session {
    // `sound`: konukta ses kartı var; { ready(), play(bytes) } verilir.
    constructor(send, display, onState, sound) {
      this.send = send; this.display = display; this.onState = onState || (() => {});
      this.sound = sound || null; this.hasAudio = false;
      this.reader = new Reader();
      this.width = 0; this.height = 0; this.ready = false; this.extendedKeys = false; this.name = "";
    }
    feed(chunk) { this.reader.push(chunk); }
    end() { this.reader.close(); }

    async run() {
      const r = this.reader;
      try {
        this.onState("bağlanıyor");
        const hello = new TextDecoder().decode(await r.read(12));
        if (!/^RFB \d{3}\.\d{3}\n$/.test(hello)) throw new Error("beklenmeyen karşılama");
        this.send(new TextEncoder().encode("RFB 003.008\n"));
        const count = (await r.read(1))[0];
        if (count === 0) throw new Error(await this.reason());
        const types = await r.read(count);
        if (!types.includes(1)) throw new Error("sunucu parolasız bağlantıyı kabul etmiyor");
        this.send(Uint8Array.from([1]));
        if (be32(await r.read(4), 0) !== 0) throw new Error(await this.reason());
        this.send(Uint8Array.from([1])); // paylaşımlı: aynı ekrana başka oturumlar da bağlanabilir
        const init = await r.read(24);
        this.resize(be16(init, 0), be16(init, 2));
        this.name = new TextDecoder().decode(await r.read(be32(init, 20)));
        this.send(msg.pixelFormat());
        this.send(msg.encodings(this.sound !== null));
        this.send(msg.updateRequest(false, this.width, this.height));
        this.ready = true;
        this.onState("bağlı", this.name);
        for (;;) await this.message();
      } catch (e) {
        this.ready = false;
        this.onState("kapandı", e && e.message ? e.message : String(e));
      }
    }

    async reason() {
      const r = this.reader;
      return new TextDecoder().decode(await r.read(be32(await r.read(4), 0)));
    }

    resize(w, h) {
      if (w < 1 || h < 1 || w > MAX_SIDE || h > MAX_SIDE) throw new Error("geçersiz ekran boyutu");
      this.width = w; this.height = h;
      this.display.resize(w, h);
    }

    async message() {
      const r = this.reader;
      const type = (await r.read(1))[0];
      if (type === 0) {
        const count = be16(await r.read(3), 1);
        for (let i = 0; i < count; i++) {
          const head = await r.read(12);
          const [x, y, w, h] = [be16(head, 0), be16(head, 2), be16(head, 4), be16(head, 6)];
          const encoding = be32(head, 8) | 0;
          if (encoding === -223) { this.resize(w, h); continue; }
          if (encoding === -258) { this.extendedKeys = true; continue; }
          if (encoding === -259) { if (this.sound) { this.hasAudio = true; this.sound.ready(); } continue; }
          if (x + w > this.width || y + h > this.height) throw new Error("ekranın dışına çizim");
          if (encoding === 0) {
            if (w && h) this.display.draw(x, y, w, h, toRgba(await r.read(w * h * 4)));
          } else if (encoding === 1) {
            const src = await r.read(4);
            this.display.copy(be16(src, 0), be16(src, 2), w, h, x, y);
          } else {
            throw new Error("desteklenmeyen kodlama " + encoding);
          }
        }
        this.send(msg.updateRequest(true, this.width, this.height));
      } else if (type === 1) {
        await r.read(be16(await r.read(5), 3) * 6); // renk tablosu: gerçek renkte kullanılmaz
      } else if (type === 2) {
        // zil
      } else if (type === 3) {
        await r.read(be32(await r.read(7), 3)); // konuğun panosu: kullanılmıyor
      } else if (type === 255 && this.hasAudio) {
        const head = await r.read(3);
        const operation = be16(head, 1);
        if (head[0] !== 1 || operation > 2) throw new Error("bilinmeyen ses iletisi");
        if (operation === 2) {
          const length = be32(await r.read(4), 0);
          if (length > MAX_AUDIO) throw new Error("ses parçası çok büyük");
          this.sound.play(await r.read(length));
        }
      } else {
        throw new Error("bilinmeyen ileti " + type);
      }
    }

    // Sunucudan ses akışını ister ya da durdurur.
    listen(on) {
      if (!this.ready || !this.hasAudio) return false;
      if (on) this.send(msg.audioFormat());
      this.send(msg.audio(on));
      return true;
    }

    // Tuş olayı. Konum biliniyorsa ve sunucu destekliyorsa tarama kodu, değilse karakter gönderilir.
    key(down, key, code) {
      if (!this.ready) return false;
      const keysym = keysymOf(key, code);
      const scancode = Object.prototype.hasOwnProperty.call(XT, code) ? XT[code] : 0;
      if (this.extendedKeys && scancode) this.send(msg.extendedKey(down, keysym, scancode));
      else if (keysym) this.send(msg.key(down, keysym));
      else return false;
      return true;
    }

    pointer(buttons, x, y) {
      if (!this.ready) return;
      const clamp = (v, max) => Math.max(0, Math.min(max - 1, Math.round(v)));
      this.send(msg.pointer(buttons, clamp(x, this.width), clamp(y, this.height)));
    }

    // Tarayıcının kendine ayırdığı bileşimi konuğa gönderir.
    // Metni tuş vuruşları olarak gönderir (pano gerektirmez; konukta ajan olması gerekmez). Yazılamayan
    // karakterlerin sayısını döndürür. `wait`: iki tuş arasında beklenen söz (konuğun arabelleği taşmasın).
    async type(text, wait) {
      let skipped = 0;
      for (const ch of Array.from(String(text).replace(/\r\n?/g, "\n"))) {
        const hit = typedKey(ch);
        if (!hit) { skipped++; continue; }
        const [key, code, shift] = hit;
        if (shift) this.key(true, "Shift", "ShiftLeft");
        this.key(true, key, code);
        this.key(false, key, code);
        if (shift) this.key(false, "Shift", "ShiftLeft");
        if (wait) await wait();
      }
      return skipped;
    }

    ctrlAltDel() {
      const keys = [["Control", "ControlLeft"], ["Alt", "AltLeft"], ["Delete", "Delete"]];
      keys.forEach(([k, c]) => this.key(true, k, c));
      keys.reverse().forEach(([k, c]) => this.key(false, k, c));
    }
  }

  const api = { XT, keysymOf, typedKey, buttonMask, msg, toRgba, toChannels, Reader, Session };
  if (typeof module !== "undefined" && module.exports) { module.exports = api; return; }
  root.RedoxConsole = api;

  // ---------------------------------------------------------------- sayfa
  const canvas = document.getElementById("screen");
  if (!canvas) return;
  const status = document.getElementById("console-status");
  const ctx = canvas.getContext("2d", { alpha: false });
  const say = (text, bad, tone) => { if (status) { status.textContent = text; status.className = "pill " + (tone || (bad ? "err" : "ok")); } };
  const display = {
    resize(w, h) { canvas.width = w; canvas.height = h; },
    draw(x, y, w, h, rgba) { ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba.buffer, rgba.byteOffset, rgba.length), w, h), x, y); },
    copy(sx, sy, w, h, dx, dy) { ctx.drawImage(canvas, sx, sy, w, h, dx, dy, w, h); },
  };
  const scheme = location.protocol === "https:" ? "wss://" : "ws://";
  const socket = new WebSocket(scheme + location.host + canvas.dataset.ws);
  socket.binaryType = "arraybuffer";
  // Tarayıcı sesi ancak bir tıklamadan sonra çalar: ses düğmeyle açılır.
  const soundButton = document.getElementById("console-sound");
  let player = null;
  let playhead = 0;
  const sound = canvas.dataset.sound ? {
    ready() { if (soundButton) soundButton.disabled = false; },
    play(bytes) {
      if (!player) return;
      const [left, right] = toChannels(bytes);
      if (!left.length) return;
      // Birikme yarım saniyeyi geçerse parça atılır: ses görüntüden geride kalmasın.
      if (playhead - player.currentTime > 0.5) return;
      const buffer = player.createBuffer(CHANNELS, left.length, RATE);
      buffer.copyToChannel(left, 0);
      buffer.copyToChannel(right, 1);
      const source = player.createBufferSource();
      source.buffer = buffer;
      source.connect(player.destination);
      playhead = Math.max(player.currentTime + 0.05, playhead);
      source.start(playhead);
      playhead += buffer.duration;
    },
  } : null;
  const session = new Session(
    (bytes) => { if (socket.readyState === WebSocket.OPEN) socket.send(bytes); },
    display,
    (state, detail) => {
      if (state === "bağlı") say("Bağlı", false);
      else if (state === "bağlanıyor") say("Bağlanıyor…", false);
      else say("Bağlantı kapandı" + (detail ? ": " + detail : ""), true);
    },
    sound,
  );
  if (soundButton) {
    soundButton.addEventListener("click", () => {
      const on = !player;
      if (!session.listen(on)) return;
      if (on) { player = new AudioContext({ sampleRate: RATE }); playhead = 0; }
      else { player.close(); player = null; }
      soundButton.textContent = on ? soundButton.dataset.on : soundButton.dataset.off;
      soundButton.setAttribute("aria-pressed", String(on));
      canvas.focus();
    });
  }
  socket.onopen = () => session.run();
  socket.onmessage = (e) => session.feed(new Uint8Array(e.data));
  socket.onclose = () => session.end();
  socket.onerror = () => say("Bağlantı kurulamadı", true);

  const onKey = (down) => (e) => {
    // Sayfadan çıkmak için: odak tuvaldeyken Tab konuğa gider; Escape ardından Tab ile odak bırakılmaz,
    // bu yüzden klavye kullanıcıları için "Odağı bırak" düğmesi vardır.
    if (session.key(down, e.key, e.code)) e.preventDefault();
  };
  canvas.addEventListener("keydown", onKey(true));
  canvas.addEventListener("keyup", onKey(false));
  const point = (e) => {
    const box = canvas.getBoundingClientRect();
    return [(e.clientX - box.left) * canvas.width / box.width, (e.clientY - box.top) * canvas.height / box.height];
  };
  const onPointer = (e) => { const [x, y] = point(e); session.pointer(buttonMask(e.buttons), x, y); e.preventDefault(); };
  canvas.addEventListener("mousemove", onPointer);
  canvas.addEventListener("mousedown", (e) => { canvas.focus(); onPointer(e); });
  canvas.addEventListener("mouseup", onPointer);
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("wheel", (e) => {
    const [x, y] = point(e);
    const bit = e.deltaY < 0 ? 8 : 16;
    session.pointer(buttonMask(e.buttons) | bit, x, y);
    session.pointer(buttonMask(e.buttons), x, y);
    e.preventDefault();
  }, { passive: false });
  // Pencere odağı kaybedince basılı kalan değiştirici tuşlar konukta takılı kalmasın.
  canvas.addEventListener("blur", () => {
    [["Shift", "ShiftLeft"], ["Control", "ControlLeft"], ["Alt", "AltLeft"], ["Meta", "MetaLeft"], ["AltGraph", "AltRight"]].forEach(([k, c]) => session.key(false, k, c));
  });
  const on = (id, fn) => { const el = document.getElementById(id); if (el) el.addEventListener("click", fn); };
  on("console-cad", () => { session.ctrlAltDel(); canvas.focus(); });
  on("console-full", () => { if (canvas.requestFullscreen) canvas.requestFullscreen(); canvas.focus(); });
  on("console-blur", () => canvas.blur());
  const field = document.getElementById("console-text");
  const typeButton = document.getElementById("console-type");
  if (field && typeButton) {
    const send = async () => {
      if (!field.value || typeButton.disabled) return;
      typeButton.disabled = true;
      const skipped = await session.type(field.value, () => new Promise((done) => setTimeout(done, 12)));
      typeButton.disabled = false;
      if (skipped) say(skipped + " " + typeButton.dataset.skipped, false, "warn");
      else field.value = "";
      canvas.focus();
    };
    typeButton.addEventListener("click", send);
    field.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); send(); } });
  }
})(typeof window !== "undefined" ? window : globalThis);
