import { getCurrentWindow } from "@tauri-apps/api/window";
import CompactCard from "./components/CompactCard";
import Settings from "./components/Settings";
import SessionsView from "./components/SessionsView";
import { isPreview } from "./lib/bridge";
import "./App.css";

/**
 * Label de la fenêtre Tauri courante — les fenêtres main / settings / sessions
 * chargent la même index.html, seul le label les distingue.
 * `getCurrentWindow()` est synchrone (lit les métadonnées injectées par Tauri) ;
 * hors contexte Tauri (vite dev dans un navigateur), fallback sur le hash.
 */
function currentWindowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return window.location.hash.slice(1) || "main";
  }
}

function App() {
  if (currentWindowLabel() === "sessions") return <SessionsView />;
  if (currentWindowLabel() === "settings") {
    return <Settings />;
  }
  return (
    <div className={`widget${isPreview ? " demo-widget" : ""}`}>
      <CompactCard />
    </div>
  );
}

export default App;
