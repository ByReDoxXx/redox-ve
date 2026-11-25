//! CSV dışa aktarımı: kaynak ölçümü ve denetim günlüğü (faturalama ve inceleme için).

use redox_core::Cluster;

use crate::audit::AuditEntry;

/// Bir CSV hücresi. Virgül, tırnak ya da satır sonu içeren değer tırnaklanır. `=`, `+`, `-`, `@` ile başlayan
/// metin hücreleri tablolama programlarında formül olarak çalışmasın diye başına tek tırnak konur (konuk adı ve
/// istek yolu gibi alanları kullanıcı belirler).
pub fn cell(value: &str) -> String {
    let guarded = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{value}") } else { value.to_string() };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn row(cells: &[String]) -> String {
    let mut line = cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",");
    line.push_str("\r\n");
    line
}

/// Bütün konukların birikimli tüketimi. Sütun adları dilden bağımsızdır (betiklerle okunur).
pub fn usage(c: &Cluster) -> String {
    let mut out = row(&["vmid", "name", "kind", "pool", "since", "uptime_secs", "cpu_core_hours", "mem_gb_hours", "disk_read_mb", "disk_write_mb", "net_rx_mb", "net_tx_mb"].map(String::from));
    for g in &c.guests {
        let u = c.usage_of(g.vmid);
        out.push_str(&row(&[
            g.vmid.to_string(),
            g.name.clone(),
            g.kind.to_string(),
            g.pool.clone().unwrap_or_default(),
            u.since.to_string(),
            u.uptime_secs.to_string(),
            format!("{:.3}", u.cpu_core_hours()),
            format!("{:.3}", u.mem_gb_hours()),
            (u.disk_read_bytes >> 20).to_string(),
            (u.disk_write_bytes >> 20).to_string(),
            (u.net_rx_bytes >> 20).to_string(),
            (u.net_tx_bytes >> 20).to_string(),
        ]));
    }
    out
}

/// Denetim günlüğü kayıtları (en yeni başta, verildiği sırayla).
pub fn audit(entries: &[AuditEntry]) -> String {
    let mut out = row(&["time", "user", "address", "action", "ok"].map(String::from));
    for e in entries {
        out.push_str(&row(&[e.ts.to_string(), e.user.clone(), e.ip.clone(), e.action.clone(), e.ok.to_string()]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_quoted_and_cannot_become_formulas() {
        assert_eq!(cell("web-01"), "web-01");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("=HYPERLINK(\"http://x\")"), "\"'=HYPERLINK(\"\"http://x\"\")\"");
        assert_eq!(cell("-1"), "'-1");
        assert_eq!(cell("@cmd"), "'@cmd");
    }

    #[test]
    fn usage_has_one_row_per_guest_and_audit_one_per_entry() {
        let mut c = Cluster::demo(1_700_000_000);
        c.record_usage(100, Some((5 << 20, 7 << 20)), 1_700_000_000);
        let text = usage(&c);
        let lines: Vec<&str> = text.trim_end().split("\r\n").collect();
        assert_eq!(lines.len(), c.guests.len() + 1);
        assert!(lines[0].starts_with("vmid,name,kind,pool,since,uptime_secs,cpu_core_hours"));
        assert!(lines[1].starts_with("100,") && lines[1].ends_with(",5,7,0,0"), "{}", lines[1]);
        let log = audit(&[AuditEntry { ts: 5, user: "admin".into(), ip: "10.0.0.1".into(), action: "POST /guests/103/action".into(), ok: true }]);
        assert_eq!(log, "time,user,address,action,ok\r\n5,admin,10.0.0.1,POST /guests/103/action,true\r\n");
    }
}
