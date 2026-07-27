import { createSignal, createEffect } from "solid-js";

export type Theme = "geek-dark" | "classic-dark" | "light";

export const [currentTheme, setCurrentTheme] = createSignal<Theme>("geek-dark");
export const [currentZoom, setCurrentZoom] = createSignal<number>(100);

/** Logo path for the current theme: white logo on dark themes, dark logo on light. */
export const logoSrc = () => (currentTheme() === "light" ? "/logo.png" : "/logo_white.png");

// Set theme class on the document root when it changes
createEffect(() => {
  const t = currentTheme();
  document.documentElement.className = t;
});

// Set zoom style on the document root when it changes
createEffect(() => {
  const z = currentZoom();
  // Using zoom property for Chromium webview engine
  document.documentElement.style.zoom = (z / 100).toString();
});
