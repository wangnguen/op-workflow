import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { ErrorBoundary } from "./app/ErrorBoundary";
import { followSheets } from "./shared/sheetMotion";
import { applyTheme, loadPrefs, resolveTheme, systemMode } from "./shared/theme";
import { ThemeProvider } from "./shared/useTheme";
// Fonts ship with the app (CSP allows only 'self'); unicode-range loads just the subsets in use,
// including Vietnamese.
import "@fontsource-variable/inter/wght.css";
import "@fontsource-variable/jetbrains-mono/wght.css";
import "./styles/tokens.css";
import "./styles/themes.css";
import "./styles/global.css";
import "./styles/kit.css";

// before the first paint, so a saved theme never flashes the default
applyTheme(resolveTheme(loadPrefs(), systemMode()));
// dialogs stay in the middle and glide when their content changes
followSheets();

const root = document.getElementById("root");
if (!root) throw new Error("Missing #root element");

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </ErrorBoundary>
  </StrictMode>,
);
