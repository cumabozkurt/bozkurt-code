import { useEffect, useRef, useState } from "react";
import { Camera, Link2, PlayCircle } from "lucide-react";
import { errorMessage } from "../store";
import { PAIRING_CODE_DIGITS as CODE_DIGITS, pairRemote, parsePairingUri } from "../api/client";
import type { RemoteConnection } from "../api";
import { Button, Field, Logo } from "../components/ui";
import { LANGUAGES, useI18n, type Lang } from "../i18n";
import { deviceName, isTauri, scanQr } from "../lib/platform";

const CODE_RE = new RegExp(`^\\d{${CODE_DIGITS}}$`);

/** First screen of the mobile companion: scan the desktop's QR code or type host + 8-digit code. */
export function ConnectScreen({ onConnected, onDemo, initial }: { onConnected: (c: RemoteConnection) => void; onDemo?: () => void; initial?: { host: string; code: string } | null }) {
  const { t, lang, setLang } = useI18n();
  const [host, setHost] = useState(initial?.host ?? "");
  const [code, setCode] = useState(initial?.code ?? "");
  const [name, setName] = useState(deviceName());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const auto = useRef(false);

  const connect = async (h = host, c = code) => {
    setError(null);
    setBusy(true);
    try {
      const { baseUrl, token, deviceId } = await pairRemote(h, c, name || deviceName());
      onConnected({ baseUrl, token, name: h, deviceId });
    } catch (e) {
      setError(t("connect.failed", { msg: errorMessage(e) }));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (!auto.current && initial?.host && CODE_RE.test(initial.code)) {
      auto.current = true;
      void connect(initial.host, initial.code);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const scan = async () => {
    setError(null);
    try {
      const text = await scanQr();
      if (!text) return;
      const p = parsePairingUri(text);
      if (!p) return setError(t("connect.badQr"));
      setHost(p.host);
      setCode(p.code);
      await connect(p.host, p.code);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="connect-screen">
      <div className="card connect-card" style={{ gap: 18 }}>
        <div className="col" style={{ alignItems: "center", textAlign: "center", gap: 10 }}>
          <Logo size={72} />
          <div className="brand-name" style={{ fontSize: 16 }}>{t("app.name")}</div>
          <h1 style={{ fontSize: 20 }}>{t("connect.title")}</h1>
          <p className="muted">{t("connect.subtitle")}</p>
        </div>
        {isTauri() ? (
          <Button variant="primary" icon={<Camera />} onClick={scan} style={{ height: 44 }}>{t("connect.scan")}</Button>
        ) : (
          <p className="faint" style={{ fontSize: 12.5, textAlign: "center" }}>{t("connect.scanUnavailable")}</p>
        )}
        <div className="divider">{t("connect.or")}</div>
        <form className="col" style={{ gap: 12 }} onSubmit={(e) => { e.preventDefault(); void connect(); }}>
          <Field label={t("connect.host")} htmlFor="cn-host">
            <input id="cn-host" className="input mono" inputMode="url" autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder={t("connect.hostPlaceholder")} value={host} onChange={(e) => setHost(e.target.value)} required />
          </Field>
          <Field label={t("connect.code")} htmlFor="cn-code">
            <input id="cn-code" className="input mono" inputMode="numeric" autoComplete="one-time-code" pattern={`[0-9]{${CODE_DIGITS}}`} maxLength={CODE_DIGITS} placeholder="12345678" value={code}
              onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, CODE_DIGITS))} required style={{ letterSpacing: "0.3em", fontSize: 18, height: 42 }} />
          </Field>
          <Field label={t("connect.deviceName")} htmlFor="cn-name">
            <input id="cn-name" className="input" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          {error && <p role="alert" style={{ color: "var(--danger)", fontSize: 13 }}>{error}</p>}
          <Button type="submit" variant="primary" icon={<Link2 />} loading={busy} disabled={!host || !CODE_RE.test(code)} style={{ height: 42 }}>
            {busy ? t("connect.connecting") : t("connect.connect")}
          </Button>
        </form>
        <div className="row">
          <select className="input" style={{ width: 130 }} aria-label={t("settings.language")} value={lang} onChange={(e) => setLang(e.target.value as Lang)}>
            {LANGUAGES.map((l) => <option key={l.id} value={l.id}>{l.label}</option>)}
          </select>
          <span className="spacer" />
          {onDemo && <Button variant="ghost" icon={<PlayCircle />} onClick={onDemo}>{t("connect.demo")}</Button>}
        </div>
      </div>
    </div>
  );
}
