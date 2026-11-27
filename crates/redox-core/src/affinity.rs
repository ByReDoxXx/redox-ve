//! Yerleştirme kuralları, konuk etiketleriyle verilir:
//!
//! - `ayri-<grup>`: aynı gruptaki konuklar aynı sunucuda çalışmamalı (örn. iki veritabanı kopyası).
//! - `birlikte-<grup>`: aynı gruptaki konuklar aynı sunucuda kalmalı.
//! - `sabit`: konuk yük dengeleme için taşınmaz.
//!
//! Sunucular bağımsız olduğundan kurallar konuk kimliğine değil etikete dayanır.

use crate::{Guest, GuestStatus};

pub const PINNED_TAG: &str = "sabit";
const APART: &str = "ayri-";
const TOGETHER: &str = "birlikte-";

fn groups<'a>(g: &'a Guest, prefix: &'a str) -> impl Iterator<Item = &'a str> {
    g.tags.iter().filter(move |t| t.len() > prefix.len() && t.starts_with(prefix)).map(String::as_str)
}

/// `guest`, `others` içinde çalışan bir konukla ayrı tutulmalı mı? Tutulmalıysa ortak etiket.
pub fn apart_conflict<'a>(guest: &'a Guest, others: impl IntoIterator<Item = &'a Guest>) -> Option<&'a str> {
    let running: Vec<&Guest> = others.into_iter().filter(|o| o.status == GuestStatus::Running).collect();
    let found = groups(guest, APART).find(|tag| running.iter().any(|o| o.tags.iter().any(|t| t == tag)));
    found
}

/// Bu konuk taşınırsa aynı sunucudaki bir "birlikte" eşinden ayrılır mı?
pub fn bound_here<'a>(guest: &'a Guest, local: &'a [Guest]) -> Option<&'a str> {
    let found = groups(guest, TOGETHER).find(|tag| local.iter().any(|o| o.vmid != guest.vmid && o.status == GuestStatus::Running && o.tags.iter().any(|t| t == tag)));
    found
}

pub fn pinned(guest: &Guest) -> bool {
    guest.tags.iter().any(|t| t == PINNED_TAG)
}

/// Aynı sunucuda birlikte çalışan "ayrı" grup üyeleri: `(etiket, konuk kimlikleri)`.
pub fn apart_violations(local: &[Guest]) -> Vec<(String, Vec<crate::VmId>)> {
    let mut found: Vec<(String, Vec<crate::VmId>)> = Vec::new();
    for g in local.iter().filter(|g| g.status == GuestStatus::Running) {
        for tag in groups(g, APART) {
            match found.iter_mut().find(|(t, _)| t == tag) {
                Some((_, members)) => members.push(g.vmid),
                None => found.push((tag.to_string(), vec![g.vmid])),
            }
        }
    }
    found.retain(|(_, members)| members.len() > 1);
    found.sort();
    found
}

/// Bu sunucuda üyesi olup bir başka üyesi eşte çalışan "birlikte" grupları: `(etiket, eş adı)`.
pub fn together_violations<'a>(local: &[Guest], peers: impl IntoIterator<Item = (&'a str, &'a [Guest])>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (peer, guests) in peers {
        for g in local.iter().filter(|g| g.status == GuestStatus::Running) {
            for tag in groups(g, TOGETHER) {
                let entry = (tag.to_string(), peer.to_string());
                if !found.contains(&entry) && guests.iter().any(|o| o.status == GuestStatus::Running && o.tags.iter().any(|t| t == tag)) {
                    found.push(entry);
                }
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cluster;

    fn guest(vmid: u32, status: GuestStatus, tags: &[&str]) -> Guest {
        let mut g = Cluster::demo(0).guests[0].clone();
        (g.vmid, g.status) = (vmid, status);
        g.tags = tags.iter().map(|t| t.to_string()).collect();
        g
    }

    #[test]
    fn tags_express_apart_together_and_pinned() {
        use GuestStatus::{Running, Stopped};
        let db1 = guest(1, Running, &["ayri-db", "birlikte-web"]);
        let db2 = guest(2, Running, &["ayri-db"]);
        let web = guest(3, Running, &["birlikte-web", "sabit"]);
        let off = guest(4, Stopped, &["ayri-db"]);
        assert_eq!(apart_conflict(&db1, [&db2]), Some("ayri-db"));
        assert_eq!(apart_conflict(&db1, [&off, &web]), None, "kapalı konuk çakışma sayılmaz");
        let local = [db1.clone(), db2, web.clone(), off];
        assert_eq!(apart_violations(&local), [("ayri-db".to_string(), vec![1, 2])]);
        assert_eq!(bound_here(&db1, &local), Some("birlikte-web"));
        assert!(bound_here(&local[1], &local).is_none() && pinned(&web) && !pinned(&db1));
        let far = [guest(9, Running, &["birlikte-web"])];
        assert_eq!(together_violations(&local, [("ankara", &far[..])]), [("birlikte-web".to_string(), "ankara".to_string())]);
        assert!(apart_violations(&[guest(1, Running, &["ayri-"]), guest(2, Running, &["ayri-"])]).is_empty(), "grup adı boş olamaz");
    }
}
