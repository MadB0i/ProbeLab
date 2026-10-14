import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { PanelBoundary } from "./ErrorBoundary";
import "./styles.css";

// A boundary at the root as well as around each panel.
//
// The per-panel boundaries inside `App` catch a fault raised while a child renders. They cannot catch a
// fault raised while `App` itself renders — a boundary is only a defence for what is *below* it, so an
// exception in the component that assembles the routes leaves nothing above it and the window goes
// blank. That case is reachable: the route table is built from fetched data, so a command that
// returns the wrong shape takes out the entire application rather than one panel.
//
// The recovery action is a reload rather than a retry, because there is no partial state left to
// retry: whatever faulted did so before any of it could be reconciled.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <PanelBoundary label="ProbeLab" onRecover={() => window.location.reload()} recoverLabel="RELOAD">
      <App />
    </PanelBoundary>
  </React.StrictMode>,
);
