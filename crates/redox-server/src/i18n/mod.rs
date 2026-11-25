//! Arayüz dili: Türkçe (kaynak) ve İngilizce.
//!
//! Kaynak koddaki metinler Türkçedir. İngilizce istenince her metin, anahtarı Türkçe metnin kendisi olan
//! tablodan çevrilir; tabloda yoksa Türkçesi gösterilir (eksik çeviri sayfayı bozmaz).
//!
//! İki tür metin vardır:
//! - **Sabit metinler** (`tx("Kaydet")`): tabloda birebir aranır.
//! - **Değişkenli iletiler** (`tfmt(format!("{id} silindi."))`, hata ve bildirim metinleri): üretilmiş
//!   metin, Türkçe kalıplarla eşleştirilir (`{} silindi.`), değişkenler yakalanır ve İngilizce kalıba
//!   yerleştirilir. Yakalanan değişkenin kendisi de bir iletiyse (iç içe hata) o da çevrilir.
//!
//! Dil, istek boyunca görev-yerel bir değişkende tutulur: görünüm işlevlerine ayrıca taşınmaz.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;

mod en;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Tr,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Tr => "tr",
            Lang::En => "en",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        match code.trim().to_ascii_lowercase().as_str() {
            "tr" => Some(Lang::Tr),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// `Accept-Language` başlığının ilk dili (`en-US,en;q=0.9` → İngilizce); tanınmıyorsa `None`.
    pub fn from_accept(header: &str) -> Option<Lang> {
        let first = header.split(',').next()?.split(';').next()?.trim();
        Lang::parse(first.split('-').next()?)
    }

    pub fn other(self) -> Lang {
        match self {
            Lang::Tr => Lang::En,
            Lang::En => Lang::Tr,
        }
    }
}

tokio::task_local! {
    static CURRENT: Lang;
}

/// `f` çalışırken geçerli dili `lang` yapar (isteğin tamamı bunun içinde işlenir).
pub async fn scope<F: Future>(lang: Lang, f: F) -> F::Output {
    CURRENT.scope(lang, f).await
}

/// Eşzamanlı kod için aynısı (sınamalar ve arka plan işleri).
pub fn sync_scope<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    CURRENT.sync_scope(lang, f)
}

/// Geçerli dil; bir istek kapsamında değilse Türkçe.
pub fn current() -> Lang {
    CURRENT.try_with(|lang| *lang).unwrap_or_default()
}

static EXACT: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| en::EXACT.iter().copied().collect());

struct Template {
    /// Kalıbın değişkenler arasındaki sabit parçaları.
    parts: Vec<&'static str>,
    en: &'static str,
}

/// Kalıplar, sabit metni en uzun olandan başlayarak denenir: daha özgül kalıp önce eşleşir.
static TEMPLATES: LazyLock<Vec<Template>> = LazyLock::new(|| {
    let mut list: Vec<Template> = en::TEMPLATES
        .iter()
        .map(|(tr, en)| Template { parts: tr.split("{}").collect(), en })
        // Art arda iki değişken (aralarında metin yok) güvenle ayrıştırılamaz.
        .filter(|t| t.parts.len() > 1 && t.parts[1..t.parts.len() - 1].iter().all(|p| !p.is_empty()))
        .collect();
    list.sort_by_key(|t| std::cmp::Reverse(t.parts.iter().map(|p| p.len()).sum::<usize>()));
    list
});

/// `text` kalıba uyuyorsa değişkenlerin yerindeki metinleri döndürür.
fn capture<'a>(parts: &[&str], text: &'a str) -> Option<Vec<&'a str>> {
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return None;
    }
    let body = &text[first.len()..text.len() - last.len()];
    let mut found = Vec::with_capacity(parts.len() - 1);
    let mut rest = body;
    for part in &parts[1..parts.len() - 1] {
        let at = rest.find(part)?;
        found.push(&rest[..at]);
        rest = &rest[at + part.len()..];
    }
    found.push(rest);
    Some(found)
}

/// Sabit arayüz metnini geçerli dile çevirir.
pub fn tx(text: &'static str) -> &'static str {
    match current() {
        Lang::Tr => text,
        Lang::En => EXACT.get(text).copied().unwrap_or(text),
    }
}

/// Türkçe üretilmiş bir iletiyi İngilizceye çevirir; karşılığı yoksa olduğu gibi döner.
pub fn to_english(text: &str) -> String {
    english(text, 0)
}

fn english(text: &str, depth: u8) -> String {
    if let Some(known) = EXACT.get(text) {
        return (*known).to_string();
    }
    // Çok kısa metinler (bir ad, bir sayı) kalıpla eşleştirilmez; iç içe çeviri sınırlıdır.
    if depth > 3 || text.len() < 4 {
        return text.to_string();
    }
    for template in TEMPLATES.iter() {
        let Some(found) = capture(&template.parts, text) else { continue };
        let mut out = template.en.to_string();
        for (i, value) in found.iter().enumerate() {
            out = out.replace(&format!("{{{i}}}"), &english(value, depth + 1));
        }
        return out;
    }
    // Listeler: "Pzt, Çar, Cum" gibi virgüllü metinlerde her öğe ayrı çevrilir (hepsi biliniyorsa).
    if text.contains(", ") {
        let items: Vec<Option<&str>> = text.split(", ").map(|item| EXACT.get(item).copied()).collect();
        if items.iter().all(Option::is_some) {
            return items.into_iter().flatten().collect::<Vec<_>>().join(", ");
        }
    }
    // Cümleler: "Birinci. İkinci." biçimindeki birleşik iletiler parça parça denenir.
    if let Some((head, tail)) = text.split_once(". ") {
        let (first, second) = (english(&format!("{head}."), depth + 1), english(tail, depth + 1));
        if first != format!("{head}.") || second != tail {
            return format!("{first} {second}");
        }
    }
    text.to_string()
}

/// Çok satırlı metni satır satır çevirir (komut satırı çıktıları, raporlar). Satır başındaki ve sonundaki
/// boşluklar ile sütun hizası korunur: yakalanan değişkenler boşluklarıyla birlikte yerine konur.
pub fn lines_to_english(text: &str) -> String {
    // Çok satırlı metin bütün olarak bir kalıba uydurulmaz: ilk satırın kalıbı gerisini yutardı.
    if !text.contains('\n') {
        return to_english(text);
    }
    if let Some(known) = EXACT.get(text) {
        return (*known).to_string();
    }
    text.split('\n').map(to_english).collect::<Vec<_>>().join("\n")
}

/// İstek dışında üretilen metinlerin (uyarı bildirimleri, e-postalar) dili: sunucunun varsayılanı.
static BACKGROUND_ENGLISH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Komut satırının dili (`REDOX_LANG=en` ya da `--lang en`).
static CLI_ENGLISH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_background(lang: Lang) {
    BACKGROUND_ENGLISH.store(lang == Lang::En, std::sync::atomic::Ordering::Relaxed);
}

pub fn set_cli(lang: Lang) {
    CLI_ENGLISH.store(lang == Lang::En, std::sync::atomic::Ordering::Relaxed);
}

pub fn cli_lang() -> Lang {
    if CLI_ENGLISH.load(std::sync::atomic::Ordering::Relaxed) { Lang::En } else { Lang::Tr }
}

/// İstek içinde isteğin diline, istek dışında sunucunun varsayılan diline çevirir (uyarılar, raporlar).
pub fn auto(text: String) -> String {
    let english = match CURRENT.try_with(|lang| *lang) {
        Ok(lang) => lang == Lang::En,
        Err(_) => BACKGROUND_ENGLISH.load(std::sync::atomic::Ordering::Relaxed),
    };
    if english { lines_to_english(&text) } else { text }
}

/// Komut satırı çıktısını komut satırının diline çevirir.
pub fn cli_text(text: &str) -> String {
    match cli_lang() {
        Lang::Tr => text.to_string(),
        Lang::En => lines_to_english(text),
    }
}

/// Değişkenli (üretilmiş) iletiyi geçerli dile çevirir.
pub fn tfmt(text: String) -> String {
    match current() {
        Lang::Tr => text,
        Lang::En => to_english(&text),
    }
}

/// Ödünç alınmış ileti için `tfmt`.
pub fn message(text: &str) -> String {
    tfmt(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes_and_accept_header() {
        assert_eq!((Lang::parse("EN"), Lang::parse("tr"), Lang::parse("de")), (Some(Lang::En), Some(Lang::Tr), None));
        assert_eq!(Lang::from_accept("en-US,en;q=0.9,tr;q=0.8"), Some(Lang::En));
        assert_eq!(Lang::from_accept("tr"), Some(Lang::Tr));
        assert_eq!(Lang::from_accept("de-DE,de;q=0.9"), None);
        assert_eq!((Lang::Tr.other(), Lang::En.code()), (Lang::En, "en"));
    }

    #[test]
    fn fixed_text_follows_the_current_language_and_falls_back_to_turkish() {
        assert_eq!(current(), Lang::Tr, "kapsam dışında Türkçe");
        assert_eq!(tx("Kaydet"), "Kaydet");
        sync_scope(Lang::En, || {
            assert_eq!(tx("Kaydet"), "Save");
            assert_eq!(tx("Çevirisi olmayan metin"), "Çevirisi olmayan metin");
        });
        sync_scope(Lang::Tr, || assert_eq!(tx("Kaydet"), "Kaydet"));
    }

    #[test]
    fn templates_capture_variables_in_order() {
        assert_eq!(capture(&["", " silindi."], "vm-103-1 silindi."), Some(vec!["vm-103-1"]));
        assert_eq!(capture(&["a ", " b ", " c"], "a 1 b 2 c"), Some(vec!["1", "2"]));
        assert_eq!(capture(&["a ", " c"], "x 1 c"), None);
        assert_eq!(capture(&["uzun önek ", ""], "kısa"), None);
    }

    #[test]
    fn multi_line_output_is_translated_line_by_line_and_keeps_its_columns() {
        let out = "küme      istanbul\ndüğümler  3/3 çevrimiçi, çoğunluk var\ncpu       12% (48 çekirdek)\nbilinmeyen satır";
        assert_eq!(lines_to_english(out), "cluster   istanbul\nnodes     3/3 online, quorate\ncpu       12% (48 cores)\nbilinmeyen satır");
        // Sütunlar: değişkenler dolgu boşluklarıyla birlikte taşınır.
        let row = format!("{:<14} {:<6} her {:>5} dk   {}", "ankara", 500, 60, "henüz çoğaltılmadı");
        assert_eq!(lines_to_english(&row), format!("{:<14} {:<6} every {:>5} min   {}", "ankara", 500, 60, "not replicated yet"));
        // Sürücü hatası, onu saran iletiyle birlikte çevrilir.
        assert_eq!(to_english("sürücü hatası: QEMU başlatılamadı: disk imajı bulunamadı: /x/y.qcow2"), "driver error: could not start QEMU: disk image not found: /x/y.qcow2");
        // İstek dışında dil, sunucunun varsayılanıdır.
        assert_eq!(auto("yedek yok".to_string()), "yedek yok");
        sync_scope(Lang::En, || assert_eq!(auto("yedek yok".to_string()), "no backups"));
    }

    #[test]
    fn the_table_is_well_formed() {
        for (tr, en) in en::EXACT {
            assert!(!tr.is_empty() && !en.is_empty(), "boş çeviri: {tr:?}");
        }
        for (tr, en) in en::TEMPLATES {
            let holes = tr.matches("{}").count();
            assert!(holes > 0, "kalıpta değişken yok: {tr}");
            // İngilizce kalıp, Türkçede olmayan bir değişkene başvurmamalı.
            for i in holes..holes + 4 {
                assert!(!en.contains(&format!("{{{i}}}")), "{tr} ⇒ {en}: {{{i}}} yok");
            }
        }
        let mut keys: Vec<&str> = en::EXACT.iter().map(|e| e.0).collect();
        keys.sort_unstable();
        let total = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), total, "yinelenen anahtar");
    }
}
