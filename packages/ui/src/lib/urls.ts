import type { Site } from "../api/types";

export interface WebPorts {
  http_port: number;
  https_port: number;
}

/** A TCP port the daemon can actually use (0 means "unset/invalid" and is never sent). */
export const isValidPort = (p: unknown): p is number => typeof p === "number" && Number.isInteger(p) && p >= 1 && p <= 65535;

/**
 * Browser URL of a site. Appends `:port` when Caddy is not on 80/443 (custom ports or the
 * daemon's 8080/8443 fallback); otherwise the link would hit nothing.
 */
export function siteUrl(s: Pick<Site, "https" | "domain">, ports?: Partial<WebPorts> | null): string {
  const scheme = s.https ? "https" : "http";
  const port = s.https ? ports?.https_port : ports?.http_port;
  const def = s.https ? 443 : 80;
  return `${scheme}://${s.domain}${isValidPort(port) && port !== def ? `:${port}` : ""}`;
}

/** IPv4 private ranges first (192.168/16, 172.16/12 minus docker's 172.17/16, 10/8), then others. */
export function addressRank(ip: string): number {
  const m = /^(\d+)\.(\d+)\.(\d+)\.(\d+)$/.exec(ip);
  if (!m) return 9;
  const [a, b] = [Number(m[1]), Number(m[2])];
  if (a === 192 && b === 168) return 0;
  if (a === 172 && b >= 16 && b <= 31) return b === 17 ? 4 : 1; // 172.17.x is docker0 by default
  if (a === 10) return 2;
  if (a === 100 && b >= 64 && b <= 127) return 5; // CGNAT / Tailscale
  if (a === 169 && b === 254) return 8; // link-local
  return 3;
}

/** Addresses sorted by how likely a phone on the same Wi-Fi can reach them. */
export const sortAddresses = (list: string[]) => [...list].sort((x, y) => addressRank(x) - addressRank(y) || x.localeCompare(y, "en", { numeric: true }));
