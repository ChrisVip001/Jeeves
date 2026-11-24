/** Shell-wide keys: Escape closes the open panel, ⌘, (Ctrl+, elsewhere) opens Settings. */
export function setupNavigation(closePanels: () => void): void {
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closePanels();
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      event.preventDefault();
      document.querySelector<HTMLButtonElement>("#settings-entry button")?.click();
    }
  });
}
