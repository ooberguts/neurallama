import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";

if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  const theme = new URLSearchParams(location.search).get("theme");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.previewTheme = theme;
}

const app = mount(App, { target: document.getElementById("app")! });
export default app;
