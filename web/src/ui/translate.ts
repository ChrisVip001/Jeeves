import { inject, type InjectionKey } from "vue";
export type UiTranslator = (
  text: string,
  vars?: Record<string, string | number | bigint>,
) => string;
export const uiTranslatorKey: InjectionKey<UiTranslator> =
  Symbol("jeeves-ui-translator");
export function useUiText(): UiTranslator {
  return inject(uiTranslatorKey, (text, vars) =>
    text.replace(/\{(\w+)\}/g, (whole, key: string) =>
      vars && key in vars ? String(vars[key]) : whole,
    ),
  );
}
