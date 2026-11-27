use super::*;

const PW: &str = "dogru-at-pil-zimba";

fn users() -> UserFile {
    let mut f = UserFile::default();
    f.add_user("admin", PW, Role::Admin, Cost::Test).unwrap();
    f.add_user("izleyici", "izleyici-parolasi-1", Role::Viewer, Cost::Test).unwrap();
    f
}

fn auth() -> Auth {
    Auth::new(users(), None, false, Cost::Test)
}

#[test]
fn sessions_survive_a_restart_without_storing_the_token() {
    let dir = std::env::temp_dir().join(format!("redox-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("users.json");
    let mut file = UserFile::default();
    file.add_user("admin", PW, Role::Admin, Cost::Test).unwrap();
    file.save(&path).unwrap();

    let first = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    let token = first.login("ip", "admin", PW, 1_000).unwrap();
    let other = first.login("ip", "admin", PW, 1_000).unwrap();
    let stored = std::fs::read_to_string(dir.join("sessions.json")).unwrap();
    assert!(!stored.contains(&token) && stored.contains(&sha256_hex(&token)), "dosyada token'ın kendisi değil özeti durur");
    drop(first);

    // Yeniden başlayan hizmet oturumu tanır; süre sınırları kaldığı yerden işler.
    let second = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    assert_eq!(second.session(&token, 1_500).map(|i| i.user), Some("admin".to_string()));
    assert!(second.session(&token, 1_500 + IDLE_SECS).is_none(), "boşta kalma süresi yeniden başlamaz");
    // Çıkış kalıcıdır.
    second.logout(&other);
    drop(second);
    let third = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    assert!(third.session(&other, 1_600).is_none() && third.session(&token, 1_600).is_none());

    // Bozuk dosya hizmeti durdurmaz: yalnızca herkes yeniden giriş yapar.
    std::fs::write(dir.join("sessions.json"), "{ bozuk").unwrap();
    let fourth = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    assert!(fourth.login("ip", "admin", PW, 2_000).is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scope_and_sso_are_validated() {
    let mut f = UserFile::default();
    f.add_user("nobet", PW, Role::Operator, Cost::Test).unwrap();
    f.add_user("ikinci", PW, Role::Viewer, Cost::Test).unwrap();
    f.set_scope("nobet", vec!["web".into(), "db".into(), "web".into()]).unwrap();
    assert_eq!(f.find("nobet").unwrap().scope, ["db", "web"], "sıralı ve yinelemesiz");
    assert!(f.set_scope("nobet", vec!["Büyük Harf".into()]).unwrap_err().contains("geçersiz etiket"));
    assert!(f.set_scope("yok", Vec::new()).is_err());
    assert_eq!("operator".parse::<Role>(), Ok(Role::Operator));

    let op = Identity { user: "nobet".into(), role: Role::Operator, scope: vec!["web".into()], permits: None };
    assert!(op.can_operate(&["web".into(), "x".into()]) && !op.can_operate(&["db".into()]) && !op.can_operate(&[]) && !op.can_write());
    let all = Identity { scope: Vec::new(), ..op.clone() };
    assert!(all.can_operate(&[]), "boş kapsam bütün konuklar demektir");
    let viewer = Identity { role: Role::Viewer, ..all.clone() };
    assert!(!viewer.can_operate(&["web".into()]));

    f.set_sso("nobet", "nobet@example.com").unwrap();
    assert!(f.set_sso("ikinci", "nobet@example.com").unwrap_err().contains("başka bir kullanıcıyla"));
    assert!(f.set_sso("ikinci", "iki satır\nx").is_err());
    f.set_sso("nobet", "").unwrap();
    assert_eq!(f.find("nobet").unwrap().sso, None);
}

#[test]
fn password_hash_verifies_and_is_salted() {
    let a = hash_password(PW, Cost::Test);
    let b = hash_password(PW, Cost::Test);
    assert!(a.starts_with("$argon2id$"));
    assert_ne!(a, b, "aynı parola farklı tuzla farklı özet vermeli");
    assert!(verify_password(&a, PW) && verify_password(&b, PW));
    assert!(!verify_password(&a, "yanlis-parola-123"));
    assert!(!verify_password("bozuk-ozet", PW));
    assert!(!verify_password("", ""));
}

#[test]
fn production_cost_uses_owasp_parameters() {
    let h = hash_password(PW, Cost::Production);
    assert!(h.contains("m=19456,t=2,p=1"), "{h}");
    assert!(verify_password(&h, PW));
}

#[test]
fn tokens_are_random_and_long() {
    let (a, b) = (random_token(), random_token());
    assert_eq!(a.len(), 64);
    assert_ne!(a, b);
    assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn user_rules() {
    let mut f = users();
    assert!(f.add_user("admin", PW, Role::Admin, Cost::Test).unwrap_err().contains("zaten var"));
    assert!(f.add_user("Büyük", PW, Role::Admin, Cost::Test).unwrap_err().contains("geçersiz kullanıcı adı"));
    assert!(f.add_user("", PW, Role::Admin, Cost::Test).is_err());
    assert!(f.add_user("yeni", "kisa", Role::Viewer, Cost::Test).unwrap_err().contains("en az 12 karakter"));
    assert!(f.set_password("admin", "kisa", Cost::Test).is_err());
    assert!(f.set_password("yok", PW, Cost::Test).unwrap_err().contains("bulunamadı"));

    assert!(f.remove_user("admin").unwrap_err().contains("son yönetici"));
    f.remove_user("izleyici").unwrap();
    f.add_user("ikinci", PW, Role::Admin, Cost::Test).unwrap();
    f.remove_user("admin").unwrap();
    assert_eq!(f.users.len(), 1);
}

#[test]
fn login_and_session_lifecycle() {
    let a = auth();
    assert_eq!(a.login("ip", "admin", "yanlis-parola-123", 100), Err(LoginError::Invalid));
    assert_eq!(a.login("ip", "yok", PW, 100), Err(LoginError::Invalid));

    let t = a.login("ip", "admin", PW, 100).unwrap();
    assert_eq!(a.session(&t, 200), Some(Identity { user: "admin".into(), role: Role::Admin, scope: Vec::new(), permits: None }));
    assert_eq!(a.session("uydurma", 200), None);

    let t2 = a.login("ip", "admin", PW, 100).unwrap();
    assert_ne!(t, t2, "her giriş yeni token üretir");
    a.logout(&t);
    assert_eq!(a.session(&t, 200), None);
    assert!(a.session(&t2, 200).is_some(), "çıkış yalnızca o oturumu kapatır");
}

#[test]
fn sessions_expire_when_idle_or_too_old() {
    let a = auth();
    let t = a.login("ip", "admin", PW, 0).unwrap();
    assert!(a.session(&t, IDLE_SECS - 1).is_some());
    assert!(a.session(&t, 2 * IDLE_SECS - 2).is_some(), "kullanım hareketsizlik süresini uzatır");
    assert!(a.session(&t, 2 * IDLE_SECS - 2 + IDLE_SECS).is_none(), "hareketsiz oturum düşer");

    let t = a.login("ip", "admin", PW, 0).unwrap();
    let mut now = 0;
    while now + IDLE_SECS / 2 < ABSOLUTE_SECS {
        now += IDLE_SECS / 2;
        assert!(a.session(&t, now).is_some(), "{now}");
    }
    assert!(a.session(&t, ABSOLUTE_SECS).is_none(), "sürekli kullanılsa da mutlak süre dolunca düşer");
}

#[test]
fn lockout_after_repeated_failures_is_per_client() {
    let a = auth();
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(a.login("1.1.1.1", "admin", "yanlis-parola-123", 0), Err(LoginError::Invalid));
    }
    assert_eq!(a.login("1.1.1.1", "admin", "yanlis-parola-123", 0), Err(LoginError::Locked(LOCK_SECS)));
    assert_eq!(a.login("1.1.1.1", "admin", PW, 10), Err(LoginError::Locked(LOCK_SECS - 10)), "kilitliyken doğru parola da reddedilir");
    assert_eq!(a.login("1.1.1.1", "ADMIN", PW, 10), Err(LoginError::Locked(LOCK_SECS - 10)), "büyük/küçük harf kilidi aşmaz");
    assert!(a.login("2.2.2.2", "admin", PW, 10).is_ok(), "başka istemci etkilenmez");
    assert!(a.login("1.1.1.1", "admin", PW, LOCK_SECS).is_ok(), "süre dolunca açılır");
}

#[test]
fn failures_outside_window_do_not_accumulate() {
    let a = auth();
    for i in 0..20 {
        let now = i * (WINDOW_SECS + 1);
        assert_eq!(a.login("ip", "admin", "yanlis-parola-123", now), Err(LoginError::Invalid));
    }
    // Başarılı giriş sayacı sıfırlar.
    let now = 21 * (WINDOW_SECS + 1);
    for _ in 0..MAX_FAILURES - 1 {
        a.login("ip", "admin", "yanlis-parola-123", now).unwrap_err();
    }
    a.login("ip", "admin", PW, now).unwrap();
    assert_eq!(a.login("ip", "admin", "yanlis-parola-123", now), Err(LoginError::Invalid));
}

#[test]
fn bearer_tokens_are_stored_hashed() {
    let mut f = users();
    let plain = f.create_token("izleyici", "ci").unwrap();
    assert!(plain.starts_with("redox_") && plain.len() == 70);
    let json = serde_json::to_string(&f).unwrap();
    assert!(!json.contains(&plain), "düz token dosyaya yazılmamalı");
    assert!(f.create_token("izleyici", "ci").unwrap_err().contains("zaten var"));
    assert!(f.create_token("yok", "ci").is_err());

    let a = Auth::new(f.clone(), None, false, Cost::Test);
    assert_eq!(a.bearer(&plain), Some(Identity { user: "izleyici".into(), role: Role::Viewer, scope: Vec::new(), permits: None }));
    assert_eq!(a.bearer("redox_yanlis"), None);
    assert_eq!(a.bearer(""), None);

    f.revoke_token("izleyici", "ci").unwrap();
    assert!(f.revoke_token("izleyici", "ci").is_err());
    assert_eq!(Auth::new(f, None, false, Cost::Test).bearer(&plain), None);
}

#[test]
fn cookie_flags() {
    let plain = Auth::new(users(), None, false, Cost::Test).session_cookie("t");
    assert!(plain.contains("HttpOnly") && plain.contains("SameSite=Strict") && !plain.contains("Secure"));
    let tls = Auth::new(users(), None, true, Cost::Test);
    assert!(tls.session_cookie("t").ends_with("; Secure"));
    assert!(tls.clear_cookie().contains("Max-Age=0"));
}

#[test]
fn user_file_changes_take_effect_without_restart() {
    let dir = std::env::temp_dir().join(format!("redox-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("users.json");
    assert_eq!(UserFile::load(&path), Ok(UserFile::default()), "dosya yoksa boş liste");

    let mut f = users();
    f.save(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let a = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    let t = a.login("ip", "admin", PW, 0).unwrap();
    assert!(a.session(&t, 1).is_some());

    // Parola değişince (dosya yeniden yazılır) eski oturum düşer, eski parola geçmez.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    f.set_password("admin", "yepyeni-parola-456", Cost::Test).unwrap();
    f.save(&path).unwrap();
    assert!(a.session(&t, 2).is_none());
    assert_eq!(a.login("ip", "admin", PW, 2), Err(LoginError::Invalid));
    assert!(a.login("ip", "admin", "yepyeni-parola-456", 2).is_ok());

    std::fs::write(&path, b"{ bozuk").unwrap();
    assert!(UserFile::load(&path).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn second_factor_is_required_once_enabled() {
    let mut f = users();
    let secret = f.begin_totp("admin").unwrap();
    assert!(f.find("admin").unwrap().totp_secret.is_none(), "doğrulanana kadar etkin değil");
    assert!(f.confirm_totp("admin", "000000", 1000).is_err() || crate::totp::verify(&secret, "000000", 1000).is_some());
    let code = |now: u64| format!("{:06}", crate::totp::code_at(&crate::totp::base32_decode(&secret).unwrap(), now / 30));
    assert!(f.confirm_totp("izleyici", &code(1000), 1000).unwrap_err().contains("önce kurulumu"));
    f.confirm_totp("admin", &code(1000), 1000).unwrap();
    assert_eq!(f.find("admin").unwrap().totp_pending, None);

    let a = Auth::new(f.clone(), None, false, Cost::Test);
    assert_eq!(a.login("ip", "admin", PW, 2000), Err(LoginError::Invalid), "kod olmadan parola yetmez");
    assert_eq!(a.login_with_code("ip", "admin", PW, "123", 2000), Err(LoginError::Invalid));
    assert_eq!(a.login_with_code("ip", "admin", "yanlis-parola-123", &code(2000), 2000), Err(LoginError::Invalid));
    let token = a.login_with_code("ip2", "admin", PW, &code(2000), 2000).unwrap();
    assert!(a.session(&token, 2001).is_some());
    // Aynı kod (ya da daha eski bir adımın kodu) ikinci kez kullanılamaz.
    assert_eq!(a.login_with_code("ip3", "admin", PW, &code(2000), 2001), Err(LoginError::Invalid));
    assert_eq!(a.login_with_code("ip4", "admin", PW, &code(1970), 2001), Err(LoginError::Invalid));
    assert!(a.login_with_code("ip5", "admin", PW, &code(2060), 2060).is_ok(), "sonraki adımın kodu geçer");
    // Kodu olmayan kullanıcı etkilenmez.
    assert!(a.login("ip", "izleyici", "izleyici-parolasi-1", 2000).is_ok());

    f.disable_totp("admin").unwrap();
    assert!(Auth::new(f, None, false, Cost::Test).login("ip", "admin", PW, 2000).is_ok());
}

#[test]
fn wrong_codes_count_toward_lockout() {
    let mut f = users();
    f.begin_totp("admin").unwrap();
    let secret = f.find("admin").unwrap().totp_pending.clone().unwrap();
    let code = format!("{:06}", crate::totp::code_at(&crate::totp::base32_decode(&secret).unwrap(), 0));
    f.confirm_totp("admin", &code, 0).unwrap();
    let a = Auth::new(f, None, false, Cost::Test);
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(a.login_with_code("ip", "admin", PW, "000001", 90_000), Err(LoginError::Invalid));
    }
    assert_eq!(a.login_with_code("ip", "admin", PW, "000001", 90_000), Err(LoginError::Locked(LOCK_SECS)));
}

#[test]
fn roles_and_edits() {
    let mut f = users();
    assert!(f.set_role("admin", Role::Viewer).unwrap_err().contains("son yönetici"));
    f.set_role("izleyici", Role::Admin).unwrap();
    f.set_role("admin", Role::Viewer).unwrap();
    assert!(f.set_role("yok", Role::Admin).is_err());

    let a = auth();
    assert!(a.check_password("admin", PW) && !a.check_password("admin", "x") && !a.check_password("yok", PW));
    // Başarısız düzenleme hiçbir şeyi değiştirmez.
    let err = a.edit(|f| { f.remove_user("izleyici")?; f.remove_user("admin") }).unwrap_err();
    assert!(err.contains("son yönetici"));
    assert_eq!(a.list().len(), 2);
    a.edit(|f| f.add_user("yeni", "yeni-kullanici-parolasi", Role::Viewer, Cost::Test)).unwrap();
    assert_eq!(a.list().len(), 3);
    assert!(a.login("ip", "yeni", "yeni-kullanici-parolasi", 0).is_ok());
}

#[test]
fn edits_are_written_to_the_user_file() {
    let dir = std::env::temp_dir().join(format!("redox-auth-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("users.json");
    users().save(&path).unwrap();
    let a = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
    let t = a.login("ip", "izleyici", "izleyici-parolasi-1", 0).unwrap();

    a.edit(|f| f.set_role("izleyici", Role::Admin)).unwrap();
    assert_eq!(UserFile::load(&path).unwrap().find("izleyici").unwrap().role, Role::Admin);
    assert_eq!(a.session(&t, 1).unwrap().role, Role::Admin, "rol değişikliği açık oturuma hemen yansır");
    a.edit(|f| f.remove_user("izleyici")).unwrap();
    assert!(a.session(&t, 2).is_none(), "silinen kullanıcının oturumu düşer");

    a.audit.record(5, "admin", "ip", "deneme", true);
    assert!(dir.join("audit.log").is_file(), "denetim günlüğü kullanıcı dosyasının yanına yazılır");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn read_only_tokens_act_as_viewers_whatever_the_owner_is() {
    let mut file = users();
    let full = file.create_token("admin", "tam").unwrap();
    let limited = file.create_token_as("admin", "izleme", true).unwrap();
    let a = Auth::new(file, None, false, Cost::Test);
    assert_eq!(a.bearer(&full).unwrap().role, Role::Admin);
    let me = a.bearer(&limited).unwrap();
    assert_eq!((me.user.as_str(), me.role, me.can_write()), ("admin", Role::Viewer, false));
    // Eski kullanıcı dosyalarındaki token'lar (alan yok) tam yetkilidir.
    let old: ApiToken = serde_json::from_str(r#"{"name":"eski","sha256":"00"}"#).unwrap();
    assert!(!old.read_only);
}

/// Aynı anda yollanan denemeler sınırı birlikte aşamaz: sayaç, parola doğrulanmadan önce artar.
#[test]
fn a_burst_of_guesses_gets_no_more_tries_than_the_limit() {
    let auth = std::sync::Arc::new(auth());
    let workers: Vec<_> = (0..40)
        .map(|n| {
            let auth = auth.clone();
            std::thread::spawn(move || auth.login("10.0.0.9", "admin", &format!("yanlis-tahmin-{n:04}"), 1_000))
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let checked = results.iter().filter(|r| matches!(r, Err(LoginError::Invalid))).count();
    assert!(checked < MAX_FAILURES as usize, "{checked} tahmin denetlendi");
    assert!(matches!(auth.login("10.0.0.9", "admin", PW, 1_001), Err(LoginError::Locked(_))), "istemci kilitlendi");
    // Kilit istemciye özeldir; süresi dolunca kalkar.
    assert!(auth.login("10.0.0.10", "admin", PW, 1_001).is_ok());
    assert!(auth.login("10.0.0.9", "admin", PW, 1_001 + LOCK_SECS).is_ok());
}

/// IPv6'da tek makine ağındaki (/64) bütün adresleri kullanabilir: sayaç ağa göre tutulur.
#[test]
fn ipv6_clients_are_counted_per_network() {
    let auth = auth();
    for n in 1..=MAX_FAILURES {
        let _ = auth.login(&format!("2001:db8:1:2::{n:x}"), "admin", "yanlis-parola-123", 1_000);
    }
    assert!(matches!(auth.login("2001:db8:1:2:ffff::1", "admin", PW, 1_001), Err(LoginError::Locked(_))));
    assert!(auth.login("2001:db8:1:3::1", "admin", PW, 1_001).is_ok(), "başka ağ etkilenmez");
    assert_eq!(attempt_key("::ffff:10.0.0.1", "Admin"), "::ffff:10.0.0.1|admin", "IPv4 eşlemeli adres tek başına sayılır");
    assert!(attempt_key("ip", &"x".repeat(500)).len() < 80, "uzun ad anahtarı büyütmez");
}

/// Doğru parolanın ardından gelen güvenlik anahtarı adımı deneme hakkından yemez; hatalı parola sayılır.
#[test]
fn confirm_password_counts_like_a_login() {
    let auth = auth();
    assert_eq!(auth.confirm_password("admin", PW, 1_000), Ok(true));
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(auth.confirm_password("admin", "yanlis-parola-123", 1_000), Ok(false));
    }
    assert_eq!(auth.confirm_password("admin", "yanlis-parola-123", 1_000), Err(LOCK_SECS));
    assert!(auth.confirm_password("admin", PW, 1_001).is_err(), "kilitliyken doğru parola da geçmez");
    assert_eq!(auth.confirm_password("izleyici", "izleyici-parolasi-1", 1_001), Ok(true), "başka hesap etkilenmez");
    assert_eq!(auth.confirm_password("admin", PW, 1_000 + LOCK_SECS), Ok(true));
    // Giriş sayacı ayrıdır: hesap formlarındaki kilit girişi kapatmaz.
    assert!(auth.login("ip", "admin", PW, 1_001).is_ok());
}

#[test]
fn an_active_second_step_is_not_replaced_by_a_new_setup() {
    let mut f = users();
    let secret = f.begin_totp("admin").unwrap();
    let key = crate::totp::base32_decode(&secret).unwrap();
    let code = format!("{:06}", crate::totp::code_at(&key, 1_000 / crate::totp::STEP_SECS));
    f.confirm_totp("admin", &code, 1_000).unwrap();
    assert!(f.begin_totp("admin").unwrap_err().contains("zaten açık"));
    assert_eq!(f.find("admin").unwrap().totp_secret.as_deref(), Some(secret.as_str()));
    assert!(f.find("admin").unwrap().totp_pending.is_none());
}
