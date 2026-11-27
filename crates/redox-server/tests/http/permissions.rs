//! Yetki tablosu: sunucunun **bütün** adresleri her kimlikle denenir. Adresler kaynak dosyalardan okunur;
//! böylece yeni eklenen bir adres de kendiliğinden denetlenir. İzleyicinin okuyabildiği adresler aşağıda tek
//! tek yazılıdır: listede olmayan yeni bir okuma adresi izleyiciye açılırsa test kırılır.

use super::*;

/// Oturum olmadan da açılan adresler.
const PUBLIC: [&str; 9] = ["/tls-ask", "/login", "/login/key", "/login/oidc", "/login/oidc/callback", "/healthz", "/lang", "/assets/{file}", "/assets/fonts/{file}"];

/// İzleyicinin (ve salt okunur token'ın) okuyabildiği adresler. Yedeklerin içeriği, konuk diskleri, ekran,
/// konsol, kullanıcılar, günlük ve destek paketi burada **olamaz**.
const READABLE: &[&str] = &[
    "/", "/nodes/{id}", "/guests", "/tasks", "/backups", "/guests/new", "/guests/{vmid}", "/guests/{vmid}/delete",
    "/guests/{vmid}/edit", "/guests/{vmid}/confirm", "/backups/{id}/{op}", "/backups/{id}/files", "/storage", "/pools",
    "/cluster", "/help", "/theme", "/usage.csv", "/metrics",
    "/api/v1/openapi.json", "/api/v1/cluster", "/api/v1/nodes", "/api/v1/nodes/{id}", "/api/v1/guests",
    "/api/v1/guests/{vmid}", "/api/v1/guests/{vmid}/state", "/api/v1/guests/{vmid}/usage",
    "/api/v1/guests/{vmid}/filesystems", "/api/v1/usage", "/api/v1/usage.csv", "/api/v1/backups",
    "/api/v1/backups/report", "/api/v1/backups/archive", "/api/v1/backups/forecast", "/api/v1/backups/{id}/files",
    "/api/v1/approvals", "/api/v1/maintenance", "/api/v1/lockdown", "/api/v1/replicas", "/api/v1/storage",
    "/api/v1/media", "/api/v1/media/catalog", "/api/v1/peers", "/api/v1/profile", "/api/v1/balance",
    "/api/v1/ft/protections", "/api/v1/ft/standby", "/api/v1/ft/arbitrate/{key}", "/api/v1/mirrors",
    "/api/v1/mirror-stores", "/api/v1/vswitches", "/api/v1/host/performance", "/api/v1/host/networks",
    "/api/v1/host/health", "/api/v1/firewall/sets", "/api/v1/firewall/groups", "/api/v1/shared",
    "/api/v1/shared/offer/{vmid}", "/api/v1/pools", "/api/v1/jobs", "/api/v1/tasks", "/api/v1/tasks/{id}",
];

/// Adresi `READABLE`'da olan ama yedeğin içeriğine giden yollar: izleyiciye içerik verilmediği ayrıca sınanır.
const CONTENT_BEHIND_HANDLER: [&str; 3] = ["/backups/{id}/files", "/backups/{id}/{op}", "/api/v1/backups/{id}/files"];

/// Kendi kapsamındaki operatörün yazabildiği adresler.
const OPERABLE: &[&str] = &[
    "/guests/{vmid}/action", "/guests/{vmid}/suspend", "/guests/{vmid}/snapshots", "/guests/{vmid}/backups",
    "/guests/{vmid}/keys", "/guests/{vmid}/exec", "/api/v1/guests/{vmid}/suspend", "/api/v1/guests/{vmid}/snapshots",
    "/api/v1/guests/{vmid}/backups", "/api/v1/guests/{vmid}/exec", "/api/v1/guests/{vmid}/{action}",
];

fn routes() -> Vec<(Method, String)> {
    let mut out = Vec::new();
    for (file, prefix) in [("src/lib.rs", ""), ("src/api.rs", "/api/v1")] {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        let mut pieces = text.split(".route(");
        pieces.next();
        for piece in pieces {
            let piece = piece.trim_start().strip_prefix('"').expect("adres sabit metin olmalı");
            let (path, rest) = piece.split_once('"').unwrap();
            let handlers = rest.lines().next().unwrap_or_default();
            let mut methods = Vec::new();
            for (marker, method) in [("get(", Method::GET), ("post(", Method::POST), ("delete_route(", Method::DELETE), (".delete(", Method::DELETE), ("patch(", Method::PATCH), ("::put(", Method::PUT)] {
                // `x_put)` gibi işleyici adları yöntem değildir.
                if handlers.match_indices(marker).any(|(at, _)| !handlers[..at].ends_with('_')) {
                    methods.push(method);
                }
            }
            assert!(!methods.is_empty(), "yöntemi okunamayan adres: {path}");
            out.extend(methods.into_iter().map(|m| (m, format!("{prefix}{path}"))));
        }
    }
    assert!(out.len() > 220, "adresler okunamadı: {}", out.len());
    out
}

fn concrete(template: &str, backup: &str) -> String {
    let id = if template.contains("/backups/") { backup } else if template.contains("/nodes/") { "pve-ist-01" } else { "1" };
    let mut path = template.to_string();
    for (name, value) in [("{vmid}", "100"), ("{id}", id), ("{name}", "x"), ("{op}", "download"), ("{index}", "0"), ("{action}", "start"), ("{file}", "style.css"), ("{peer}", "x"), ("{key}", "x"), ("{label}", "x"), ("{storage}", "local"), ("{origin}", "x")] {
        path = path.replace(name, value);
    }
    path
}

struct World {
    app: Router,
    state: AppState,
    backup: String,
    /// Yöneticinin salt okunur token'ı.
    reading: String,
}

async fn world() -> World {
    let (app, state) = setup_real();
    let reading = state
        .auth()
        .edit(|u| {
            u.add_user("kapsamdisi", "kapsam-parolasi-12", Role::Operator, Cost::Test)?;
            u.set_scope("kapsamdisi", vec!["olmayan-etiket".into()])?;
            u.add_token("kapsamdisi", "t", "redox_op_disi")?;
            u.add_user("kapsamli", "kapsam-parolasi-12", Role::Operator, Cost::Test)?;
            u.add_token("kapsamli", "t", "redox_op_ici")?;
            u.create_token_as("admin", "salt-okunur", true)
        })
        .unwrap();
    assert!(post_ok(&app, "/guests/103/backups", "").await.contains("notice="));
    let backup = state.snapshot().backups.first().expect("yedek oluşmalı").id.clone();
    World { app, state, backup, reading }
}

async fn status_of(w: &World, method: &Method, path: &str, token: Option<&str>) -> StatusCode {
    let mut builder = anon(method.clone(), path).header(CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    w.app.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap().status()
}

#[tokio::test]
async fn without_signing_in_only_the_listed_addresses_answer() {
    let w = world().await;
    let before = format!("{:?}", w.state.snapshot());
    for (method, template) in routes() {
        let status = status_of(&w, &method, &concrete(&template, &w.backup), None).await;
        if !PUBLIC.contains(&template.as_str()) {
            // Sayfalar giriş sayfasına yönlendirir, API ve ölçümler 401 verir.
            assert!(status == StatusCode::SEE_OTHER || status == StatusCode::UNAUTHORIZED, "{method} {template}: {status}");
        }
    }
    assert_eq!(format!("{:?}", w.state.snapshot()), before);
}

#[tokio::test]
async fn read_only_identities_change_nothing_and_read_only_what_is_listed() {
    let w = world().await;
    let (before, users) = (format!("{:?}", w.state.snapshot()), w.state.auth().list());
    let mut leaked = Vec::new();
    for token in [VIEWER_TOKEN, w.reading.as_str(), "redox_op_disi"] {
        for (method, template) in routes() {
            if PUBLIC.contains(&template.as_str()) || template == "/logout" {
                continue;
            }
            let status = status_of(&w, &method, &concrete(&template, &w.backup), Some(token)).await;
            let listed = method == Method::GET && READABLE.contains(&template.as_str());
            if !listed && status != StatusCode::FORBIDDEN {
                leaked.push(format!("{method} {template}: {status}"));
            }
        }
    }
    leaked.sort();
    leaked.dedup();
    assert!(leaked.is_empty(), "izin verilmemesi gerekenler:\n{}", leaked.join("\n"));
    assert_eq!(format!("{:?}", w.state.snapshot()), before, "küme değişmedi");
    assert_eq!(w.state.auth().list(), users, "kullanıcılar değişmedi");
    // Adresi açık görünen yedek içeriği işleyicide kapalıdır.
    for template in CONTENT_BEHIND_HANDLER {
        let path = format!("{}?path=%2Fetc", concrete(template, &w.backup));
        let (status, _, body) = send(&w.app, as_viewer(Method::GET, &path).body(Body::empty()).unwrap()).await;
        assert!(!status.is_success() || body.contains("yalnızca yönetici"), "{path}: {status} {body}");
    }
}

#[tokio::test]
async fn an_operator_in_scope_writes_only_to_the_listed_guest_operations() {
    let w = world().await;
    let mut wrong = Vec::new();
    for (method, template) in routes() {
        if method == Method::GET || PUBLIC.contains(&template.as_str()) || template == "/logout" {
            continue;
        }
        let status = status_of(&w, &method, &concrete(&template, &w.backup), Some("redox_op_ici")).await;
        let allowed = method == Method::POST && OPERABLE.contains(&template.as_str());
        if allowed == (status == StatusCode::FORBIDDEN) {
            wrong.push(format!("{method} {template}: {status} (izinli: {allowed})"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Genel eylem adresi yalnızca güç işlemlerine açıktır.
    for action in ["delete", "clone", "resize", "migrate", "x"] {
        let status = status_of(&w, &Method::POST, &format!("/api/v1/guests/100/{action}"), Some("redox_op_ici")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{action}");
    }
}

#[tokio::test]
async fn in_lockdown_even_an_administrator_changes_only_the_exempt_addresses() {
    let w = world().await;
    w.state.lockdown_set(true, "admin");
    for (method, template) in routes() {
        let path = concrete(&template, &w.backup);
        if method == Method::GET || PUBLIC.contains(&template.as_str()) || redox_server::lockdown_exempt(&path) {
            continue;
        }
        assert_eq!(status_of(&w, &method, &path, Some(ADMIN_TOKEN)).await, StatusCode::LOCKED, "{method} {template}");
    }
}

#[tokio::test]
async fn redirect_targets_cannot_leave_the_site() {
    let (app, _) = setup();
    // Tarayıcı sekmeyi ve satır sonunu siler: "/<sekme>/kotu.example" bir başka site olurdu.
    for bad in ["%2F%09%2Fkotu.example", "%2F%09%09%2Fkotu.example", "%2F%0B%2Fkotu.example", "%2F%0A%2Fkotu.example", "%2F%0D%2Fkotu.example", "%2F%2Fkotu.example", "%2F%5Ckotu.example", "https%3A%2F%2Fkotu.example"] {
        let (_, headers, _) = send(&app, login_request("admin", ADMIN_PW, bad)).await;
        assert_eq!(location(&headers), "/", "giriş: {bad}");
        let (_, headers, _) = send(&app, anon(Method::GET, &format!("/lang?to=en&back={bad}")).body(Body::empty()).unwrap()).await;
        assert_eq!(location(&headers), "/", "dil: {bad}");
    }
    let (_, headers, _) = send(&app, login_request("admin", ADMIN_PW, "%2Fguests%3Fkind%3Dct")).await;
    assert_eq!(location(&headers), "/guests?kind=ct");
}
