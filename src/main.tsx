import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import App from "./App";

const buildServerUrl = import.meta.env.VITE_SERVER_URL;
const buildAppEnv = import.meta.env.VITE_APP_ENV;

if (buildServerUrl && buildAppEnv) {
  invoke("init_build_env", {
    serverUrl: buildServerUrl,
    appEnv: buildAppEnv,
  }).catch((err) => {
    console.warn("init_build_env failed:", err);
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);