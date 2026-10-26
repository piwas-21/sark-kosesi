export type Theme = "system" | "light" | "dark";
export function isTheme(value: unknown): value is Theme {
  return value === "system" || value === "light" || value === "dark";
}
export function resolveTheme(saved: unknown, dark: boolean): "light" | "dark" {
  return isTheme(saved) && saved !== "system" ? saved : dark ? "dark" : "light";
}
// Runs before content paints. CSP receives the request nonce; denied storage is harmless.
export const themeBootstrap = `(()=>{let t='system';try{t=localStorage.getItem('theme')||t}catch{}document.documentElement.dataset.theme=t==='light'||t==='dark'?t:matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'})()`;
