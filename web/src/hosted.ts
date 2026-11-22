export function hostedBase(path = globalThis.location?.pathname ?? ""): string {
  return /^\/app\/u-[a-f0-9]{12}(?=\/)/.exec(path)?.[0] ?? "";
}

export function accountKey(name: string): string {
  const base = hostedBase();
  return base ? base + "/" + name : name;
}

export const accountStorage = {
  getItem: (name: string) => localStorage.getItem(accountKey(name)),
  setItem: (name: string, value: string) =>
    localStorage.setItem(accountKey(name), value),
  removeItem: (name: string) => localStorage.removeItem(accountKey(name)),
};

export function loginPage(): string {
  return (
    "/account/?next=" + encodeURIComponent(location.pathname + location.search)
  );
}
