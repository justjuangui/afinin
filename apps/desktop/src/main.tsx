// SPDX-License-Identifier: MIT OR Apache-2.0

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";

// Demo mode (`npm run demo`) swaps the backend for sample data; the branch is
// removed from regular builds.
if (import.meta.env.MODE === "demo") await import("./demo");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
