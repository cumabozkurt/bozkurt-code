import { useEffect, useState } from "react";
import QRCode from "qrcode";

/** Renders `value` as an SVG QR code (always dark-on-white for camera contrast). */
export function QrCode({ value, size = 200, label }: { value: string; size?: number; label: string }) {
  const [svg, setSvg] = useState("");
  useEffect(() => {
    let alive = true;
    QRCode.toString(value, { type: "svg", margin: 0, errorCorrectionLevel: "M", color: { dark: "#0b1220", light: "#ffffff" } })
      .then((s) => alive && setSvg(s.replace("<svg", `<svg width="${size}" height="${size}"`)))
      .catch(() => setSvg(""));
    return () => {
      alive = false;
    };
  }, [value, size]);
  return (
    <div className="qr" role="img" aria-label={label}>
      <div style={{ width: size, height: size }} dangerouslySetInnerHTML={{ __html: svg }} />
    </div>
  );
}
