// SPDX-License-Identifier: MIT
import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import "./styles.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("в разметке нет узла #root");
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
