// RedoX VE güvenlik anahtarı (WebAuthn): tarayıcının anahtar yanıtını forma yazıp gönderir.
// Denetimin tamamı sunucudadır; bu betik yalnızca taşır.
"use strict";

function toBytes(text) {
  const raw = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

function toText(buffer) {
  let raw = "";
  for (const byte of new Uint8Array(buffer)) raw += String.fromCharCode(byte);
  return btoa(raw).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const known = (list) => list.split(",").filter(Boolean).map((id) => ({ type: "public-key", id: toBytes(id) }));

// Parola ilk adımdır: anahtardan kullanıcı doğrulaması (PIN, parmak izi) yalnızca elverişliyse istenir.
function createOptions(data) {
  return {
    challenge: toBytes(data.challenge),
    rp: { id: data.rp, name: "RedoX VE" },
    user: { id: toBytes(data.user), name: data.name, displayName: data.name },
    pubKeyCredParams: [-7, -8, -257].map((alg) => ({ type: "public-key", alg })),
    excludeCredentials: known(data.keys),
    authenticatorSelection: { residentKey: "discouraged", userVerification: "preferred" },
    attestation: "none",
    timeout: 120000,
  };
}

function getOptions(data) {
  return { challenge: toBytes(data.challenge), rpId: data.rp, allowCredentials: known(data.keys), userVerification: "preferred", timeout: 120000 };
}

// Tarayıcının yanıtından formun alanları.
function fields(mode, credential) {
  const response = credential.response;
  const out = { id: toText(credential.rawId), client_data: toText(response.clientDataJSON) };
  if (mode === "register") {
    // Açık anahtar tarayıcıdan hazır biçimde alınır (WebAuthn Level 2).
    const key = response.getPublicKey && response.getPublicKey();
    if (!key || !response.getAuthenticatorData) return null;
    out.auth_data = toText(response.getAuthenticatorData());
    out.public_key = toText(key);
    out.alg = String(response.getPublicKeyAlgorithm());
  } else {
    out.auth_data = toText(response.authenticatorData);
    out.signature = toText(response.signature);
  }
  return out;
}

function start(document, window) {
  const form = document.getElementById("key-form");
  if (!form) return;
  const button = document.getElementById("key-go");
  const status = document.getElementById("key-status");
  const say = (text) => {
    status.textContent = text;
    status.hidden = false;
  };
  if (!window.PublicKeyCredential || !window.navigator.credentials) {
    say(form.dataset.unsupported);
    button.disabled = true;
    return;
  }
  const mode = form.dataset.mode;
  button.addEventListener("click", async () => {
    button.disabled = true;
    status.hidden = true;
    try {
      const credential =
        mode === "register"
          ? await window.navigator.credentials.create({ publicKey: createOptions(form.dataset) })
          : await window.navigator.credentials.get({ publicKey: getOptions(form.dataset) });
      const values = fields(mode, credential);
      if (!values) {
        say(form.dataset.unsupported);
        return;
      }
      for (const [name, value] of Object.entries(values)) form.elements.namedItem(name).value = value;
      form.submit();
    } catch (error) {
      say(`${form.dataset.failed} (${error.name})`);
      button.disabled = false;
    }
  });
}

if (typeof module !== "undefined") module.exports = { toBytes, toText, createOptions, getOptions, fields };
else start(document, window);
