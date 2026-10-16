// Apply the reader's light or dark choice before the page draws, as the rest of the site does
// (Starlight keeps it under `starlight-theme`). Without a choice the page follows the system.
try {
  const t = localStorage.getItem("starlight-theme");
  if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
} catch {
  // storage may be blocked; the system theme applies
}
