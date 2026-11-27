//! İmajın çalışma ayarı (program, ortam, dizin, kullanıcı) ve bunun LXC ayar satırları.
//!
//! İmajdan gelen her değer güvenilmezdir: satır sonu ya da NUL içeren hiçbir değer ayar dosyasına yazılmaz,
//! yoksa imaj dosyaya kendi LXC satırlarını (örn. sunucuda çalışan bir kanca) ekleyebilirdi.

use serde_json::Value;

const MAX_ITEMS: usize = 256;
const MAX_VALUE: usize = 8192;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageConfig {
    /// Giriş noktası ve komut birlikte.
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub cwd: String,
    /// `ad`, `uid`, `ad:grup` ya da `uid:gid`; boşsa kök.
    pub user: String,
    pub stop_signal: String,
}

fn safe(value: &str) -> Result<&str, String> {
    if value.len() > MAX_VALUE || value.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err("imaj ayarında satır sonu içeren ya da çok uzun bir değer var; imaj reddedildi".to_string());
    }
    Ok(value)
}

fn list(value: &Value) -> Result<Vec<String>, String> {
    let Some(items) = value.as_array() else { return Ok(Vec::new()) };
    if items.len() > MAX_ITEMS {
        return Err("imaj ayarı çok büyük".to_string());
    }
    items.iter().map(|v| v.as_str().ok_or_else(|| "imaj ayarı anlaşılamadı".to_string()).and_then(|s| safe(s).map(str::to_string))).collect()
}

impl ImageConfig {
    pub fn parse(document: &Value) -> Result<ImageConfig, String> {
        let config = &document["config"];
        let text = |name: &str| safe(config[name].as_str().unwrap_or_default()).map(str::to_string);
        let mut args = list(&config["Entrypoint"])?;
        args.extend(list(&config["Cmd"])?);
        if args.is_empty() {
            return Err("imajda çalıştırılacak program tanımlı değil".to_string());
        }
        let env = list(&config["Env"])?;
        let valid_name = |name: &str| !name.is_empty() && !name.bytes().next().is_some_and(|b| b.is_ascii_digit()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-');
        if !env.iter().all(|e| e.split_once('=').is_some_and(|(name, _)| valid_name(name))) {
            return Err("imajda geçersiz bir ortam değişkeni var".to_string());
        }
        let cwd = text("WorkingDir")?;
        if !cwd.is_empty() && !cwd.starts_with('/') {
            return Err("imajın çalışma dizini mutlak bir yol değil".to_string());
        }
        let signal = text("StopSignal")?;
        if !signal.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+') {
            return Err("imajın durdurma sinyali geçersiz".to_string());
        }
        Ok(ImageConfig { args, env, cwd, user: text("User")?, stop_signal: signal })
    }

    /// Kullanıcı ve grup numaraları. Adlar imajın kendi `passwd` ve `group` dosyalarından çözülür.
    pub fn ids(&self, passwd: &str, group: &str) -> Result<(u32, u32), String> {
        if self.user.is_empty() {
            return Ok((0, 0));
        }
        let (user, wanted_group) = match self.user.split_once(':') {
            Some((u, g)) => (u, Some(g)),
            None => (self.user.as_str(), None),
        };
        let fields = |text: &str, key: &str| text.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.len() > 3 && (f[0] == key || f[2] == key)).map(|f| (f[2].parse::<u32>().ok(), f[3].parse::<u32>().ok()));
        let (uid, primary) = match (user.parse::<u32>(), fields(passwd, user)) {
            (_, Some((Some(uid), gid))) => (uid, gid),
            (Ok(uid), _) => (uid, None),
            _ => return Err(format!("imajın kullanıcısı bulunamadı: {user}")),
        };
        let gid = match wanted_group {
            None => primary.unwrap_or(0),
            Some(name) => match name.parse::<u32>() {
                Ok(gid) => gid,
                Err(_) => group.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.len() > 2 && f[0] == name).and_then(|f| f[2].parse().ok()).ok_or_else(|| format!("imajın grubu bulunamadı: {name}"))?,
            },
        };
        Ok((uid, gid))
    }

    /// `lxc.init.cmd` sözcükleri: LXC satırı boşluklardan böler; boşluklu sözcük tırnaklanır.
    fn command(&self) -> Result<String, String> {
        let word = |arg: &String| {
            let plain = !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'');
            if plain {
                Ok(arg.clone())
            } else if !arg.contains('"') {
                Ok(format!("\"{arg}\""))
            } else if !arg.contains('\'') {
                Ok(format!("'{arg}'"))
            } else {
                Err("imajın komutunda hem tek hem çift tırnak içeren bir sözcük var; LXC bunu taşıyamıyor".to_string())
            }
        };
        Ok(self.args.iter().map(word).collect::<Result<Vec<_>, _>>()?.join(" "))
    }

    /// Konteyner ayar dosyasına bir kez yazılan satırlar.
    pub fn lxc_lines(&self, uid: u32, gid: u32) -> Result<String, String> {
        let mut lines = vec![format!("lxc.init.cmd = {}", self.command()?), format!("lxc.init.uid = {uid}"), format!("lxc.init.gid = {gid}")];
        if !self.cwd.is_empty() {
            lines.push(format!("lxc.init.cwd = {}", self.cwd));
        }
        lines.extend(self.env.iter().map(|e| format!("lxc.environment = {e}")));
        if !self.stop_signal.is_empty() {
            lines.push(format!("lxc.signal.halt = {}", self.stop_signal));
        }
        Ok(lines.join("\n") + "\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(config: Value) -> Result<ImageConfig, String> {
        ImageConfig::parse(&json!({ "config": config }))
    }

    #[test]
    fn entrypoint_command_environment_and_user_become_lxc_lines() {
        let c = image(json!({ "Entrypoint": ["/docker-entrypoint.sh"], "Cmd": ["nginx", "-g", "daemon off;"], "Env": ["PATH=/usr/bin:/bin", "BOS="], "WorkingDir": "/srv", "User": "www:ek", "StopSignal": "SIGQUIT" })).unwrap();
        let ids = c.ids("root:x:0:0::/root:/bin/sh\nwww:x:101:102::/:/sbin/nologin\n", "ek:x:55:\n").unwrap();
        assert_eq!(ids, (101, 55));
        assert_eq!(
            c.lxc_lines(ids.0, ids.1).unwrap(),
            "lxc.init.cmd = /docker-entrypoint.sh nginx -g \"daemon off;\"\nlxc.init.uid = 101\nlxc.init.gid = 55\nlxc.init.cwd = /srv\nlxc.environment = PATH=/usr/bin:/bin\nlxc.environment = BOS=\nlxc.signal.halt = SIGQUIT\n"
        );
        let numeric = image(json!({ "Cmd": ["a"], "User": "1000" })).unwrap();
        assert_eq!(numeric.ids("", "").unwrap(), (1000, 0));
        assert_eq!(numeric.ids("kisi:x:1000:1001::/:/bin/sh\n", "").unwrap(), (1000, 1001));
        assert!(image(json!({ "Cmd": ["a"], "User": "yok" })).unwrap().ids("", "").is_err());
    }

    #[test]
    fn an_image_cannot_add_lines_to_the_container_configuration() {
        for config in [
            json!({ "Cmd": ["a\nlxc.hook.pre-start = /bin/x"] }),
            json!({ "Cmd": ["a"], "Env": ["A=1\nlxc.hook.pre-start = /bin/x"] }),
            json!({ "Cmd": ["a"], "Env": ["A\r=1"] }),
            json!({ "Cmd": ["a"], "Env": ["=1"] }),
            json!({ "Cmd": ["a"], "WorkingDir": "/a\nlxc.mount.entry = / host none bind" }),
            json!({ "Cmd": ["a"], "User": "0\nlxc.apparmor.profile = unconfined" }),
            json!({ "Cmd": ["a"], "StopSignal": "15 x" }),
            json!({ "Cmd": ["a"], "WorkingDir": "goreli" }),
            json!({ "Cmd": [1] }),
            json!({}),
        ] {
            assert!(image(config.clone()).is_err(), "{config}");
        }
        assert!(image(json!({ "Cmd": ["a'b\"c"] })).unwrap().lxc_lines(0, 0).is_err());
    }
}
