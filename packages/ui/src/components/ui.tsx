import { useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type ReactNode, type RefObject } from "react";
import { create } from "zustand";
import { AlertTriangle, CheckCircle2, CloudOff, Info, Loader2, RefreshCw, X, XCircle } from "lucide-react";
import type { ServiceStatus } from "../api/types";
import { useT } from "../i18n";
import { useStore } from "../store";

/** Original BOZKURT CODE mark: a geometric wolf head (ears, cheek ruff, eye slits, brow ridge, nose). */
export const LOGO_PATH =
  "M12 4 L25 19 H39 L52 4 L55 28 L61 37 L50 41 L32 60 L14 41 L3 37 L9 28 Z M16 12 L22 20 L17 25 Z M48 12 L42 20 L47 25 Z M19 31 L28 34 L20 36 Z M45 31 L36 34 L44 36 Z M27 47 H37 L32 53 Z M32 22 L35 33 L32 40 L29 33 Z";

export function Logo({ size = 30, className }: { size?: number; className?: string }) {
  const id = useId().replace(/:/g, "");
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} className={className} aria-hidden="true">
      <defs>
        <linearGradient id={`g${id}`} x1="12" y1="4" x2="52" y2="60" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#7dd3fc" />
          <stop offset="1" stopColor="#0e7490" />
        </linearGradient>
      </defs>
      <path fill={`url(#g${id})`} fillRule="evenodd" d={LOGO_PATH} />
    </svg>
  );
}

type BtnProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "danger" | "ghost" | "default";
  size?: "sm" | "md";
  icon?: ReactNode;
  loading?: boolean;
  iconOnly?: boolean;
};
export function Button({ variant = "default", size = "md", icon, loading, iconOnly, className, children, disabled, type, ...rest }: BtnProps) {
  const cls = ["btn", variant !== "default" && variant, size === "sm" && "sm", iconOnly && "icon", className].filter(Boolean).join(" ");
  return (
    <button type={type ?? "button"} className={cls} disabled={disabled || loading} aria-busy={loading || undefined} {...rest}>
      {loading ? <Loader2 className="spin" /> : icon}
      {!iconOnly && children}
    </button>
  );
}

export function Switch({ checked, onChange, label, disabled, hint }: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode; disabled?: boolean; hint?: ReactNode }) {
  return (
    <label className="switch">
      <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span className="track" aria-hidden="true" />
      <span className="col" style={{ gap: 0 }}>
        <span>{label}</span>
        {hint && <span className="faint" style={{ fontSize: 12 }}>{hint}</span>}
      </span>
    </label>
  );
}

export function StatusBadge({ status }: { status: ServiceStatus }) {
  const t = useT();
  const moving = status === "starting" || status === "stopping";
  const tone = status === "running" ? "ok" : status === "crashed" ? "danger" : moving ? "warn" : status === "external" ? "accent" : "";
  return (
    <span className={`badge ${tone} ${moving ? "pulse" : ""}`}>
      <span className="dot" />
      {t(`status.${status}`)}
    </span>
  );
}

/** `indeterminate` for phases that report no progress (e.g. archive extraction). */
export function Progress({ value, label, indeterminate }: { value: number; label?: string; indeterminate?: boolean }) {
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  if (indeterminate)
    return (
      <div className="progress indeterminate" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-label={label} aria-busy="true">
        <div />
      </div>
    );
  return (
    <div className="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={label}>
      <div style={{ width: `${pct}%` }} />
    </div>
  );
}

/** True for job phases where bozkurtd sends no further progress until the end. */
export const isIndeterminatePhase = (message?: string) => !!message && /^(Extracting|Opening tunnel|Routing|Removing)/.test(message);

/** Re-renders every `ms` and returns Date.now() (for ticking uptimes / countdowns). */
export function useNow(ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const i = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(i);
  }, [ms]);
  return now;
}

/** "Failed to load" state with a retry action (distinct from an empty state). */
export function ErrorState({ title, message, onRetry, offline }: { title?: string; message?: string | null; onRetry?: () => void; offline?: boolean }) {
  const t = useT();
  return (
    <div className="empty" role="alert">
      <div className="empty-icon warn" aria-hidden>{offline ? <CloudOff /> : <AlertTriangle />}</div>
      <h2>{title ?? (offline ? t("state.offlineTitle") : t("state.errorTitle"))}</h2>
      <p>{message || (offline ? t("state.offlineBody") : t("error.generic"))}</p>
      {onRetry && <Button icon={<RefreshCw />} onClick={onRetry}>{t("common.retry")}</Button>}
    </div>
  );
}

/** Shared loading placeholder. */
export function Loading({ label }: { label?: string }) {
  const t = useT();
  return (
    <div className="loading muted" role="status" aria-live="polite">
      <Loader2 className="spin" size={16} aria-hidden />
      {label ?? t("common.loading")}
    </div>
  );
}

export function EmptyState({ icon, title, body, action }: { icon: ReactNode; title: string; body?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <div className="empty-icon" aria-hidden>{icon}</div>
      <h2>{title}</h2>
      {body && <p>{body}</p>}
      {action}
    </div>
  );
}

function useEscape(onClose: () => void) {
  const ref = useRef(onClose);
  ref.current = onClose;
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        ref.current();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);
}

/** Keeps Tab focus inside `ref` and restores focus to the opener on unmount. */
function useFocusTrap(ref: RefObject<HTMLElement>) {
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const el = ref.current;
    if (el && !el.contains(document.activeElement)) {
      el.querySelector<HTMLElement>("[autofocus], input, select, textarea, button:not([disabled])")?.focus();
    }
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || !el) return;
      const items = [...el.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])')];
      if (!items.length) return;
      const a = items[0];
      const b = items[items.length - 1];
      if (e.shiftKey && document.activeElement === a) {
        e.preventDefault();
        b.focus();
      } else if (!e.shiftKey && document.activeElement === b) {
        e.preventDefault();
        a.focus();
      }
    };
    el?.addEventListener("keydown", h);
    return () => {
      el?.removeEventListener("keydown", h);
      prev?.focus?.();
    };
  }, [ref]);
}

export function Dialog({ title, onClose, children, footer, size }: { title: ReactNode; onClose: () => void; children: ReactNode; footer?: ReactNode; size?: "sm" | "lg" }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEscape(onClose);
  useFocusTrap(ref);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} className={`dialog ${size ?? ""}`} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <div className="dialog-head">
          <h2 id={titleId}>{title}</h2>
          <Button variant="ghost" iconOnly icon={<X />} aria-label={t("common.close")} title={t("common.close")} onClick={onClose} />
        </div>
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-foot">{footer}</div>}
      </div>
    </div>
  );
}

export function Drawer({ title, onClose, children, wide, actions }: { title: ReactNode; onClose: () => void; children: ReactNode; wide?: boolean; actions?: ReactNode }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEscape(onClose);
  useFocusTrap(ref);
  return (
    <>
      <div className="overlay" style={{ background: "rgba(2,6,23,.25)", zIndex: 39 }} onMouseDown={onClose} />
      <div ref={ref} className={`drawer ${wide ? "wide" : ""}`} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <div className="drawer-head">
          <h2 id={titleId} className="ellipsis">{title}</h2>
          {actions}
          <Button variant="ghost" iconOnly icon={<X />} aria-label={t("common.close")} onClick={onClose} />
        </div>
        <div className="drawer-body">{children}</div>
      </div>
    </>
  );
}

export function Tabs<T extends string>({ value, onChange, items, label }: { value: T; onChange: (v: T) => void; items: { id: T; label: ReactNode }[]; label: string }) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {items.map((it) => (
        <button key={it.id} type="button" role="tab" aria-selected={value === it.id} onClick={() => onChange(it.id)}>
          {it.label}
        </button>
      ))}
    </div>
  );
}

export function Segmented<T extends string>({ value, onChange, items, label }: { value: T; onChange: (v: T) => void; items: { id: T; label: ReactNode }[]; label: string }) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {items.map((it) => (
        <button key={it.id} type="button" aria-pressed={value === it.id} onClick={() => onChange(it.id)}>
          {it.label}
        </button>
      ))}
    </div>
  );
}

export function Field({ label, hint, children, htmlFor }: { label: ReactNode; hint?: ReactNode; children: ReactNode; htmlFor?: string }) {
  return (
    <div className="field">
      <label htmlFor={htmlFor}>{label}</label>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </div>
  );
}

function highlight(text: string, q: string): ReactNode {
  if (!q) return text;
  const i = text.toLowerCase().indexOf(q.toLowerCase());
  if (i < 0) return text;
  return (
    <>
      {text.slice(0, i)}
      <mark>{text.slice(i, i + q.length)}</mark>
      {text.slice(i + q.length)}
    </>
  );
}

export function LogView({ lines, follow = true, tall, query = "", showSource, label = "log" }: { lines: { id?: number; source?: string; line: string }[]; follow?: boolean; tall?: boolean; query?: string; showSource?: boolean; label?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (el && follow && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines, follow]);
  return (
    <div
      ref={ref}
      className={`logview ${tall ? "tall" : ""}`}
      role="log"
      aria-label={label}
      tabIndex={0}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      {lines.map((l, i) => {
        const lvl = /\b(error|fatal|crash|failed|exited)\b/i.test(l.line) ? "err" : /\b(warn|warning|deprecated)\b/i.test(l.line) ? "warn" : "";
        return (
          <div key={l.id ?? `i${i}`} className={`ln ${lvl}`}>
            {showSource && l.source && <span className="src">[{l.source}]</span>}
            {highlight(l.line, query)}
          </div>
        );
      })}
    </div>
  );
}

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);
  const t = useT();
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((x) => (
        <div key={x.id} className={`toast ${x.kind}`}>
          {x.kind === "success" ? <CheckCircle2 className="kind" /> : x.kind === "error" ? <XCircle className="kind" /> : x.kind === "warn" ? <AlertTriangle className="kind" /> : <Info className="kind" />}
          <span className="text">{x.text}</span>
          {x.action && (
            <Button size="sm" onClick={() => { x.action!.run(); dismiss(x.id); }}>
              {x.action.label}
            </Button>
          )}
          <Button size="sm" variant="ghost" iconOnly icon={<X />} aria-label={t("common.close")} onClick={() => dismiss(x.id)} />
        </div>
      ))}
    </div>
  );
}

// ---- Promise-based confirm dialog ----
interface ConfirmReq { message: string; confirmLabel: string; danger?: boolean; title?: string; details?: string[]; resolve: (ok: boolean) => void }
const useConfirmStore = create<{ req: ConfirmReq | null }>(() => ({ req: null }));
/** `opts.title` overrides the dialog title (default: the confirm label); `opts.details` adds paragraphs. */
export function confirmAction(message: string, confirmLabel: string, danger = true, opts?: { title?: string; details?: string[] }): Promise<boolean> {
  return new Promise((resolve) => useConfirmStore.setState({ req: { message, confirmLabel, danger, title: opts?.title, details: opts?.details, resolve } }));
}
export function ConfirmHost() {
  const req = useConfirmStore((s) => s.req);
  const t = useT();
  if (!req) return null;
  const done = (ok: boolean) => {
    req.resolve(ok);
    useConfirmStore.setState({ req: null });
  };
  return (
    <Dialog
      size="sm"
      title={req.title ?? req.confirmLabel}
      onClose={() => done(false)}
      footer={
        <>
          <Button onClick={() => done(false)}>{t("common.cancel")}</Button>
          <Button variant={req.danger ? "danger" : "primary"} onClick={() => done(true)} autoFocus>
            {req.confirmLabel}
          </Button>
        </>
      }
    >
      <p>{req.message}</p>
      {req.details?.map((d, i) => <p key={i} className="muted">{d}</p>)}
    </Dialog>
  );
}

export function formatDuration(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  if (m) return `${m}m`;
  return `${secs}s`;
}
export function formatBytes(n?: number): string {
  if (!n) return "";
  return n > 1e9 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.round(n / 1e6)} MB`;
}
