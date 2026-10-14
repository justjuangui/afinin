// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import type { Settings } from "./types";

/** Applies the theme setting (FR-52); `system` follows the OS. */
export function applyTheme(theme: Settings["theme"]) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

function isDark(): boolean {
  const forced = document.documentElement.dataset.theme;
  if (forced) return forced === "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Re-renders when the effective theme changes (OS or setting). */
export function useDarkMode(): boolean {
  const [dark, setDark] = useState(isDark);
  useEffect(() => {
    const update = () => setDark(isDark());
    const mql = window.matchMedia("(prefers-color-scheme: dark)");
    mql.addEventListener("change", update);
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => {
      mql.removeEventListener("change", update);
      observer.disconnect();
    };
  }, []);
  return dark;
}
