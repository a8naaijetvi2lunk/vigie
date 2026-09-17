import { normalizeSkin, normalizeTheme, type Config } from "./config";

export function applyAppearance(config: Config) {
  const root = document.documentElement;
  root.dataset.skin = normalizeSkin(config.skin);
  root.dataset.motion = config.animations ? "on" : "off";
  const theme = normalizeTheme(config.theme);
  if (theme === "auto") delete root.dataset.theme;
  else root.dataset.theme = theme;
}
