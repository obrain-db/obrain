import React from "react";
import ReactDOM from "react-dom/client";
import App from "./app/App";
import { demarrerPontEtat } from "./app/uiStateBridge";
import "./styles/base.scss";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><App /></React.StrictMode>
);

// Le pont d'état d'interface : le navigateur et this-rs partagent un document.
// Sans serveur d'état, l'application fonctionne à l'identique (état local).
demarrerPontEtat();
