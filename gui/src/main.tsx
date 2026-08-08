import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { initLang } from "./i18n";
import "./theme.css";

// Before the first render: the dictionary resolves itself at import time, but
// `<html lang>` — what a screen reader picks its voice from — has to be stamped.
initLang();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
