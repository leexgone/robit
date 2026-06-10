import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles/globals.css";

// Initialize theme from localStorage
if (localStorage.getItem("theme") === "dark") {
  document.documentElement.classList.add("dark");
} else if (localStorage.getItem("theme") === "light") {
  document.documentElement.classList.remove("dark");
} else if (window.matchMedia("(prefers-color-scheme: dark)").matches) {
  document.documentElement.classList.add("dark");
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
