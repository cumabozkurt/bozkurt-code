import { createContext, useContext } from "react";

export interface AppModeCtx {
  mode: "local" | "remote";
  /** host:port shown in remote mode */
  remoteHost?: string;
  /** Forget the stored token and return to the Connect screen (remote mode). */
  disconnect?: () => void;
}

export const AppModeContext = createContext<AppModeCtx>({ mode: "local" });
export const useAppMode = () => useContext(AppModeContext);
