import { createApp } from "vue";
import { initLang } from "./i18n";
import AccountCenter from "./AccountCenter.vue";

initLang();
createApp(AccountCenter).mount("#account");
