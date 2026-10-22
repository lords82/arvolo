import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { initLang } from "./i18n";
// The phone's display face, bundled rather than fetched — see `--font-display`
// in theme.css. One variable file covers every weight the screens set, and no
// glyph of it is rasterised until something asks for the family, which on a
// desktop nothing does.
import "@fontsource-variable/plus-jakarta-sans";
// E il corpo del testo (sezione 14: Inter), per la stessa ragione.
import "@fontsource-variable/inter";
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
