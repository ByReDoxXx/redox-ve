// RedoX VE konteyner terminali: küçük bir VT100/xterm öykünücüsü ve WebSocket istemcisi.
// Konuktan gelen hiçbir şey HTML olarak yorumlanmaz: ekran yalnızca textContent ile çizilir.
"use strict";

const SCROLLBACK = 1000;
const blank = () => ({ ch: " ", fg: -1, bg: -1, bold: false, inverse: false });

class Screen {
  constructor(cols, rows, reply) {
    this.cols = cols;
    this.rows = rows;
    this.reply = reply || (() => {});
    this.lines = Array.from({ length: rows }, () => this.newLine());
    this.history = [];
    this.pushed = 0; // geçmişe aktarılan toplam satır (çizim bununla izlenir)
    this.saved = null;
    this.alternate = null;
    this.dirty = new Set();
    this.reset();
  }

  newLine() {
    return Array.from({ length: this.cols }, blank);
  }

  reset() {
    this.x = 0;
    this.y = 0;
    this.top = 0;
    this.bottom = this.rows - 1;
    this.style = blank();
    this.wrapPending = false;
    this.cursorVisible = true;
    this.appCursor = false;
    this.bracketedPaste = false;
    this.state = "ground";
    this.params = "";
    this.touchAll();
  }

  touch(y) {
    this.dirty.add(y);
  }

  touchAll() {
    for (let y = 0; y < this.rows; y++) this.dirty.add(y);
  }

  text(y) {
    return this.lines[y].map((c) => c.ch).join("").replace(/ +$/, "");
  }

  erased() {
    return { ...blank(), bg: this.style.bg };
  }

  scrollUp(count) {
    for (let i = 0; i < count; i++) {
      const [line] = this.lines.splice(this.top, 1);
      // Yalnızca olağan ekranın tepesinden çıkan satır geçmişe gider.
      if (this.top === 0 && !this.alternate) {
        this.history.push(line);
        this.pushed++;
        if (this.history.length > SCROLLBACK) this.history.shift();
      }
      this.lines.splice(this.bottom, 0, this.newLine());
    }
    this.touchAll();
  }

  scrollDown(count) {
    for (let i = 0; i < count; i++) {
      this.lines.splice(this.bottom, 1);
      this.lines.splice(this.top, 0, this.newLine());
    }
    this.touchAll();
  }

  lineFeed() {
    if (this.y === this.bottom) this.scrollUp(1);
    else if (this.y < this.rows - 1) this.y++;
    this.touch(this.y);
  }

  move(x, y) {
    this.touch(this.y);
    this.x = Math.max(0, Math.min(this.cols - 1, x));
    this.y = Math.max(0, Math.min(this.rows - 1, y));
    this.wrapPending = false;
    this.touch(this.y);
  }

  put(ch) {
    if (this.wrapPending) {
      this.x = 0;
      this.lineFeed();
      this.wrapPending = false;
    }
    this.lines[this.y][this.x] = { ...this.style, ch };
    this.touch(this.y);
    if (this.x === this.cols - 1) this.wrapPending = true;
    else this.x++;
  }

  write(data) {
    for (const ch of data) this.feed(ch);
  }

  feed(ch) {
    const code = ch.codePointAt(0);
    if (this.state === "osc") {
      // Pencere başlığı gibi iletiler yok sayılır; BEL ya da ESC \ ile biter.
      if (code === 7) this.state = "ground";
      else if (code === 27) this.state = "esc";
      return;
    }
    if (code === 27) {
      this.state = "esc";
      return;
    }
    if (code < 32 || code === 127) {
      if (code === 13) this.move(0, this.y);
      else if (code === 10 || code === 11 || code === 12) {
        this.wrapPending = false;
        this.lineFeed();
      } else if (code === 8) this.move(this.x - 1, this.y);
      else if (code === 9) this.move((Math.floor(this.x / 8) + 1) * 8, this.y);
      return;
    }
    switch (this.state) {
      case "ground":
        this.put(ch);
        break;
      case "esc":
        this.escape(ch);
        break;
      case "charset":
        this.state = "ground";
        break;
      case "csi":
        if (/[0-9;?>=! "$']/.test(ch)) {
          if (this.params.length < 64) this.params += ch;
        } else {
          this.state = "ground";
          this.csi(ch, this.params);
        }
        break;
    }
  }

  escape(ch) {
    this.state = "ground";
    if (ch === "[") {
      this.state = "csi";
      this.params = "";
    } else if (ch === "]") this.state = "osc";
    else if (ch === "(" || ch === ")" || ch === "#" || ch === "%") this.state = "charset";
    else if (ch === "7") this.saveCursor();
    else if (ch === "8") this.restoreCursor();
    else if (ch === "D") this.lineFeed();
    else if (ch === "E") {
      this.move(0, this.y);
      this.lineFeed();
    } else if (ch === "M") {
      if (this.y === this.top) this.scrollDown(1);
      else this.move(this.x, this.y - 1);
    } else if (ch === "c") {
      this.lines = Array.from({ length: this.rows }, () => this.newLine());
      this.reset();
    }
  }

  saveCursor() {
    this.saved = { x: this.x, y: this.y, style: { ...this.style } };
  }

  restoreCursor() {
    if (!this.saved) return;
    this.style = { ...this.saved.style };
    this.move(this.saved.x, this.saved.y);
  }

  clearRange(y, from, to) {
    for (let x = from; x < to; x++) this.lines[y][x] = this.erased();
    this.touch(y);
  }

  csi(final, raw) {
    const secret = raw.startsWith("?");
    const nums = raw.replace(/[^0-9;]/g, "").split(";").map((n) => (n === "" ? 0 : Math.min(parseInt(n, 10), 9999)));
    const n = nums[0] || 0;
    const one = n || 1;
    if (raw.startsWith(">") || raw.startsWith("=")) {
      if (final === "c") this.reply("\x1b[>0;0;0c");
      return;
    }
    if (secret) {
      if (final !== "h" && final !== "l") return;
      const on = final === "h";
      for (const mode of nums) {
        if (mode === 1) this.appCursor = on;
        else if (mode === 25) this.cursorVisible = on;
        else if (mode === 2004) this.bracketedPaste = on;
        else if (mode === 1049 || mode === 1047 || mode === 47) this.useAlternate(on);
      }
      this.touchAll();
      return;
    }
    switch (final) {
      case "A": this.move(this.x, this.y - one); break;
      case "B": case "e": this.move(this.x, this.y + one); break;
      case "C": case "a": this.move(this.x + one, this.y); break;
      case "D": this.move(this.x - one, this.y); break;
      case "E": this.move(0, this.y + one); break;
      case "F": this.move(0, this.y - one); break;
      case "G": case "`": this.move(one - 1, this.y); break;
      case "d": this.move(this.x, one - 1); break;
      case "H": case "f": this.move((nums[1] || 1) - 1, one - 1); break;
      case "J":
        if (n === 0) {
          this.clearRange(this.y, this.x, this.cols);
          for (let y = this.y + 1; y < this.rows; y++) this.clearRange(y, 0, this.cols);
        } else if (n === 1) {
          for (let y = 0; y < this.y; y++) this.clearRange(y, 0, this.cols);
          this.clearRange(this.y, 0, this.x + 1);
        } else {
          for (let y = 0; y < this.rows; y++) this.clearRange(y, 0, this.cols);
          if (n === 3) {
            this.history = [];
            this.cleared = true;
          }
        }
        break;
      case "K":
        if (n === 0) this.clearRange(this.y, this.x, this.cols);
        else if (n === 1) this.clearRange(this.y, 0, this.x + 1);
        else this.clearRange(this.y, 0, this.cols);
        break;
      case "L": case "M": {
        if (this.y < this.top || this.y > this.bottom) break;
        for (let i = 0; i < Math.min(one, this.rows); i++) {
          if (final === "L") {
            this.lines.splice(this.bottom, 1);
            this.lines.splice(this.y, 0, this.newLine());
          } else {
            this.lines.splice(this.y, 1);
            this.lines.splice(this.bottom, 0, this.newLine());
          }
        }
        this.touchAll();
        break;
      }
      case "P": case "@": {
        const line = this.lines[this.y];
        for (let i = 0; i < Math.min(one, this.cols - this.x); i++) {
          if (final === "P") {
            line.splice(this.x, 1);
            line.push(this.erased());
          } else {
            line.splice(this.x, 0, this.erased());
            line.pop();
          }
        }
        this.touch(this.y);
        break;
      }
      case "X": this.clearRange(this.y, this.x, Math.min(this.cols, this.x + one)); break;
      case "S": this.scrollUp(Math.min(one, this.rows)); break;
      case "T": this.scrollDown(Math.min(one, this.rows)); break;
      case "r": {
        const top = (n || 1) - 1;
        const bottom = (nums[1] || this.rows) - 1;
        if (top < bottom && bottom < this.rows) {
          this.top = top;
          this.bottom = bottom;
          this.move(0, 0);
        }
        break;
      }
      case "m": this.sgr(nums); break;
      case "s": this.saveCursor(); break;
      case "u": this.restoreCursor(); break;
      case "n":
        if (n === 6) this.reply(`\x1b[${this.y + 1};${this.x + 1}R`);
        else if (n === 5) this.reply("\x1b[0n");
        break;
      case "c": this.reply("\x1b[?1;2c"); break;
    }
  }

  useAlternate(on) {
    if (on && !this.alternate) {
      this.alternate = { lines: this.lines, x: this.x, y: this.y };
      this.lines = Array.from({ length: this.rows }, () => this.newLine());
    } else if (!on && this.alternate) {
      const main = this.alternate;
      this.alternate = null;
      this.lines = main.lines;
      this.top = 0;
      this.bottom = this.rows - 1;
      this.move(main.x, main.y);
    }
  }

  sgr(nums) {
    for (let i = 0; i < nums.length; i++) {
      const v = nums[i];
      if (v === 0) this.style = blank();
      else if (v === 1) this.style.bold = true;
      else if (v === 22) this.style.bold = false;
      else if (v === 7) this.style.inverse = true;
      else if (v === 27) this.style.inverse = false;
      else if (v >= 30 && v <= 37) this.style.fg = v - 30;
      else if (v >= 90 && v <= 97) this.style.fg = v - 90 + 8;
      else if (v === 39) this.style.fg = -1;
      else if (v >= 40 && v <= 47) this.style.bg = v - 40;
      else if (v >= 100 && v <= 107) this.style.bg = v - 100 + 8;
      else if (v === 49) this.style.bg = -1;
      else if (v === 38 || v === 48) {
        // 256 renk ve gerçek renk: ilk 16 renk dışındakiler olağan renge indirilir.
        let color = -1;
        if (nums[i + 1] === 5) {
          color = nums[i + 2] < 16 ? nums[i + 2] : -1;
          i += 2;
        } else if (nums[i + 1] === 2) i += 4;
        if (v === 38) this.style.fg = color;
        else this.style.bg = color;
      }
    }
  }

  // Bir satırı aynı biçimli parçalara böler: [{ text, classes }]. `cursor`: imlecin sütunu ya da -1.
  runs(line, cursor) {
    const out = [];
    let end = line.length;
    while (end > 0 && line[end - 1].ch === " " && line[end - 1].bg === -1 && !line[end - 1].inverse && end - 1 > cursor) end--;
    for (let x = 0; x < end; x++) {
      const c = line[x];
      let fg = c.fg;
      let bg = c.bg;
      let inverse = c.inverse;
      if (x === cursor) inverse = !inverse;
      const classes = [];
      if (inverse) classes.push("t-inv");
      if (fg >= 0) classes.push(`t-fg${c.bold && fg < 8 ? fg + 8 : fg}`);
      if (bg >= 0) classes.push(`t-bg${bg}`);
      if (c.bold) classes.push("t-b");
      const name = classes.join(" ");
      const last = out[out.length - 1];
      if (last && last.classes === name) last.text += c.ch;
      else out.push({ text: c.ch, classes: name });
    }
    return out;
  }
}

// Tuş → konuğa gidecek baytlar. Yazdırılabilir karakterler burada ele alınmaz: onlar metin girişinden gelir,
// böylece klavye düzeni (AltGr, ölü tuşlar) tarayıcınınkiyle aynı olur.
function keySequence(event, appCursor) {
  const arrows = { ArrowUp: "A", ArrowDown: "B", ArrowRight: "C", ArrowLeft: "D", Home: "H", End: "F" };
  if (event.metaKey) return null;
  const altGr = event.ctrlKey && event.altKey;
  if (arrows[event.key]) return (appCursor ? "\x1bO" : "\x1b[") + arrows[event.key];
  const fixed = {
    Enter: "\r", Backspace: "\x7f", Tab: event.shiftKey ? "\x1b[Z" : "\t", Escape: "\x1b",
    Insert: "\x1b[2~", Delete: "\x1b[3~", PageUp: "\x1b[5~", PageDown: "\x1b[6~",
    F1: "\x1bOP", F2: "\x1bOQ", F3: "\x1bOR", F4: "\x1bOS", F5: "\x1b[15~", F6: "\x1b[17~",
    F7: "\x1b[18~", F8: "\x1b[19~", F9: "\x1b[20~", F10: "\x1b[21~", F11: "\x1b[23~", F12: "\x1b[24~",
  };
  if (fixed[event.key] !== undefined) return fixed[event.key];
  if (event.ctrlKey && !altGr && event.key.length === 1) {
    // Ctrl+Shift+C / V tarayıcının kopyala-yapıştırıdır.
    if (event.shiftKey && /^[cv]$/i.test(event.key)) return null;
    const upper = event.key.toUpperCase();
    if (upper >= "A" && upper <= "Z") return String.fromCharCode(upper.charCodeAt(0) - 64);
    const extra = { " ": "\x00", "[": "\x1b", "\\": "\x1c", "]": "\x1d" };
    return extra[event.key] !== undefined ? extra[event.key] : null;
  }
  if (event.altKey && !altGr && event.key.length === 1) return "\x1b" + event.key;
  return null;
}

// Kutuya (iki yanda 8 piksel iç boşlukla) sığan sütun ve satır sayısı.
function fit(width, height, cell) {
  return {
    cols: Math.max(40, Math.min(240, Math.floor((width - 16) / cell.w))),
    rows: Math.max(10, Math.min(80, Math.floor((height - 16) / cell.h))),
  };
}

function start(document, window) {
  const host = document.getElementById("terminal");
  if (!host) return;
  const status = document.getElementById("terminal-status");
  const say = (text, kind) => {
    status.textContent = text;
    status.className = `pill ${kind}`;
  };
  const probe = document.createElement("div");
  probe.className = "t-row";
  const sample = document.createElement("span");
  sample.textContent = "MMMMMMMMMM";
  probe.appendChild(sample);
  host.appendChild(probe);
  const cell = { w: sample.getBoundingClientRect().width / 10 || 8, h: probe.getBoundingClientRect().height || 17 };
  host.removeChild(probe);
  // Ölçü kutunun kendisinden alınır: ekranın tamamı kaydırmadan görünmelidir (vi, top).
  const { cols, rows } = fit(host.clientWidth, host.clientHeight, cell);

  const url = `${window.location.protocol === "https:" ? "wss" : "ws"}://${window.location.host}${host.dataset.ws}?cols=${cols}&rows=${rows}`;
  const socket = new window.WebSocket(url);
  socket.binaryType = "arraybuffer";
  const encoder = new window.TextEncoder();
  const decoder = new window.TextDecoder("utf-8");
  const send = (text) => {
    if (socket.readyState === 1) socket.send(encoder.encode(text));
  };
  const screen = new Screen(cols, rows, send);

  const past = document.createElement("div");
  const live = document.createElement("div");
  const rowNodes = Array.from({ length: rows }, () => {
    const node = document.createElement("div");
    node.className = "t-row";
    live.appendChild(node);
    return node;
  });
  host.append(past, live);
  const fill = (node, runs) => {
    node.replaceChildren(
      ...runs.map((run) => {
        const span = document.createElement("span");
        span.textContent = run.text;
        if (run.classes) span.className = run.classes;
        return span;
      }),
    );
  };

  let drawn = 0;
  let scheduled = false;
  let lastCursorRow = 0;
  const paint = () => {
    scheduled = false;
    const stick = host.scrollTop + host.clientHeight >= host.scrollHeight - 4;
    if (screen.cleared) {
      past.replaceChildren();
      screen.cleared = false;
      drawn = screen.pushed;
    }
    const fresh = Math.min(screen.pushed - drawn, screen.history.length);
    for (const line of screen.history.slice(screen.history.length - fresh)) {
      const node = document.createElement("div");
      node.className = "t-row";
      fill(node, screen.runs(line, -1));
      past.appendChild(node);
    }
    drawn = screen.pushed;
    while (past.childElementCount > SCROLLBACK) past.removeChild(past.firstElementChild);
    // Seçenek ekranında (vi, top) yalnızca ekranın kendisi görünür.
    past.hidden = screen.alternate !== null;
    screen.dirty.add(lastCursorRow);
    screen.dirty.add(screen.y);
    for (const y of screen.dirty) fill(rowNodes[y], screen.runs(screen.lines[y], screen.cursorVisible && y === screen.y ? screen.x : -1));
    screen.dirty.clear();
    lastCursorRow = screen.y;
    if (stick) host.scrollTop = host.scrollHeight;
  };
  const refresh = () => {
    if (!scheduled) {
      scheduled = true;
      window.requestAnimationFrame(paint);
    }
  };

  const input = document.getElementById("terminal-input");
  socket.onopen = () => {
    say(host.dataset.connected, "ok");
    input.focus();
  };
  socket.onmessage = (event) => {
    screen.write(decoder.decode(event.data, { stream: true }));
    refresh();
  };
  socket.onclose = () => say(host.dataset.closed, "idle");

  input.addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    const sequence = keySequence(event, screen.appCursor);
    if (sequence !== null) {
      event.preventDefault();
      send(sequence);
      host.scrollTop = host.scrollHeight;
    }
  });
  const flushInput = () => {
    if (input.value) {
      send(input.value.replace(/\n/g, "\r"));
      input.value = "";
      host.scrollTop = host.scrollHeight;
    }
  };
  input.addEventListener("input", (event) => {
    if (!event.isComposing) flushInput();
  });
  input.addEventListener("compositionend", flushInput);
  input.addEventListener("paste", (event) => {
    event.preventDefault();
    const text = event.clipboardData.getData("text").replace(/\r?\n/g, "\r");
    // Yapıştırılan metin, çerçeveleme dizilerini taklit edemez.
    send(screen.bracketedPaste ? `\x1b[200~${text.replace(/\x1b/g, "")}\x1b[201~` : text);
  });
  host.addEventListener("click", () => {
    if (!String(window.getSelection())) input.focus({ preventScroll: true });
  });
  refresh();
}

if (typeof module !== "undefined") module.exports = { Screen, keySequence, fit };
else start(document, window);
