import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BozkurtApp, platform } from "@bozkurt/ui";

/** Bridge native → UI: tray menu items and bozkurt:// deep links emit "bozkurt://navigate" with a route. */
async function bridgeTauri() {
  if (!platform.isTauri()) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen<string>("bozkurt://navigate", (e) => {
    window.dispatchEvent(new CustomEvent("bozkurt:navigate", { detail: e.payload }));
  });
}
void bridgeTauri();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BozkurtApp mode="local" />
  </StrictMode>,
);
