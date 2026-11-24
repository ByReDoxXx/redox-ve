//! Arayüzün üstündeki komut satırı: `start 103`, `open db-postgres`, `help` …
//!
//! Ayrıştırma (`parse`) küme durumuna bakmaz; çözümleme (`resolve`) hedefi kümede arar.

use crate::cluster::Cluster;
use crate::model::{Action, VmId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Home,
    Storage,
    Act { action: Action, target: String },
    Open(String),
    /// Bilinen bir komut değil: önce kaynak adı olarak denenir, yoksa arama yapılır.
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("komut boş; komutları görmek için \"help\" yazın")]
    Empty,
    #[error("\"{0}\" bir hedef ister, örn. \"{0} 103\"")]
    MissingTarget(String),
    #[error("konuk bulunamadı: {0}")]
    GuestNotFound(String),
    #[error("kaynak bulunamadı: {0}")]
    NotFound(String),
}

/// Çözümlenmiş komut: arayüzün gideceği yer ya da uygulayacağı eylem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Help,
    Home,
    Storage,
    Node(String),
    Guest(VmId),
    Act { vmid: VmId, action: Action },
    Search(String),
}

pub fn parse(input: &str) -> Result<Command, CommandError> {
    let input = input.trim();
    let mut words = input.split_whitespace();
    let Some(first) = words.next() else {
        return Err(CommandError::Empty);
    };
    let verb = first.to_lowercase();
    let arg = words.next().map(str::to_string);
    let need = |arg: Option<String>| arg.ok_or_else(|| CommandError::MissingTarget(verb.clone()));

    Ok(match verb.as_str() {
        "help" | "?" | "yardım" => Command::Help,
        "home" | "dc" => Command::Home,
        "storage" | "depolama" => Command::Storage,
        "open" | "show" => Command::Open(need(arg)?),
        _ => match verb.parse::<Action>() {
            Ok(action) => Command::Act { action, target: need(arg)? },
            Err(()) => Command::Search(input.to_string()),
        },
    })
}

/// Kimlik ya da tam ad ile konuk bulur.
fn find_guest(cluster: &Cluster, target: &str) -> Option<VmId> {
    let by_id = target.parse::<VmId>().ok();
    cluster
        .guests
        .iter()
        .find(|g| Some(g.vmid) == by_id || g.name == target)
        .map(|g| g.vmid)
}

/// Düğüm, konuk (kimlik/tam ad), ardından adın bir parçası olarak arar.
fn find_any(cluster: &Cluster, target: &str) -> Option<Resolved> {
    if let Some(n) = cluster.nodes.iter().find(|n| n.id == target) {
        return Some(Resolved::Node(n.id.clone()));
    }
    if let Some(vmid) = find_guest(cluster, target) {
        return Some(Resolved::Guest(vmid));
    }
    let mut partial = cluster.guests.iter().filter(|g| g.name.contains(target));
    match (partial.next(), partial.next()) {
        // Yalnızca tek eşleşme varsa doğrudan aç; birden çoksa belirsizdir.
        (Some(g), None) => Some(Resolved::Guest(g.vmid)),
        _ => None,
    }
}

pub fn resolve(command: &Command, cluster: &Cluster) -> Result<Resolved, CommandError> {
    Ok(match command {
        Command::Help => Resolved::Help,
        Command::Home => Resolved::Home,
        Command::Storage => Resolved::Storage,
        Command::Act { action, target } => Resolved::Act {
            vmid: find_guest(cluster, target)
                .ok_or_else(|| CommandError::GuestNotFound(target.clone()))?,
            action: *action,
        },
        Command::Open(target) => {
            find_any(cluster, target).ok_or_else(|| CommandError::NotFound(target.clone()))?
        }
        Command::Search(text) => {
            find_any(cluster, text).unwrap_or_else(|| Resolved::Search(text.clone()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Result<Resolved, CommandError> {
        resolve(&parse(input)?, &Cluster::demo(0))
    }

    #[test]
    fn parses_verbs_case_insensitively_and_trims() {
        assert_eq!(parse("  HELP "), Ok(Command::Help));
        assert_eq!(parse("?"), Ok(Command::Help));
        assert_eq!(parse("storage"), Ok(Command::Storage));
        assert_eq!(parse("Start 103"), Ok(Command::Act { action: Action::Start, target: "103".into() }));
        assert_eq!(parse("open   db-postgres  extra"), Ok(Command::Open("db-postgres".into())));
        assert_eq!(parse("web prod"), Ok(Command::Search("web prod".into())));
    }

    #[test]
    fn parse_errors() {
        assert_eq!(parse(""), Err(CommandError::Empty));
        assert_eq!(parse("   "), Err(CommandError::Empty));
        assert_eq!(parse("stop"), Err(CommandError::MissingTarget("stop".into())));
        assert_eq!(parse("open"), Err(CommandError::MissingTarget("open".into())));
    }

    #[test]
    fn actions_resolve_by_id_or_exact_name() {
        assert_eq!(run("start 103"), Ok(Resolved::Act { vmid: 103, action: Action::Start }));
        assert_eq!(run("reboot db-postgres"), Ok(Resolved::Act { vmid: 102, action: Action::Reboot }));
        assert_eq!(run("stop 999"), Err(CommandError::GuestNotFound("999".into())));
        // Eylemler kısmi adla çalışmaz: yanlış makineyi kapatmak pahalıdır.
        assert_eq!(run("stop postgres"), Err(CommandError::GuestNotFound("postgres".into())));
    }

    #[test]
    fn open_finds_nodes_guests_and_unique_partials() {
        assert_eq!(run("open pve-ank-01"), Ok(Resolved::Node("pve-ank-01".into())));
        assert_eq!(run("open 200"), Ok(Resolved::Guest(200)));
        assert_eq!(run("open postgres"), Ok(Resolved::Guest(102)));
        assert_eq!(run("open web-prod"), Err(CommandError::NotFound("web-prod".into())), "iki eşleşme belirsizdir");
        assert_eq!(run("open yok"), Err(CommandError::NotFound("yok".into())));
    }

    #[test]
    fn bare_words_open_or_fall_back_to_search() {
        assert_eq!(run("102"), Ok(Resolved::Guest(102)));
        assert_eq!(run("monitoring"), Ok(Resolved::Guest(201)));
        assert_eq!(run("web-prod"), Ok(Resolved::Search("web-prod".into())));
        assert_eq!(run("zzz"), Ok(Resolved::Search("zzz".into())));
        assert_eq!(run("home"), Ok(Resolved::Home));
    }
}
