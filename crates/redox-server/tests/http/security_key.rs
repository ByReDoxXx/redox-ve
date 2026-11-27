//! Güvenlik anahtarı (WebAuthn): kayıt, girişin ikinci adımı ve reddedilmesi gereken yanıtlar.
//!
//! Anahtarın yerini gerçek ES256 / Ed25519 imzası atan küçük bir yazılım doğrulayıcı tutar.

use super::*;

use redox_server::oidc::{base64url, base64url_decode};
use redox_server::webauthn::{Relying, ALG_EDDSA, ALG_ES256};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use sha2::{Digest, Sha256};

const ORIGIN: &str = "http://localhost:8006";

enum Signer {
    P256(EcdsaKeyPair),
    Ed(Ed25519KeyPair),
}

struct Authenticator {
    signer: Signer,
    id: Vec<u8>,
    count: u32,
    /// Sınanan sapmalar.
    origin: String,
    rp_id: String,
    flags: u8,
}

impl Authenticator {
    fn new(seed: u8) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let signer = Signer::P256(EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap());
        Authenticator { signer, id: vec![seed; 32], count: 0, origin: ORIGIN.into(), rp_id: "localhost".into(), flags: 0x01 }
    }

    fn ed25519(seed: u8) -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let signer = Signer::Ed(Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap());
        Authenticator { signer, ..Self::new(seed) }
    }

    fn alg(&self) -> i32 {
        match self.signer {
            Signer::P256(_) => ALG_ES256,
            Signer::Ed(_) => ALG_EDDSA,
        }
    }

    /// Tarayıcının `getPublicKey()` yanıtı: SubjectPublicKeyInfo.
    fn spki(&self) -> Vec<u8> {
        let (prefix, key): (&str, &[u8]) = match &self.signer {
            Signer::P256(pair) => ("3059301306072a8648ce3d020106082a8648ce3d030107034200", pair.public_key().as_ref()),
            Signer::Ed(pair) => ("302a300506032b6570032100", pair.public_key().as_ref()),
        };
        let mut out: Vec<u8> = (0..prefix.len()).step_by(2).map(|i| u8::from_str_radix(&prefix[i..i + 2], 16).unwrap()).collect();
        out.extend_from_slice(key);
        out
    }

    fn client_data(&self, kind: &str, challenge: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({ "type": kind, "challenge": challenge, "origin": self.origin, "crossOrigin": false })).unwrap()
    }

    fn auth_data(&self, attested: bool) -> Vec<u8> {
        let mut data = Sha256::digest(self.rp_id.as_bytes()).to_vec();
        data.push(if attested { self.flags | 0x40 } else { self.flags });
        data.extend_from_slice(&self.count.to_be_bytes());
        if attested {
            data.extend_from_slice(&[0; 16]);
            data.extend_from_slice(&(self.id.len() as u16).to_be_bytes());
            data.extend_from_slice(&self.id);
            data.extend_from_slice(&[0xa0]); // COSE anahtarının yeri; sunucu SPKI'yi kullanır
        }
        data
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match &self.signer {
            Signer::P256(pair) => pair.sign(&SystemRandom::new(), message).unwrap().as_ref().to_vec(),
            Signer::Ed(pair) => pair.sign(message).as_ref().to_vec(),
        }
    }

    fn create(&self, page: &str) -> String {
        format!(
            "ticket={}&id={}&client_data={}&auth_data={}&public_key={}&alg={}",
            hidden(page, "ticket"),
            base64url(&self.id),
            base64url(&self.client_data("webauthn.create", &data(page, "challenge"))),
            base64url(&self.auth_data(true)),
            base64url(&self.spki()),
            self.alg()
        )
    }

    /// Anahtar her kullanımda sayacını artırır.
    fn get(&mut self, page: &str) -> String {
        self.count += 1;
        self.assertion(page)
    }

    fn assertion(&self, page: &str) -> String {
        let client_data = self.client_data("webauthn.get", &data(page, "challenge"));
        let auth_data = self.auth_data(false);
        let mut signed = auth_data.clone();
        signed.extend_from_slice(&Sha256::digest(&client_data));
        format!(
            "ticket={}&id={}&client_data={}&auth_data={}&signature={}&next=%2Fguests",
            hidden(page, "ticket"),
            base64url(&self.id),
            base64url(&client_data),
            base64url(&auth_data),
            base64url(&self.sign(&signed))
        )
    }
}

fn between<'a>(text: &'a str, start: &str) -> &'a str {
    let rest = text.split_once(start).unwrap_or_else(|| panic!("{start} yok: {text}")).1;
    rest.split_once('"').unwrap().0
}

fn hidden(page: &str, name: &str) -> String {
    between(page, &format!(r#"name="{name}" value=""#)).to_string()
}

fn data(page: &str, name: &str) -> String {
    between(page, &format!(r#"data-{name}=""#)).to_string()
}

fn keys_app() -> (Router, AppState) {
    let mut auth = Auth::new(users(), None, false, Cost::Test);
    auth.webauthn = Some(Relying::parse(ORIGIN).unwrap());
    let state = AppState::new(Cluster::demo(now()), Box::new(SimDriver::default()), Duration::ZERO, None, auth);
    (app(state.clone()), state)
}

fn with_cookie(cookie: &str, uri: &str, body: &str) -> Request<Body> {
    form_body(anon(Method::POST, uri).header(COOKIE, cookie), body)
}

/// Hesabım sayfasından anahtar ekleme sayfasını açar.
async fn start_add(app: &Router, cookie: &str, label: &str, password: &str) -> (StatusCode, HeaderMap, String) {
    send(app, with_cookie(cookie, "/account/keys/start", &format!("label={label}&current={password}"))).await
}

async fn add_key(app: &Router, cookie: &str, label: &str, key: &Authenticator) -> String {
    let (status, _, page) = start_add(app, cookie, label, ADMIN_PW).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let (status, headers, _) = send(app, with_cookie(cookie, "/account/keys/finish", &key.create(&page))).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    location(&headers).to_string()
}

/// Parola adımı; anahtarlı hesapta anahtar sayfası döner.
async fn password_step(app: &Router) -> String {
    let (status, headers, body) = send(app, login_request("admin", ADMIN_PW, "%2Fguests")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get(SET_COOKIE).is_none(), "parola tek başına oturum açmaz");
    body
}

async fn key_step(app: &Router, body: &str) -> (StatusCode, HeaderMap, String) {
    send(app, form_body(anon(Method::POST, "/login/key"), body)).await
}

async fn enrolled() -> (Router, AppState, Authenticator) {
    let (app, state) = keys_app();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    let key = Authenticator::new(1);
    assert!(add_key(&app, &cookie, "yubikey", &key).await.starts_with("/account?notice="));
    (app, state, key)
}

fn admin_keys(state: &AppState) -> Vec<redox_server::auth::SecurityKey> {
    state.auth().list().into_iter().find(|u| u.name == "admin").unwrap().keys
}

#[tokio::test]
async fn a_registered_key_becomes_the_second_step_of_login() {
    let (app, state, mut key) = enrolled().await;
    assert_eq!(admin_keys(&state).len(), 1);

    // Anahtar sayfası yalnızca kendi betiğini yükler; giriş ve hesap sayfaları betiksiz kalır.
    let (_, headers, page) = send(&app, login_request("admin", ADMIN_PW, "%2Fguests")).await;
    let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("script-src 'self'") && !csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
    assert_eq!(page.matches("<script").count(), 1);
    assert!(page.contains("/assets/webauthn.js?v=") && page.contains(r#"data-mode="login""#) && page.contains(&base64url(&key.id)), "{page}");
    let (_, headers, login) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert!(!login.contains("<script") && headers.get("content-security-policy").unwrap().to_str().unwrap().contains("script-src 'none'"));

    let answer = key.get(&page);
    let (status, headers, _) = key_step(&app, &answer).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/guests"));
    let cookie = headers.get(SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let (status, headers, account) = send(&app, anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(account.contains("yubikey") && !account.contains("<script") && headers.get("content-security-policy").unwrap().to_str().unwrap().contains("script-src 'none'"));
    assert_eq!(admin_keys(&state)[0].sign_count, 1);

    // Aynı yanıt ikinci kez kullanılamaz.
    assert_eq!(key_step(&app, &answer).await.0, StatusCode::UNAUTHORIZED);
    // API token'ları etkilenmez.
    assert_eq!(send(&app, get("/api/v1/cluster")).await.0, StatusCode::OK);
    let log = format!("{:?}", state.auth().audit.recent("", 20));
    assert!(log.contains("giriş (güvenlik anahtarı)") && log.contains("giriş (güvenlik anahtarı reddedildi)"), "{log}");
}

#[tokio::test]
async fn a_code_field_cannot_stand_in_for_the_key() {
    let (app, _, _) = enrolled().await;
    for code in ["000000", "123456", "x"] {
        let (status, headers, _) = send(&app, form_body(anon(Method::POST, "/login"), &format!("username=admin&password={ADMIN_PW}&code={code}&next="))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{code}: uygulama kodu kurulu değil");
        assert!(headers.get(SET_COOKIE).is_none());
    }
    // Yanlış parola anahtar sayfasını göstermez.
    let (status, _, body) = send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!body.contains("webauthn.js"));
}

#[tokio::test]
async fn wrong_answers_are_refused() {
    type Tamper = fn(&mut Authenticator);
    let cases: [(&str, Tamper); 5] = [
        ("başka adres", |k| k.origin = "http://localhost:9999".into()),
        ("sahte site", |k| k.origin = "https://localhost.example.net".into()),
        ("başka alan adı", |k| k.rp_id = "ornek.net".into()),
        ("dokunulmadı", |k| k.flags = 0),
        ("başka anahtarın kimliği", |k| k.id = vec![9; 32]),
    ];
    for (what, tamper) in cases {
        let (app, _, mut key) = enrolled().await;
        let page = password_step(&app).await;
        tamper(&mut key);
        let (status, headers, _) = key_step(&app, &key.get(&page)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
        assert!(headers.get(SET_COOKIE).is_none(), "{what}");
    }

    let (app, _, mut key) = enrolled().await;
    // Aynı kimlikle başka bir anahtar: imza tutmaz.
    let mut impostor = Authenticator::new(1);
    assert_eq!(key_step(&app, &impostor.get(&password_step(&app).await)).await.0, StatusCode::UNAUTHORIZED);
    // Başka bir sınamanın yanıtı bu bilete uymaz.
    let (old, new) = (password_step(&app).await, password_step(&app).await);
    let mixed = key.get(&old).replace(&hidden(&old, "ticket"), &hidden(&new, "ticket"));
    assert_eq!(key_step(&app, &mixed).await.0, StatusCode::UNAUTHORIZED);
    // Kayıt yanıtı giriş yerine geçmez; uydurma bilet de.
    let page = password_step(&app).await;
    assert_eq!(key_step(&app, &key.create(&page)).await.0, StatusCode::UNAUTHORIZED);
    let forged = key.get(&page).replace(&hidden(&page, "ticket"), "uydurma");
    assert_eq!(key_step(&app, &forged).await.0, StatusCode::UNAUTHORIZED);
    // Site dışından gönderilen form reddedilir.
    let page = password_step(&app).await;
    let cross = form_body(anon(Method::POST, "/login/key").header("sec-fetch-site", "cross-site"), &key.get(&page));
    assert_eq!(send(&app, cross).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_counter_that_goes_backwards_is_treated_as_a_copied_key() {
    let (app, state, mut key) = enrolled().await;
    assert_eq!(key_step(&app, &key.get(&password_step(&app).await)).await.0, StatusCode::SEE_OTHER);
    assert_eq!(key_step(&app, &key.get(&password_step(&app).await)).await.0, StatusCode::SEE_OTHER);
    // Kopya eski sayaçtan devam eder.
    let page = password_step(&app).await;
    assert_eq!(key_step(&app, &key.assertion(&page)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(admin_keys(&state)[0].sign_count, 2);
}

#[tokio::test]
async fn failed_key_answers_count_towards_the_lockout() {
    let (app, _, mut key) = enrolled().await;
    let mut impostor = Authenticator::new(1);
    // Sınamalar baştan alınır: kilitliyken parola adımı da kapanır.
    let mut pages = Vec::new();
    for _ in 0..5 {
        pages.push(password_step(&app).await);
    }
    for (i, page) in pages.iter().enumerate() {
        let expected = if i < 4 { StatusCode::UNAUTHORIZED } else { StatusCode::TOO_MANY_REQUESTS };
        assert_eq!(key_step(&app, &impostor.get(page)).await.0, expected, "{i}");
    }
    assert_eq!(send(&app, login_request("admin", ADMIN_PW, "")).await.0, StatusCode::TOO_MANY_REQUESTS);
    // Kilit açılana kadar doğru anahtarın eski sınaması da işe yaramaz.
    assert_ne!(key_step(&app, &key.get(&pages[0])).await.0, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn adding_and_removing_a_key_needs_the_password() {
    let (app, state) = keys_app();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    let (status, headers, _) = start_add(&app, &cookie, "a", "yanlis-parola-123").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/account?error="));

    let key = Authenticator::ed25519(2);
    assert!(add_key(&app, &cookie, "a", &key).await.starts_with("/account?notice="));
    // Aynı anahtar ve aynı ad ikinci kez eklenmez.
    assert!(add_key(&app, &cookie, "b", &key).await.starts_with("/account?error="));
    let (status, headers, _) = start_add(&app, &cookie, "a", ADMIN_PW).await;
    assert!(status == StatusCode::SEE_OTHER && location(&headers).starts_with("/account?error="));
    // Yanlış adresten gelen kayıt eklenmez.
    let mut wrong = Authenticator::new(3);
    wrong.origin = "https://sahte.example".into();
    assert!(add_key(&app, &cookie, "c", &wrong).await.starts_with("/account?error="));
    assert_eq!(admin_keys(&state).len(), 1);
    // Ed25519 anahtarıyla giriş.
    let mut key = key;
    assert_eq!(key_step(&app, &key.get(&password_step(&app).await)).await.0, StatusCode::SEE_OTHER);

    let id = base64url(&key.id);
    let (_, headers, _) = send(&app, with_cookie(&cookie, "/account/keys/remove", &format!("id={id}&current=yanlis-parola-123"))).await;
    assert!(location(&headers).starts_with("/account?error="));
    assert_eq!(admin_keys(&state).len(), 1);
    let (_, headers, _) = send(&app, with_cookie(&cookie, "/account/keys/remove", &format!("id={id}&current={ADMIN_PW}"))).await;
    assert!(location(&headers).starts_with("/account?notice="));
    login_cookie(&app, "admin", ADMIN_PW).await;
    assert!(base64url_decode(&id).is_some());
}

#[tokio::test]
async fn one_account_cannot_finish_anothers_registration() {
    let (app, state) = keys_app();
    let admin = login_cookie(&app, "admin", ADMIN_PW).await;
    let viewer = login_cookie(&app, "izleyici", VIEWER_PW).await;
    let (_, _, page) = start_add(&app, &admin, "a", ADMIN_PW).await;
    let (_, headers, _) = send(&app, with_cookie(&viewer, "/account/keys/finish", &Authenticator::new(4).create(&page))).await;
    assert!(location(&headers).starts_with("/account?error="));
    assert!(state.auth().list().iter().all(|u| u.keys.is_empty()));
}

#[tokio::test]
async fn totp_remains_an_alternative_and_an_admin_can_reset_both() {
    let (app, state, _) = enrolled().await;
    let secret = state.auth().edit(|u| u.begin_totp("admin")).unwrap();
    state.auth().edit(|u| u.confirm_totp("admin", &users::totp_code(&secret), now())).unwrap();
    // Kodsuz giriş anahtara, kodlu giriş doğrudan oturuma gider.
    password_step(&app).await;
    let with_code = form_body(anon(Method::POST, "/login"), &format!("username=admin&password={ADMIN_PW}&code={}&next=", users::totp_code(&secret)));
    assert_eq!(send(&app, with_code).await.0, StatusCode::SEE_OTHER);

    assert!(page(&app, "/users").await.contains("güvenlik anahtarı"));
    assert!(post_ok(&app, "/users/admin/totp-reset", "").await.contains("notice="));
    assert!(admin_keys(&state).is_empty());
    login_cookie(&app, "admin", ADMIN_PW).await;
}

#[tokio::test]
async fn without_a_configured_address_an_account_with_a_key_stays_closed() {
    let (_, state, _) = enrolled().await;
    let file = UserFile { users: state.auth().list() };
    let auth = Auth::new(file, None, false, Cost::Test);
    let closed = app(AppState::new(Cluster::demo(now()), Box::new(SimDriver::default()), Duration::ZERO, None, auth));
    let (status, headers, body) = send(&closed, login_request("admin", ADMIN_PW, "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(SET_COOKIE).is_none() && !body.contains("webauthn.js"));
    // Anahtarı olmayan hesap etkilenmez ve ekleme formu yerine açıklama görür.
    let viewer = login_cookie(&closed, "izleyici", VIEWER_PW).await;
    let (_, _, account) = send(&closed, anon(Method::GET, "/account").header(COOKIE, &viewer).body(Body::empty()).unwrap()).await;
    assert!(account.contains("--webauthn-origin") && !account.contains("/account/keys/start"));
}

#[tokio::test]
async fn the_key_card_and_the_key_step_are_translated() {
    let (app, _, _) = enrolled().await;
    let english = form_body(anon(Method::POST, "/login").header(COOKIE, "lang=en"), &format!("username=admin&password={ADMIN_PW}&code=&next="));
    let (_, _, step) = send(&app, english).await;
    assert!(step.contains("Your password was accepted.") && step.contains("Use the Key") && step.contains(r#"data-failed="The key could not be used."#), "{step}");
    let (app, _) = keys_app();
    let session = login_cookie(&app, "admin", ADMIN_PW).await;
    let both = format!("{session}; lang=en");
    let (_, _, account) = send(&app, anon(Method::GET, "/account").header(COOKIE, &both).body(Body::empty()).unwrap()).await;
    assert!(account.contains("Security keys") && account.contains("Name of the new key") && account.contains("Add Key") && !account.contains("Güvenlik anahtar"), "{account}");
    let (_, headers, _) = send(&app, with_cookie(&both, "/account/keys/start", "label=%20&current=yonetici-parolasi-1")).await;
    let (_, _, shown) = send(&app, anon(Method::GET, location(&headers)).header(COOKIE, &both).body(Body::empty()).unwrap()).await;
    assert!(shown.contains("The key name must be 1 to 40 characters"), "{shown}");
}

#[test]
fn browser_script_passes_its_node_tests() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/webauthn.test.js");
    match std::process::Command::new("node").arg("--test").arg(&script).output() {
        Ok(out) => assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        Err(e) => eprintln!("node bulunamadı, betik testleri atlandı: {e}"),
    }
}
