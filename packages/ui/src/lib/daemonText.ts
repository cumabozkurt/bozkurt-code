// Localisation of text that originates in bozkurtd (doctor checks, job titles/messages, errors).
// The daemon speaks English only; until it returns stable codes for everything (see
// docs/audit/DAEMON_TODO_FROM_UI.md) the UI maps the strings it knows. Unknown text passes
// through unchanged, so a daemon change never hides information — it only loses translation.
import type { DoctorCheck, RuntimeKind } from "../api/types";
import type { Lang } from "../i18n";
import { RUNTIME_LABEL, SERVICE_LABEL } from "./labels";

type Rule = [RegExp, (m: RegExpExecArray, lang: Lang) => string];

const rtLabel = (k: string) => RUNTIME_LABEL[k as RuntimeKind] ?? k;
const svcLabel = (n: string) => SERVICE_LABEL(n as never);
/** "node" → "Node.js", "caddy" → "Caddy", anything else unchanged. */
export const thingLabel = (id: string) => (id in RUNTIME_LABEL ? rtLabel(id) : svcLabel(id));

function apply(rules: Rule[], text: string, lang: Lang): string | null {
  for (const [re, fn] of rules) {
    const m = re.exec(text);
    if (m) return fn(m, lang);
  }
  return null;
}
const pick = (lang: Lang, en: string, tr: string) => (lang === "tr" ? tr : en);

const ACTION_TR: Record<string, string> = { start: "Başlat", stop: "Durdur", restart: "Yeniden başlat" };
const ACTION_EN: Record<string, string> = { start: "Start", stop: "Stop", restart: "Restart" };

const JOB_TITLES: Rule[] = [
  [/^Install (\S+) (\S+)$/, (m, l) => pick(l, `Install ${thingLabel(m[1])} ${m[2]}`, `${thingLabel(m[1])} ${m[2]} kurulumu`)],
  [/^Install (\S+)$/, (m, l) => pick(l, `Install ${thingLabel(m[1])}`, `${thingLabel(m[1])} kurulumu`)],
  [/^Configure DNS( resolver)?$/, (_m, l) => pick(l, "Configure DNS", "DNS yapılandırması")],
  [/^Remove DNS routing$/, (_m, l) => pick(l, "Remove DNS routing", "DNS yönlendirmesini kaldır")],
  [/^Trust local CA \(system store\)$/, (_m, l) => pick(l, "Trust local CA (system store)", "Yerel CA'ya güven (sistem deposu)")],
  [/^Trust local CA$/, (_m, l) => pick(l, "Trust local CA", "Yerel CA'ya güven")],
  [/^Remove local CA trust$/, (_m, l) => pick(l, "Remove local CA trust", "Yerel CA güvenini kaldır")],
  [/^Import sites$/, (_m, l) => pick(l, "Import sites", "Siteleri içe aktar")],
  [/^Share (.+)$/, (m, l) => pick(l, `Share ${m[1]}`, `${m[1]} paylaşımı`)],
  [/^(start|stop|restart) (\S+)$/, (m, l) => `${(l === "tr" ? ACTION_TR : ACTION_EN)[m[1]]} · ${svcLabel(m[2])}`],
];

const JOB_MESSAGES: Rule[] = [
  [/^Downloading cloudflared$/, (_m, l) => pick(l, "Downloading cloudflared", "cloudflared indiriliyor")],
  [/^Downloading (.+)$/, (m, l) => pick(l, `Downloading ${m[1]}`, `${m[1]} indiriliyor`)],
  [/^Extracting$/, (_m, l) => pick(l, "Extracting", "Arşiv açılıyor")],
  [/^Installed$/, (_m, l) => pick(l, "Installed", "Kuruldu")],
  [/^Installed and started (\S+) (\S+)$/, (m, l) => pick(l, `Installed and started ${thingLabel(m[1])} ${m[2]}`, `${thingLabel(m[1])} ${m[2]} kuruldu ve başlatıldı`)],
  [/^Installed (\S+) (\S+) \(could not start it: (.+)\)$/, (m, l) => pick(l, `Installed ${thingLabel(m[1])} ${m[2]} (could not start it: ${m[3]})`, `${thingLabel(m[1])} ${m[2]} kuruldu (başlatılamadı: ${m[3]})`)],
  [/^Unverified download: (\S+) has no published checksum; sha256 pinned for later installs$/, (m, l) =>
    pick(l, `Unverified download: ${m[1]} has no published checksum; its SHA-256 is now pinned for later installs`, `Doğrulanmamış indirme: ${m[1]} için yayımlanmış sağlama toplamı yok; SHA-256 değeri sonraki kurulumlar için sabitlendi`)],
  [/^Installed (\S+) (\S+)$/, (m, l) => pick(l, `Installed ${thingLabel(m[1])} ${m[2]}`, `${thingLabel(m[1])} ${m[2]} kuruldu`)],
  [/^Routing \.(\S+) to 127\.0\.0\.1$/, (m, l) => pick(l, `Routing .${m[1]} to 127.0.0.1`, `.${m[1]} 127.0.0.1'e yönlendiriliyor`)],
  [/^Removing \.(\S+) routing$/, (m, l) => pick(l, `Removing .${m[1]} routing`, `.${m[1]} yönlendirmesi kaldırılıyor`)],
  [/^\.(\S+) now resolves locally$/, (m, l) => pick(l, `.${m[1]} now resolves locally`, `.${m[1]} artık yerelde çözülüyor`)],
  [/^\.(\S+) is no longer routed to BOZKURT$/, (m, l) => pick(l, `.${m[1]} is no longer routed to BOZKURT`, `.${m[1]} artık BOZKURT'a yönlendirilmiyor`)],
  [/^Local CA trusted$/, (_m, l) => pick(l, "Local CA trusted", "Yerel CA'ya güveniliyor")],
  [/^Opening tunnel$/, (_m, l) => pick(l, "Opening tunnel", "Tünel açılıyor")],
  [/^Importing (.+)$/, (m, l) => pick(l, `Importing ${m[1]}`, `${m[1]} içe aktarılıyor`)],
  [/^(\S+) is (running|stopped|starting|stopping|crashed)$/, (m, l) => {
    const tr: Record<string, string> = { running: "çalışıyor", stopped: "durdu", starting: "başlıyor", stopping: "duruyor", crashed: "çöktü" };
    return pick(l, `${svcLabel(m[1])} is ${m[2]}`, `${svcLabel(m[1])} ${tr[m[2]]}`);
  }],
];

const CHECK_TITLES: Record<string, { en: string; tr: string }> = {
  home: { en: "BOZKURT home is writable", tr: "BOZKURT ana dizini yazılabilir" },
  path: { en: "Shims are on PATH", tr: "Shim'ler PATH üzerinde" },
  caddy: { en: "Web server (Caddy)", tr: "Web sunucusu (Caddy)" },
  port_http: { en: "HTTP port", tr: "HTTP portu" },
  port_https: { en: "HTTPS port", tr: "HTTPS portu" },
  runtimes: { en: "Runtimes", tr: "Çalışma ortamları" },
  sites: { en: "Site directories exist", tr: "Site klasörleri mevcut" },
  site_runtimes: { en: "Sites have their runtime", tr: "Sitelerin çalışma ortamı kurulu" },
  tls: { en: "Local CA trusted", tr: "Yerel CA güvenilir" },
  dns: { en: "Local DNS", tr: "Yerel DNS" },
};

const CHECK_DETAILS: Rule[] = [
  [/^Caddy is not installed$/, (_m, l) => pick(l, "Caddy is not installed", "Caddy kurulu değil")],
  [/^Caddy crashed$/, (_m, l) => pick(l, "Caddy crashed", "Caddy çöktü")],
  [/^no runtime installed$/, (_m, l) => pick(l, "No runtime installed", "Hiç çalışma ortamı kurulu değil")],
  [/^(.+) is not trusted$/, (m, l) => pick(l, `${m[1]} is not trusted`, `${m[1]} güvenilir değil`)],
  [/^\.(\S+) is not routed to the local resolver$/, (m, l) => pick(l, `.${m[1]} is not routed to the local resolver`, `.${m[1]} yerel çözümleyiciye yönlendirilmemiş`)],
  [/^responder on port (\d+) is not running$/, (m, l) => pick(l, `Responder on port ${m[1]} is not running`, `${m[1]} portundaki DNS yanıtlayıcısı çalışmıyor`)],
  [/^port (\d+) available$/, (m, l) => pick(l, `Port ${m[1]} available`, `${m[1]} portu kullanılabilir`)],
  [/^port (\d+) is taken(.*) or needs privileges; (\d+) will be used$/, (m, l) =>
    pick(l, `Port ${m[1]} is taken${m[2]} or needs privileges; ${m[3]} will be used`, `${m[1]} portu dolu${m[2] ? ` (${m[2].replace(/^ by /, "")})` : ""} ya da yetki gerektiriyor; ${m[3]} kullanılacak`)],
  [/^missing: (.+)$/, (m, l) => pick(l, `Missing: ${m[1]}`, `Eksik: ${m[1]}`)],
  [/^(\d+) site\(s\)$/, (m, l) => pick(l, `${m[1]} site(s)`, `${m[1]} site`)],
  [/^(.+) is not on PATH$/, (m, l) => pick(l, `${m[1]} is not on PATH`, `${m[1]} PATH üzerinde değil`)],
  [/^(.+) need PHP, none installed$/, (m, l) => pick(l, `${m[1]} need PHP, none installed`, `${m[1]} PHP gerektiriyor, kurulu PHP yok`)],
  [/^(.+) needs (PHP|Node\.js|node|php), none installed$/i, (m, l) => pick(l, `${m[1]} needs ${thingLabel(m[2].toLowerCase())}, none installed`, `${m[1]} ${thingLabel(m[2].toLowerCase())} gerektiriyor, kurulu sürüm yok`)],
  [/^(\S*) ?(running|stopped|starting|stopping|crashed|external)$/, (m, l) => {
    const tr: Record<string, string> = { running: "çalışıyor", stopped: "durdu", starting: "başlıyor", stopping: "duruyor", crashed: "çöktü", external: "harici" };
    return `${m[1] ? m[1] + " " : ""}${l === "tr" ? tr[m[2]] : m[2]}`;
  }],
  [/^all sites have their runtime$/, (_m, l) => pick(l, "All sites have their runtime", "Tüm sitelerin çalışma ortamı kurulu")],
];

/** Config keys that appear in daemon port errors. */
const CONFIG_KEY: Record<string, { en: string; tr: string }> = {
  http_port: { en: "HTTP port", tr: "HTTP portu" },
  https_port: { en: "HTTPS port", tr: "HTTPS portu" },
  dns_port: { en: "DNS port", tr: "DNS portu" },
  api_port: { en: "API port", tr: "API portu" },
  "remote.port": { en: "Remote access port", tr: "Uzaktan erişim portu" },
};
const keyLabel = (k: string, l: Lang) => CONFIG_KEY[k]?.[l] ?? k;

/** A daemon port validation error (`<key> must be between 1 and 65535`, `<key> (<n>) is already used by <other>`),
 *  as the field it belongs to plus a localized message; null for any other error. */
export function portErrorField(msg: string, lang: Lang): { key: string; other?: string; message: string } | null {
  const s = msg.replace(/^invalid input:\s*/i, "");
  const m = /^(http_port|https_port|dns_port|api_port|remote\.port) (must be between 1 and 65535|\((\d+)\) is already used by (\S+))$/.exec(s);
  if (!m) return null;
  return { key: m[1], other: m[4], message: localizeError(s, lang) };
}

/** Failed job / error text for a download that has no published checksum and no pin (`unverified_download`). */
export const isUnverifiedDownload = (msg?: string) => !!msg && /^unverified download:/i.test(msg);

const ERRORS: Rule[] = [
  [/^unverified download: (\S+) has no published checksum/i, (m, l) =>
    pick(l, `${m[1]} has no published checksum, so BOZKURT cannot verify it. Install it anyway to trust this file once (its SHA-256 is then pinned and later downloads must match), or set allow_unverified = true in config.toml.`,
      `${m[1]} için yayımlanmış bir sağlama toplamı yok, BOZKURT dosyayı doğrulayamıyor. Bu dosyaya bir kez güvenmek için yine de kurun (SHA-256 değeri sabitlenir, sonraki indirmeler eşleşmek zorundadır) ya da config.toml'da allow_unverified = true ayarlayın.`)],
  [/^checksum mismatch for (\S+) \(differs from the sha256 pinned on first download\)/i, (m, l) =>
    pick(l, `${m[1]} differs from the copy pinned on its first download and was refused. The file may have been replaced or tampered with upstream.`,
      `${m[1]}, ilk indirmede sabitlenen kopyadan farklı olduğu için reddedildi. Dosya kaynağında değiştirilmiş ya da kurcalanmış olabilir.`)],
  [/^checksum mismatch for (\S+):/i, (m, l) =>
    pick(l, `${m[1]} failed checksum verification and was discarded. Try again; if it keeps failing, the download source may be broken or tampered with.`,
      `${m[1]} sağlama toplamı doğrulamasını geçemedi ve silindi. Yeniden deneyin; sürekli başarısız oluyorsa indirme kaynağı bozuk ya da kurcalanmış olabilir.`)],
  [/^(\S+) must be between 1 and 65535$/, (m, l) => pick(l, `${keyLabel(m[1], l)} must be between 1 and 65535`, `${keyLabel(m[1], l)} 1 ile 65535 arasında olmalı`)],
  [/^(\S+) \((\d+)\) is already used by (\S+)$/, (m, l) =>
    pick(l, `${keyLabel(m[1], l)} (${m[2]}) is already used by the ${keyLabel(m[3], l)}`, `${keyLabel(m[1], l)} (${m[2]}) zaten ${keyLabel(m[3], l)} tarafından kullanılıyor`)],
  [/^(api_port|remote\.port) can only be changed locally$/, (m, l) =>
    pick(l, `The ${keyLabel(m[1], l)} can only be changed on the computer itself, not from a paired device.`, `${keyLabel(m[1], l)} eşleştirilmiş bir cihazdan değil, yalnızca bilgisayarın kendisinden değiştirilebilir.`)],
  [/^cannot listen on [^:]+:(\d+): (.+)$/, (m, l) =>
    pick(l, `Cannot listen on port ${m[1]} (${m[2]}). Remote access stays off; free the port or change remote.port in config.toml.`,
      `${m[1]} portu dinlenemiyor (${m[2]}). Uzaktan erişim kapalı kaldı; portu boşaltın ya da config.toml'da remote.port değerini değiştirin.`)],
  [/^Too many pairing attempts; wait a minute$/, (_m, l) =>
    pick(l, "Too many pairing attempts. Wait a minute and try again.", "Çok fazla eşleştirme denemesi. Bir dakika bekleyip yeniden deneyin.")],
  [/^Too many wrong pairing codes/, (_m, l) =>
    pick(l, "Too many wrong codes: the pairing code was revoked. Generate a new code on the computer and enter that one.", "Çok fazla yanlış kod girildi, eşleştirme kodu iptal edildi. Bilgisayarda yeni bir kod oluşturup onu girin.")],
  [/`?\.?(\w+)`? is a real HSTS-preloaded TLD/i, (m, l) =>
    pick(l, `.${m[1]} is a real, HSTS-preloaded TLD — browsers would force HTTPS and real DNS. Use .test instead.`, `.${m[1]} gerçek ve HSTS-önyüklü bir TLD; tarayıcılar HTTPS ve gerçek DNS zorlar. Bunun yerine .test kullanın.`)],
  [/^Pairing code is invalid or expired$/, (_m, l) => pick(l, "Pairing code is invalid or expired", "Eşleştirme kodu geçersiz ya da süresi dolmuş")],
  [/^Cannot reach bozkurtd$/, (_m, l) => pick(l, "Cannot reach bozkurtd", "bozkurtd'ye ulaşılamıyor")],
  [/^Request timed out$/, (_m, l) => pick(l, "Request timed out", "İstek zaman aşımına uğradı")],
  [/^directory (.+) does not exist$/, (m, l) => pick(l, `Folder ${m[1]} does not exist`, `${m[1]} klasörü yok`)],
  [/^PHP (\S+) is not installed/, (m, l) => pick(l, `PHP ${m[1]} is not installed`, `PHP ${m[1]} kurulu değil`)],
];

export function localizeJobTitle(title: string, lang: Lang): string {
  return apply(JOB_TITLES, title, lang) ?? title;
}
export function localizeJobMessage(msg: string | undefined, lang: Lang): string | undefined {
  if (!msg) return msg;
  return apply(JOB_MESSAGES, msg, lang) ?? localizeError(msg, lang);
}
export function localizeCheck(c: DoctorCheck, lang: Lang): { title: string; detail: string } {
  const title = CHECK_TITLES[c.id]?.[lang] ?? c.title;
  return { title, detail: apply(CHECK_DETAILS, c.detail, lang) ?? c.detail };
}
/** Strips the daemon's error-kind prefix ("not found: ", "invalid input: ") and translates known messages. */
export function localizeError(msg: string, lang: Lang): string {
  const stripped = msg.replace(/^(not found|invalid input|conflict|bad request|internal error|unavailable|forbidden):\s*/i, "");
  return apply(ERRORS, stripped, lang) ?? stripped;
}
