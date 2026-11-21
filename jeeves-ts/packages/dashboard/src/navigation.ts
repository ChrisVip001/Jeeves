/** Responsive navigation owns focus; conversation controllers own no layout state. */
export function closeMobileNavigation(): void {
  document.body.classList.remove("mobile-sidebar");
  const chat = document.querySelector<HTMLElement>("main.chat");
  if (chat) chat.inert = false;
  document
    .querySelectorAll("[data-sidebar-toggle]")
    .forEach((button) => button.setAttribute("aria-expanded", "false"));
}
export function setupNavigation(
  newSession: () => void,
  closePanels: () => void,
): void {
  const narrow = matchMedia("(max-width: 640px)");
  document.querySelectorAll("[data-sidebar-toggle]").forEach((button) =>
    button.addEventListener("click", () => {
      if (!narrow.matches) {
        const closed = document.body.classList.toggle("sidebar-closed");
        document.querySelectorAll("[data-sidebar-toggle]").forEach(toggle => toggle.setAttribute("aria-expanded", String(!closed)));
        document.querySelector<HTMLButtonElement>(closed ? ".mobile-menu" : ".rail-toggle")?.focus();
        return;
      }
      if (document.body.classList.contains("mobile-sidebar")) {
        closeMobileNavigation();
        document.querySelector<HTMLButtonElement>(".mobile-menu")?.focus();
        return;
      }
      document.body.classList.add("mobile-sidebar");
      const chat = document.querySelector<HTMLElement>("main.chat");
      if (chat) chat.inert = true;
      button.setAttribute("aria-expanded", "true");
      document.querySelector<HTMLButtonElement>(".sidebar button")?.focus();
    }),
  );
  narrow.addEventListener("change", closeMobileNavigation);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeMobileNavigation();
      closePanels();
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "n") {
      event.preventDefault();
      newSession();
    }
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      event.preventDefault();
      document
        .querySelector<HTMLButtonElement>("#settings-entry button")
        ?.click();
    }
    if (
      event.key === "Tab" &&
      document.body.classList.contains("mobile-sidebar")
    ) {
      const buttons = [
        ...document.querySelectorAll<HTMLButtonElement>(
          ".sidebar button:not(:disabled), .sidebar a[href]",
        ),
      ].filter((item) => item.getClientRects().length > 0);
      const first = buttons[0],
        last = buttons.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  });
}

export function setupSidebarResize(): void {
  const handle = document.getElementById("sidebar-resizer");
  const app = document.querySelector<HTMLElement>(".app");
  if (app === null || handle === null) return;

  const MIN = 180;
  const MAX = 480;
  const DEFAULT = 244;
  const KEY = "jeeves.sidebar.w";
  const apply = (w: number): void =>
    app.style.setProperty("--sidebar-w", `${Math.round(w)}px`);
  const clamp = (w: number): number => Math.min(MAX, Math.max(MIN, w));

  const saved = Number.parseFloat(localStorage.getItem(KEY) ?? "");
  if (Number.isFinite(saved) && saved >= MIN && saved <= MAX) apply(saved);

  let dragging = false;
  handle.addEventListener("pointerdown", (event) => {
    dragging = true;
    handle.setPointerCapture(event.pointerId);
    document.body.classList.add("resizing");
  });
  handle.addEventListener("pointermove", (event) => {
    if (dragging) apply(clamp(event.clientX));
  });
  const stop = (): void => {
    if (!dragging) return;
    dragging = false;
    document.body.classList.remove("resizing");
    const w = app.style.getPropertyValue("--sidebar-w");
    if (w !== "") localStorage.setItem(KEY, w);
  };
  handle.addEventListener("pointerup", stop);
  handle.addEventListener("pointercancel", stop);

  handle.addEventListener("dblclick", () => {
    apply(DEFAULT);
    localStorage.removeItem(KEY);
  });
  handle.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const current =
      Number.parseFloat(
        getComputedStyle(app).getPropertyValue("--sidebar-w"),
      ) || DEFAULT;
    const w = clamp(current + (event.key === "ArrowRight" ? 24 : -24));
    apply(w);
    localStorage.setItem(KEY, `${Math.round(w)}px`);
  });
}
