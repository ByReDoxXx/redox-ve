//! Çalışan konuktan çekirdek ve bellek çıkarma: kayıt her zaman konukta gerçekten takılı olanı gösterir.

use super::*;

use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Machine {
    cores: u32,
    dimms: usize,
    /// Konuk kaynağı bırakmıyor.
    stubborn: bool,
    released: Vec<usize>,
}

#[derive(Clone, Default)]
struct Plugs(Arc<Mutex<Machine>>);

impl Hypervisor for Plugs {
    fn name(&self) -> &'static str {
        "tak-cikar"
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, _: GuestKind) -> bool {
        true
    }

    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }

    fn hotplug(&self, _: GuestKind) -> bool {
        true
    }

    fn hot_add_core(&self, _: &Guest) -> Result<(), String> {
        self.0.lock().unwrap().cores += 1;
        Ok(())
    }

    fn hot_add_memory(&self, _: &Guest, _: u64) -> Result<(), String> {
        self.0.lock().unwrap().dimms += 1;
        Ok(())
    }

    fn hot_remove_core(&self, _: &Guest) -> Result<(), String> {
        let mut m = self.0.lock().unwrap();
        if m.stubborn {
            return Err("konuk çekirdeği bırakmadı".into());
        }
        m.cores -= 1;
        Ok(())
    }

    fn hot_remove_memory(&self, _: &Guest) -> Result<(), String> {
        let mut m = self.0.lock().unwrap();
        if m.stubborn {
            return Err("konuk belleği bırakmadı".into());
        }
        m.dimms -= 1;
        Ok(())
    }

    fn hotplug_inventory(&self, _: &Guest) -> Option<(u32, usize)> {
        let m = self.0.lock().unwrap();
        Some((m.cores, m.dimms))
    }

    fn release_dimm(&self, _: &Guest, index: usize) {
        self.0.lock().unwrap().released.push(index);
    }
}

const VM: u32 = 100;

async fn world() -> (Router, AppState, Plugs) {
    let plugs = Plugs::default();
    let mut cluster = Cluster::demo(now());
    {
        let g = cluster.guests.iter_mut().find(|g| g.vmid == VM).unwrap();
        (g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb) = (2, 1024, 6, 8192);
    }
    plugs.0.lock().unwrap().cores = 2;
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(plugs.clone()), Duration::ZERO, None, auth);
    (app(state.clone()), state, plugs)
}

fn shape(state: &AppState) -> (u32, u64, Vec<u64>) {
    let g = state.snapshot().guest(VM).unwrap().clone();
    (g.cores, g.mem_total_mb, g.hw.dimms)
}

async fn change(app: &Router, what: &str, value: u64) -> String {
    let field = if what == "cores" { "cores" } else { "memory_mb" };
    post_ok(app, &format!("/guests/{VM}/{what}"), &format!("{field}={value}")).await
}

#[tokio::test]
async fn the_same_forms_remove_when_the_number_goes_down() {
    let (app, state, plugs) = world().await;
    assert!(change(&app, "cores", 4).await.contains("notice="));
    for mb in [1536, 1792, 2816] {
        assert!(change(&app, "memory", mb).await.contains("notice="), "{mb}");
    }
    assert_eq!(shape(&state), (4, 2816, vec![512, 256, 1024]));

    let done = change(&app, "cores", 1).await;
    assert!(done.contains("notice=") && done.contains("indirildi"), "{done}");
    assert_eq!((shape(&state).0, plugs.0.lock().unwrap().cores), (1, 1));
    assert!(change(&app, "cores", 0).await.contains("error="));

    // Ara bir değer parça sınırına denk gelmiyorsa inilebilecek değerler söylenir.
    let refused = change(&app, "memory", 2000).await;
    assert!(refused.contains("error=") && refused.contains("1792"), "{refused}");
    assert_eq!(shape(&state).1, 2816);
    assert!(change(&app, "memory", 1536).await.contains("notice="), "iki parça birden");
    assert_eq!(shape(&state), (1, 1536, vec![512]));
    // Açılış belleği çıkarılamaz.
    assert!(change(&app, "memory", 512).await.contains("error="));
    let tab = page(&app, &format!("/guests/{VM}?tab=donanim")).await;
    assert!(tab.contains("memhp_default_state=online_movable"));
}

#[tokio::test]
async fn a_refused_removal_changes_nothing_and_a_late_release_is_recorded() {
    let (app, state, plugs) = world().await;
    change(&app, "cores", 3).await;
    change(&app, "memory", 1536).await;
    change(&app, "memory", 2048).await;
    plugs.0.lock().unwrap().stubborn = true;
    assert!(change(&app, "memory", 1536).await.contains("error="));
    assert!(change(&app, "cores", 2).await.contains("error="));
    assert_eq!(shape(&state), (3, 2048, vec![512, 512]));
    state.refresh(None, None);
    assert_eq!(shape(&state), (3, 2048, vec![512, 512]), "konuk bırakmadıkça kayıt değişmez");

    // Konuk süre dolduktan sonra parçayı ve çekirdeği bırakır: izleme turu kaydı düzeltir.
    {
        let mut m = plugs.0.lock().unwrap();
        (m.dimms, m.cores) = (1, 2);
    }
    state.refresh(None, None);
    assert_eq!(shape(&state), (2, 1536, vec![512]));
    assert_eq!(plugs.0.lock().unwrap().released, [1], "bırakılan parçanın kaynağı da temizlenir");
    assert!(state.snapshot().tasks.iter().any(|t| t.command.contains("konuk sonradan bıraktı")));
    state.refresh(None, None);
    assert_eq!(plugs.0.lock().unwrap().released, [1], "bir kez");
}

#[tokio::test]
async fn removal_needs_a_running_vm_and_no_balloon_target() {
    let (_, state, _) = world().await;
    state.cores_add(VM, 3, "admin").unwrap();
    state.memory_add(VM, 1536, "admin").unwrap();
    assert!(state.cores_add(103, 1, "admin").is_err(), "kapalı konuk");
    assert!(state.memory_add(200, 128, "admin").is_err(), "konteyner");
    let mut cluster = state.snapshot();
    cluster.guests.iter_mut().find(|g| g.vmid == VM).unwrap().hw.balloon_mb = 1200;
    assert!(cluster.check_memory_remove(VM, 1024).unwrap_err().to_string().contains("balon hedefini"));
    cluster.guests.iter_mut().find(|g| g.vmid == VM).unwrap().hw.balloon_mb = 0;
    cluster.guests.iter_mut().find(|g| g.vmid == VM).unwrap().hw.mem_reserve_mb = 1200;
    assert!(cluster.check_memory_remove(VM, 1024).unwrap_err().to_string().contains("garantisinin"));
    assert_eq!(cluster.check_core_remove(VM, 2).unwrap(), 3);
}
