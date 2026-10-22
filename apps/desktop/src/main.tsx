import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { lang } from "./lib/i18n";

document.documentElement.lang = lang;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
