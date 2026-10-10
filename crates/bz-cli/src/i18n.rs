//! Minimal Turkish / English message catalog, picked from `LC_ALL` / `LC_MESSAGES` / `LANG`.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Tr,
}

/// Detect the language from a locale string such as `tr_TR.UTF-8`.
pub fn lang_from(locale: &str) -> Lang {
    if locale.to_ascii_lowercase().starts_with("tr") {
        Lang::Tr
    } else {
        Lang::En
    }
}

/// Process language (cached).
pub fn lang() -> Lang {
    static L: OnceLock<Lang> = OnceLock::new();
    *L.get_or_init(|| {
        for var in ["BZ_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    return lang_from(&v);
                }
            }
        }
        Lang::En
    })
}

/// Look up a message key. Unknown keys return the key itself.
pub fn t(key: &str) -> &'static str {
    tr_in(lang(), key)
}

pub fn tr_in(lang: Lang, key: &str) -> &'static str {
    let (en, tr) = match key {
        "proceed" => ("Proceed? [y/N] ", "Devam edilsin mi? [e/H] "),
        "aborted" => ("Aborted.", "İptal edildi."),
        "starting_daemon" => ("Starting bozkurtd…", "bozkurtd başlatılıyor…"),
        "daemon_started" => ("bozkurtd is running", "bozkurtd çalışıyor"),
        "daemon_not_running" => ("bozkurtd is not running", "bozkurtd çalışmıyor"),
        "daemon_stopped" => ("bozkurtd stopped", "bozkurtd durduruldu"),
        "daemon_unreachable" => (
            "Cannot reach bozkurtd. Start it with `bz daemon start`.",
            "bozkurtd'ye ulaşılamıyor. `bz daemon start` ile başlatın.",
        ),
        "daemon_not_found" => (
            "Could not find the `bozkurtd` executable (next to bz or on PATH).",
            "`bozkurtd` bulunamadı (bz ile aynı klasörde ya da PATH üzerinde olmalı).",
        ),
        "detached" => (
            "Detached; the job keeps running in the background.",
            "Ayrıldı; iş arka planda çalışmaya devam ediyor.",
        ),
        "job_done" => ("Done", "Tamamlandı"),
        "job_failed" => ("Failed", "Başarısız"),
        "confirm_needed_json" => (
            "Confirmation required: re-run with --yes",
            "Onay gerekli: --yes ile tekrar çalıştırın",
        ),
        "no_site_here" => (
            "No site is linked at the current directory.",
            "Bu klasöre bağlı bir site yok.",
        ),
        "detected" => ("Detected", "Algılandı"),
        "linked" => ("Linked", "Bağlandı"),
        "updated" => ("Updated", "Güncellendi"),
        "unlinked" => ("Unlinked", "Bağlantı kaldırıldı"),
        "files_untouched" => (
            "project files were not touched",
            "proje dosyalarına dokunulmadı",
        ),
        "will_change" => ("This will:", "Yapılacaklar:"),
        "needs_admin" => (
            "This needs administrator rights; your OS may ask for your password.",
            "Bu işlem yönetici yetkisi gerektirir; işletim sistemi parolanızı sorabilir.",
        ),
        "share_url" => ("Public URL", "Herkese açık adres"),
        "share_stop_hint" => (
            "Press Ctrl-C to stop sharing.",
            "Paylaşımı durdurmak için Ctrl-C'ye basın.",
        ),
        "share_stopped" => ("Sharing stopped.", "Paylaşım durduruldu."),
        "opening" => ("Opening", "Açılıyor"),
        "backup" => ("Backup written to", "Yedek yazıldı"),
        "registered" => (
            "Registered bozkurt MCP server in",
            "bozkurt MCP sunucusu kaydedildi:",
        ),
        "unregistered" => (
            "Removed bozkurt MCP server from",
            "bozkurt MCP sunucusu kaldırıldı:",
        ),
        "already_registered" => ("Already up to date:", "Zaten güncel:"),
        "not_registered" => ("No bozkurt entry in", "bozkurt kaydı yok:"),
        "pair_code" => (
            "Pairing code (valid 5 min)",
            "Eşleştirme kodu (5 dk geçerli)",
        ),
        "select_items" => (
            "Select items to import (e.g. 1,3 or `all`, empty = none): ",
            "İçe aktarılacakları seçin (ör. 1,3 ya da `all`, boş = hiçbiri): ",
        ),
        "nothing_found" => ("Nothing found.", "Hiçbir şey bulunamadı."),
        "install_first" => (
            "is not installed. Install it first?",
            "kurulu değil. Önce kurulsun mu?",
        ),
        _ => return leak(key),
    };
    match lang {
        Lang::En => en,
        Lang::Tr => tr,
    }
}

fn leak(key: &str) -> &'static str {
    // Unknown keys are programming errors; keep it cheap and obvious.
    Box::leak(key.to_string().into_boxed_str())
}

/// Is `answer` a "yes" in either language?
pub fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_lowercase().as_str(),
        "y" | "yes" | "e" | "evet"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_turkish() {
        assert_eq!(lang_from("tr_TR.UTF-8"), Lang::Tr);
        assert_eq!(lang_from("en_US.UTF-8"), Lang::En);
        assert_eq!(lang_from("C"), Lang::En);
        assert_eq!(tr_in(Lang::Tr, "aborted"), "İptal edildi.");
        assert!(is_yes("Evet") && is_yes("y") && !is_yes("") && !is_yes("n"));
    }
}
