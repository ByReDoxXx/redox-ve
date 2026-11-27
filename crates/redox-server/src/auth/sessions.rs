//! Açık oturumların görülmesi ve kapatılması.

use super::*;

const MAX_AGENT_CHARS: usize = 160;

/// Bir oturumun kullanıcıya gösterilen bilgisi.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// Kapatma isteğinde kullanılan ad. Oturum anahtarından türetilir ama oturum açmaya yaramaz.
    pub id: String,
    pub created: u64,
    pub last_seen: u64,
    pub ip: String,
    /// Tarayıcının kendini tanıtışı; istemcinin beyanıdır.
    pub agent: String,
    pub current: bool,
}

fn public_id(key: &str) -> String {
    sha256_hex(key)[..16].to_string()
}

impl Auth {
    /// Yeni açılan oturuma nereden açıldığını yazar.
    pub fn describe_session(&self, token: &str, ip: &str, agent: &str) {
        let mut sessions = lock(&self.sessions);
        if let Some(s) = sessions.get_mut(&sha256_hex(token)) {
            s.ip = ip.chars().filter(|c| !c.is_control()).take(64).collect();
            s.agent = agent.chars().filter(|c| !c.is_control()).take(MAX_AGENT_CHARS).collect();
            let at = s.created;
            self.save_sessions(&sessions, at);
        }
    }

    /// Kullanıcının geçerli oturumları, en son kullanılan önce. `current`: isteği yapan oturumun token'ı.
    pub fn sessions_of(&self, user: &str, current: Option<&str>, now: u64) -> Vec<SessionInfo> {
        let Some(hash) = self.users().find(user).map(|u| u.password_hash.clone()) else { return Vec::new() };
        let current = current.map(sha256_hex);
        let mut list: Vec<SessionInfo> = lock(&self.sessions)
            .iter()
            .filter(|(_, s)| s.user == user && s.password_hash == hash && !expired(s, now))
            .map(|(key, s)| SessionInfo { id: public_id(key), created: s.created, last_seen: s.last_seen, ip: s.ip.clone(), agent: s.agent.clone(), current: current.as_ref() == Some(key) })
            .collect();
        list.sort_by(|a, b| b.current.cmp(&a.current).then(b.last_seen.cmp(&a.last_seen)).then(a.id.cmp(&b.id)));
        list
    }

    /// Kullanıcının, `keep` dışındaki oturumlarından `matches` olanları kapatır; kapatılan sayısını döndürür.
    fn close_where(&self, user: &str, keep: Option<&str>, now: u64, matches: impl Fn(&str) -> bool) -> usize {
        let keep = keep.map(sha256_hex);
        let mut sessions = lock(&self.sessions);
        let before = sessions.len();
        sessions.retain(|key, s| s.user != user || keep.as_ref() == Some(key) || !matches(key));
        let closed = before - sessions.len();
        if closed > 0 {
            self.save_sessions(&sessions, now);
        }
        closed
    }

    /// Kullanıcının tek bir oturumunu kapatır (başkasının oturumu bu adla kapatılamaz).
    pub fn close_session(&self, user: &str, id: &str, now: u64) -> bool {
        self.close_where(user, None, now, |key| public_id(key) == id) > 0
    }

    pub fn close_other_sessions(&self, user: &str, current: &str, now: u64) -> usize {
        self.close_where(user, Some(current), now, |_| true)
    }

    /// Kullanıcının bütün oturumlarını kapatır. API token'ları etkilenmez.
    pub fn close_all_sessions(&self, user: &str, now: u64) -> usize {
        self.close_where(user, None, now, |_| true)
    }
}
