import { t } from "./i18n.ts";

const MINUTE = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})$/;

function dayKey(date: Date): number {
  return Math.round(new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime() / 86_400_000);
}

/**
 * A Runtime minute (`YYYY-MM-DD HH:MM`, local time) said the way a person would:
 * "今天 15:00", "明天 09:00", "周三 08:00" within the coming week, the date otherwise.
 * Anything that is not such a minute comes back unchanged.
 */
export function readableMinute(minute: string, now: Date = new Date()): string {
  const match = MINUTE.exec(minute);
  if (match === null) return minute;
  const [, y, mo, d, h, mi] = match;
  const at = new Date(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi));
  const clock = `${h}:${mi}`;
  const days = dayKey(at) - dayKey(now);
  if (days === 0) return t("今天 {clock}", { clock });
  if (days === 1) return t("明天 {clock}", { clock });
  if (days === -1) return t("昨天 {clock}", { clock });
  if (days > 1 && days < 7) {
    const names = [t("周日"), t("周一"), t("周二"), t("周三"), t("周四"), t("周五"), t("周六")];
    return `${names[at.getDay()]} ${clock}`;
  }
  const date = at.getFullYear() === now.getFullYear() ? `${mo}-${d}` : `${y}-${mo}-${d}`;
  return `${date} ${clock}`;
}
