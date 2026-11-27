// Güvenlik anahtarı betiğinin (assets/webauthn.js) testleri: node --test crates/redox-server/tests/webauthn.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { toBytes, toText, createOptions, getOptions, fields } = require("../assets/webauthn.js");

const buffer = (...bytes) => Uint8Array.from(bytes).buffer;

test("base64url iki yönde de dolgusuz çalışır", () => {
  for (const bytes of [[], [0], [255, 254], [1, 2, 3], [251, 239, 190, 0, 16]]) {
    const text = toText(buffer(...bytes));
    assert.match(text, /^[A-Za-z0-9_-]*$/);
    assert.deepEqual([...toBytes(text)], bytes);
  }
});

test("kayıt ve giriş seçenekleri sayfanın verisinden kurulur", () => {
  const data = { challenge: toText(buffer(1, 2)), rp: "localhost", user: toText(buffer(97)), name: "a", keys: `${toText(buffer(5))},${toText(buffer(6))}` };
  const create = createOptions(data);
  assert.deepEqual([create.rp.id, create.user.name, create.attestation], ["localhost", "a", "none"]);
  assert.deepEqual(create.pubKeyCredParams.map((p) => p.alg), [-7, -8, -257]);
  assert.deepEqual(create.excludeCredentials.map((c) => [...c.id]), [[5], [6]]);
  const get = getOptions(data);
  assert.deepEqual([get.rpId, [...get.challenge], get.allowCredentials.length], ["localhost", [1, 2], 2]);
  assert.deepEqual(getOptions({ ...data, keys: "" }).allowCredentials, []);
});

test("tarayıcı yanıtı form alanlarına çevrilir; eski tarayıcıda kayıt yapılmaz", () => {
  const rawId = buffer(7);
  const login = fields("login", { rawId, response: { clientDataJSON: buffer(1), authenticatorData: buffer(2), signature: buffer(3) } });
  assert.deepEqual(login, { id: toText(rawId), client_data: toText(buffer(1)), auth_data: toText(buffer(2)), signature: toText(buffer(3)) });
  const response = { clientDataJSON: buffer(1), getAuthenticatorData: () => buffer(2), getPublicKey: () => buffer(4), getPublicKeyAlgorithm: () => -7 };
  assert.deepEqual(fields("register", { rawId, response }), { id: toText(rawId), client_data: toText(buffer(1)), auth_data: toText(buffer(2)), public_key: toText(buffer(4)), alg: "-7" });
  assert.equal(fields("register", { rawId, response: { clientDataJSON: buffer(1) } }), null);
  assert.equal(fields("register", { rawId, response: { ...response, getPublicKey: () => null } }), null);
});
