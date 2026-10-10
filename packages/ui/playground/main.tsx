import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BozkurtApp } from "../src";

// Same-origin API through the vite proxy (see vite.config.ts) unless VITE_BOZKURT_API is set.
const baseUrl = (import.meta.env.VITE_BOZKURT_API as string | undefined) || `${location.origin}/api/v1`;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BozkurtApp mode="local" baseUrl={baseUrl} />
  </StrictMode>,
);
