import type { RuntimeKind, ServiceName } from "../api/types";
import type { MessageKey } from "../i18n";

export const RUNTIME_LABEL: Record<RuntimeKind, string> = {
  php: "PHP",
  node: "Node.js",
  python: "Python",
  bun: "Bun",
  go: "Go",
};

const SERVICE_NAMES: Record<string, string> = {
  caddy: "Caddy",
  dns: "DNS",
  mariadb: "MariaDB",
  postgres: "PostgreSQL",
  redis: "Redis",
  mailpit: "Mailpit",
};

export const SERVICE_LABEL = (n: ServiceName): string =>
  n.startsWith("php-fpm-") ? `PHP-FPM ${n.slice(8)}` : SERVICE_NAMES[n] ?? n;

export const serviceDescKey = (n: ServiceName): MessageKey =>
  n.startsWith("php-fpm-") ? "services.desc.phpfpm" : (`services.desc.${n}` as MessageKey);

/** Mailpit's web UI port (bz-core services.rs MAILPIT_UI_PORT). `Service.port` is its SMTP port (1025). */
export const MAILPIT_UI_PORT = 8025;

/** URL of a service's own web UI when it has one. Never derived from `Service.port`, which is the protocol port. */
export const serviceWebUrl = (n: ServiceName): string | null =>
  n === "mailpit" ? `http://127.0.0.1:${MAILPIT_UI_PORT}` : null;
